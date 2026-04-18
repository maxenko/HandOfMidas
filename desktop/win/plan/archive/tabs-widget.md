# Feature: Tabs Widget for `midas-ui`

## Overview

Add a reusable `Tabs` widget to the `midas-ui` crate. Horizontal row of text labels, an underline beneath the active tab, optional count badges next to labels (e.g. "Positions 3"), inactive-tab hover feedback, and full theme integration. Built by composing iced 0.14 primitives — no `Canvas`, no custom `Widget` trait impl. The widget mirrors the existing `ButtonGroup` API (parent-owned selection, builder pattern, `view(&theme)` terminal).

The reference UI shows tabs of the form `Positions 3 | Orders | Trade History | Balances | …` for a future ledger / bottom-panel surface. This plan delivers the widget, its tests, and a runnable `examples/tabs_demo.rs` in the crate so visual fidelity can be validated independently. Wiring it into `midas-app` is a separate piece of work.

## Research Summary

### Codebase analysis

- `midas-ui` is a leaf crate of 7 widgets in `desktop/win/crates/midas-ui/src/{button, button_group, editable_label, icon_button, label, theme, tooltip}.rs`, all exported individually from `lib.rs:17-31`. One file = one widget.
- All widgets follow the same shape: `Widget::new(...)` → chainable `.method(self) -> Self` builders → terminal `.view(theme: &UiTheme) -> Element<'a, Message>`. Generic over `Message: Clone + 'a`; selection-style widgets are also generic over a value `T: PartialEq + Clone + 'a`.
- The closest analog is `ButtonGroup` (`button_group.rs:30-162`). Its API — `new(items: Vec<(&'a str, T)>, selected: T, on_select: impl Fn(T) -> Message + 'a)` plus `.size/.padding_h/.padding_v/.spacing` — is the template the new widget will mirror.
- `UiTheme` (`theme.rs:15-125`) holds all colors and dimensions. Defaults match `midas-app/src/theme.rs`. The widget will add a small set of dedicated `tab_*` colors, plus one underline-height field; padding and spacing reuse the existing `button_padding_h/v` and `button_group_spacing` fields (the widget *is* a row of buttons).
- Tests live inline as `#[cfg(test)] mod tests` in each widget file (~5 tests/widget): construction, item-count, empty-input, builder-chaining, defaults. Same-file tests have direct access to private fields (see `button_group.rs:216-219` reading `group.size` directly). No integration tests exist in the crate, no `examples/`, no extra binary targets — the new example will be the first.
- **Verified independently:** `midas-ui` is *not yet* listed as a dependency of `midas-app` (`grep midas-ui desktop/win/crates/midas-app/Cargo.toml` returns nothing), and `ButtonGroup` is not used anywhere in app code. The widget can ship in pure isolation; first consumer wiring is a separate piece of work.

### Best practices & idiomatic approach (iced 0.14)

- **Composition over `advanced::Widget`.** Every existing `midas-ui` widget composes iced primitives. Implementing `Widget::draw` is reserved for shapes you can't express with primitives (e.g. animated sliding underline). For a static underline, composition wins.
- **Parent-owned selection.** `pick_list`, `radio`, `toggler`, `checkbox`, and our own `ButtonGroup` all take `selected` by value and emit a message on change — they hold no internal state. Same pattern applies here.
- **Underline rendering — inside the button.** `iced::Border` is a single-width box border (no per-edge widths in 0.14). The clean composed approach is to make each tab a `button(column![ label_row, underline ])` — i.e. the underline lives **inside** the button. The inner `Column` auto-shrinks to its widest child (the label row), so the underline's `width(Length::Fill)` resolves to the label's width naturally — without `Length::Fill` propagating up to the button and stretching all tabs to equal widths. Background is switched between `theme.tab_underline` and `Color::TRANSPARENT` based on selection — same height in both states, no layout jitter.
- **Hover feedback via `button::Status::Hovered` text color.** iced's `button::Style.text_color` propagates to text descendants that don't set their own `.color()`. By leaving the label `text(label)` uncolored and supplying the color in the style closure (varying by `Status`), inactive tabs can brighten on hover with no new theme field.
- **Badge.** A small `Container` with `theme.tab_badge_bg` background and rounded corners, holding `Text(count)` with an explicit `.color(theme.tab_badge_text)` (so it doesn't inherit the button's hover color), placed in a `Row` next to the label.
- **Animation, focus ring, keyboard nav, ARIA semantics.** All deferred. iced 0.14 has `iced::Animation` and `widget::operation::focusable` available, but introducing them now would force a custom `Widget` impl and break the composition pattern. Out of scope for v1; revisit when there's a concrete need.

## Design Decisions

### Decision 1: Item type — tuple vs struct

**Context**: `ButtonGroup` uses `Vec<(&'a str, T)>`. Tabs need a third optional field (badge count), so a plain tuple gets awkward.

**Options**:
1. `Vec<(&'a str, T)>` + separate `Vec<Option<usize>>` — keeps API tuple-shaped but two parallel vecs are error-prone.
2. `Vec<(&'a str, T, Option<usize>)>` — fits the existing pattern but every callsite without badges has to write `None`.
3. `Vec<TabItem<'a, T>>` with `TabItem::new(label, value).with_badge(count)` — slightly more typing for the simple case, much cleaner for mixed cases, lets future fields (icon, disabled, tooltip) extend without breaking callers.

**Recommendation**: Option 3. The reference UI literally has one tab with a badge and five without — this is the common case, and it must read cleanly. The extra builder method (`with_badge`) costs nothing and is consistent with the rest of `midas-ui`.
**Confidence**: high.

### Decision 2: Underline construction — inside the button

**Context**: Need a thin horizontal accent bar under the active tab's label. The visual must track the label width (per the reference image), not stretch tabs to equal widths.

**Options**:
1. `Container` with `Border { width: 2.0, ... }` on bottom edge — *not viable*; `iced::Border` applies width to all four edges in 0.14.
2. **Per-tab `button(column![ label_row, underline ])` — underline inside the button.** The inner `Column` is `Length::Shrink` and sizes to the label row; the underline's `width(Length::Fill)` then resolves to that column's width = label width. Button padding sits outside the underline (visually the underline runs only under the label, not the button padding — matches the reference). The underline's background switches between `theme.tab_underline` and `Color::TRANSPARENT` based on selection.
3. Outer `column![ button, underline ]` (underline outside the button) — initially considered, **rejected**: a `Length::Fill` underline child propagates a Fill request up through the column to the button, which then competes with sibling tabs in the outer `Row`. Result: equal-width stretched tabs, not natural label-width tabs. The plan's previous fallback ("`Length::Shrink` plus measured label width") isn't achievable in iced 0.14 — there's no callsite-level text measurement before layout.
4. `Stack` widget with an overlay underline child — works but iced docs warn "use Stack sparingly" and there's no real absolute positioning, only z-stacking. Overkill for a 2 px bar.
5. Custom `Widget` trait impl with `renderer.fill_quad` — clean but only worth it if we later need an animated underline that slides between tabs.

**Recommendation**: Option 2. Pure composition, theme-driven, zero advanced API surface. The underline naturally tracks the inner column's width, sidestepping the Fill-propagation problem. A future v2 with sliding animation can switch to Option 5 without changing the widget's public API.
**Confidence**: high.

### Decision 3: Selected state ownership

**Context**: Iced widgets split into stateful (`text_input`) and stateless (`pick_list`, `ButtonGroup`).

**Options**:
1. Internal state — Tabs owns the selected index, parent passes only initial value.
2. Parent-owned — selected value passed in each render, selection change emits a message.

**Recommendation**: Option 2. Matches `ButtonGroup`, `pick_list`, and the rest of `midas-ui`. The active tab is application state (it controls which content view to render), so the parent must hold it anyway.
**Confidence**: high.

### Decision 4: Theme additions — seven dedicated fields, reuse `button_padding_*` only

**Context**: The widget needs colors for the underline, active/inactive labels, and the badge. It also needs underline thickness, padding, and inter-tab spacing.

**Options**:
1. Reuse only — underline uses `accent`, badge uses existing fields, no theme growth at all.
2. Dedicated for everything (initial proposal: 9 fields including `tab_padding_h`, `tab_padding_v`, `tab_spacing`).
3. Hybrid, full reuse for layout — dedicated for visuals (5 colors + underline height = 6 fields); reuse `button_padding_h/v` *and* `button_group_spacing` for layout.
4. **Hybrid, partial reuse — dedicated for visuals AND for inter-tab spacing (5 colors + underline height + tab spacing = 7 fields); reuse `button_padding_h/v` only.**

**Recommendation**: Option 4. Padding values (`8.0` h, `4.0` v) are reasonable for tabs — those genuinely *can* share with buttons. But `button_group_spacing = 1.0` is calibrated for ButtonGroup's contiguous toggle-pill visual; tabs are visually different (distinct labels with breathing room, ~16 px in the reference image). Reusing it would force every callsite to call `.spacing(16.0)` — the inverse of "sensible defaults." Colors and underline height have no existing analog and stay dedicated, so the active-tab underline color is independently themeable from `accent`. Builder methods `.padding_h/.padding_v/.spacing` still allow per-callsite override.
**Confidence**: high.

### Decision 5: Inactive-tab hover feedback

**Context**: With the button background painted transparent, the original plan left inactive tabs with zero visual feedback on hover. For a tab bar — where the dominant interaction is hover-then-click — this is a real UX miss.

**Options**:
1. Defer — no hover state. (Original plan.)
2. **Brighten the inactive tab's text color on hover.** Leverage `button::Style.text_color` (which propagates to descendant text widgets that don't set their own `.color()`) and switch on `button::Status::Hovered`. No new theme field needed: reuse `theme.text_primary` for the hover color (one tone above `tab_text_inactive`). Active tab's color stays at `tab_text_active` regardless of hover.
3. Add a `tab_text_hover` theme field — finer control at the cost of theme growth.

**Recommendation**: Option 2. One additional `match` arm in the style closure, no new theme field, removes the UX miss. The badge text widget sets its color explicitly so it does not inherit the button's hover shift.
**Confidence**: high.

### Decision 6: Out of scope for v1

- **Keyboard focus ring & arrow-key navigation.** The reference screenshot shows a blue focus ring on "Trade History" — that's keyboard focus, distinct from the underline (which marks the *active* tab). Implementing real iced focus traversal touches `widget::operation`, requires app-level keyboard subscriptions, and is unrelated to the visual widget shape. Defer.
- **Animated sliding underline.** Would require a custom `Widget` impl. Defer.
- **Tab close buttons, drag-to-reorder, overflow menu, scrollable tab strip.** Not in the reference. Defer.
- **Wiring into `midas-app`.** No callsite currently exists; the bottom-panel surface from the reference is greenfield. Adding `midas-ui` as a dependency of `midas-app` and migrating any existing pseudo-tab UI is separate work.

## Implementation Plan

### Slice 1: Extend `UiTheme` with seven tab fields

**Goal**: Add the colors and dimensions the new widget will read.
**Depends on**: None.
**Files to create or modify**:
- `desktop/win/crates/midas-ui/src/theme.rs` — add the seven new fields below to `UiTheme`, populate them in `Default`, and extend the existing `default_creates_valid_theme` and `default_spacing_values_are_positive` tests to cover them.

**Key implementation details**:
- Seven new fields, grouped after the existing tooltip block:
  ```rust
  // -- Tabs --
  /// Color of the underline beneath the active tab.
  pub tab_underline: Color,
  /// Height of the active-tab underline in logical pixels.
  pub tab_underline_height: f32,
  /// Text color for the active tab label.
  pub tab_text_active: Color,
  /// Text color for inactive tab labels.
  pub tab_text_inactive: Color,
  /// Background color for the count badge next to a tab label.
  pub tab_badge_bg: Color,
  /// Text color inside the count badge.
  pub tab_badge_text: Color,
  /// Spacing in logical pixels between adjacent tabs.
  pub tab_spacing: f32,
  ```
- Defaults (calibrated against the reference image and the existing palette):
  ```rust
  tab_underline: Color::from_rgb(0.22, 0.55, 0.95),       // dark-blue accent at time of authoring
  tab_underline_height: 2.0,
  tab_text_active: Color::from_rgb(0.88, 0.88, 0.92),     // high-contrast label
  tab_text_inactive: Color::from_rgb(0.55, 0.55, 0.60),   // dimmed label
  tab_badge_bg: Color::from_rgb(0.18, 0.18, 0.22),        // slightly lighter than `surface`
  tab_badge_text: Color::from_rgb(0.70, 0.70, 0.75),
  tab_spacing: 16.0,                                       // breathing room between distinct labels
  ```
  Comments describe *intent*, not "matches X" relationships — Decision 4's "independent values" stance means the literal RGB values are free to drift from sibling fields without invalidating the comment.
- **No** new padding fields. Tab padding inherits `button_padding_h/v`. Inter-tab spacing is dedicated (Decision 4) because `button_group_spacing = 1.0` is calibrated for contiguous toggle pills, not distinct tab labels. The widget's `.padding_h/.padding_v/.spacing` builder methods can override per-callsite.

**Testing**:
- In `default_creates_valid_theme`, append all five new colors (`tab_underline`, `tab_text_active`, `tab_text_inactive`, `tab_badge_bg`, `tab_badge_text`) to the existing color array.
- In `default_spacing_values_are_positive`, add `assert!(theme.tab_underline_height > 0.0);` and `assert!(theme.tab_spacing > 0.0);`. (The existing assertion bucket already mixes spacing, font sizes, and timing — this fits.)

**Done when**: `cargo test -p midas-ui` passes, `cargo clippy -p midas-ui -- -D warnings` is clean, and the new fields have `///` rustdoc.

### Slice 2: `Tabs` widget — labels, badges, hover, demo example

**Goal**: A complete, runnable `Tabs` widget with badges, hover feedback, and a runnable visual-verification example.
**Depends on**: Slice 1.
**Files to create or modify**:
- `desktop/win/crates/midas-ui/src/tabs.rs` (new) — the widget module.
- `desktop/win/crates/midas-ui/src/lib.rs` — add `pub mod tabs;` in alphabetical order with the other widget modules and `pub use tabs::{TabItem, Tabs};` in the re-exports. Update the doc-comment widget list to include `Tabs`.
- `desktop/win/crates/midas-ui/examples/tabs_demo.rs` (new) — minimal iced application that renders a single `Tabs` instance with the reference's labels (`Positions 3 | Orders | Trade History | Balances | Account Summary | Notifications log`) and toggles selection on click. Used for visual-fidelity verification.
- `desktop/win/crates/midas-ui/Cargo.toml` — no change needed; `[[example]]` targets in `examples/` are auto-detected by Cargo.

**Key implementation details**:

Public types:
```rust
pub struct TabItem<'a, T> {
    label: &'a str,
    value: T,
    badge: Option<usize>,
}

pub struct Tabs<'a, T, Message> {
    items: Vec<TabItem<'a, T>>,
    selected: T,
    on_select: Box<dyn Fn(T) -> Message + 'a>,
    size: Option<f32>,
    padding_h: Option<f32>,
    padding_v: Option<f32>,
    spacing: Option<f32>,
    underline_height: Option<f32>,
}
```

Builders:
- `TabItem::new(label, value) -> Self` (sets `badge: None`).
- `TabItem::with_badge(self, count: usize) -> Self`.
- `Tabs::new(items, selected, on_select)` plus `.size`, `.padding_h`, `.padding_v`, `.spacing`, `.underline_height`, `.item_count()`, `.view(&UiTheme) -> Element<'a, Message>` — names and shapes match `ButtonGroup`.

Per-tab view construction (inside `items.into_iter().map(...)`):

1. **Defaults resolution**: `font_size = self.size.unwrap_or(theme.button_font_size)`; `pad_h = self.padding_h.unwrap_or(theme.button_padding_h)`; `pad_v = self.padding_v.unwrap_or(theme.button_padding_v)`; `spacing = self.spacing.unwrap_or(theme.tab_spacing)`; `underline_h = self.underline_height.unwrap_or(theme.tab_underline_height)`.

2. **Label row** — bare `text(label).size(font_size)` *without* `.color()`, so it inherits from the button's `text_color` (which the style closure picks per `Status`). If `badge.is_some()`, wrap in `row![text_widget, badge_widget].spacing(6.0).align_y(Center)`. The badge is a `container(text(n).size((font_size - 2.0).max(8.0)).color(theme.tab_badge_text))` with `padding([2.0, 6.0])` and a style closure setting `background: Some(theme.tab_badge_bg.into())` and `border.radius: 4.0.into()`. The `.max(8.0)` floor protects against tiny `font_size` overrides driving badge text to zero or negative. Badge text sets its own `.color()` so it doesn't inherit the button's hover-shifted text color.

3. **Underline** — `container(Space::with_width(Length::Fill)).height(Length::Fixed(underline_h)).width(Length::Fill)` with a `move` style closure capturing a `Color` that is `theme.tab_underline` if `is_selected` else `Color::TRANSPARENT`. Always rendered, transparent when inactive — keeps height stable.

4. **Inner column** — `column![label_row, underline].spacing(LABEL_TO_UNDERLINE_GAP)`. Define `const LABEL_TO_UNDERLINE_GAP: f32 = 4.0;` at module scope with a one-line comment ("vertical gap between label baseline area and the underline bar"). Pulled out as a named constant rather than a magic number so a future reader sees the visual relationship; not promoted to a theme field because the gap is structurally tied to the underline rendering, not a palette choice. The column is `Length::Shrink` (default) and sizes to the label row, so the underline's `Length::Fill` resolves to label-row width.

5. **Outer button** — `button(inner_column).padding([pad_v, pad_h]).on_press(msg).style(move |_iced_theme, status| { ... })`. The style closure returns `button::Style { background: Some(Color::TRANSPARENT.into()), text_color: hover_or_normal, border: Border::default(), ..Default::default() }`, where `hover_or_normal` is:
   - `theme.tab_text_active` if `is_selected` (any status)
   - `theme.text_primary` if `!is_selected && status == Hovered`
   - `theme.tab_text_inactive` otherwise

   Capture all colors by value into the closure (`Color: Copy`, `UiTheme: Clone` — pull out the three `Color`s into local bindings before the closure to keep the closure `'static`-friendly).

6. **Outer layout** — `Row::with_children(buttons).spacing(spacing).into()`. Same shape as `ButtonGroup::view` (`button_group.rs:155`).

**Reference patterns to copy** (read these first):
- `desktop/win/crates/midas-ui/src/button_group.rs:48-156` — full structure, including the `button::Status` closure pattern and `Row::with_children().spacing(...)` outer layout.
- `desktop/win/crates/midas-ui/src/tooltip.rs` — example of a widget that wraps `container::Style` with theme-driven background.

**`examples/tabs_demo.rs` skeleton** (concrete enough to write without further design):
- Standard `iced::application(...)` entry point, model holds `selected: TabId` enum.
- `view()` returns `Tabs::new(items, model.selected, Message::Selected).view(&UiTheme::default())` wrapped in a `container(...).padding(20)` against a dark background that mirrors `midas-app/src/theme.rs` to make the underline visible.
- Run via `cargo run --example tabs_demo -p midas-ui`. This is the visual-fidelity check that replaces the previous "spike" — it lives in the repo, anyone can re-run it, and it's the de facto integration test for the widget until a real consumer lands.

**Testing** (inline `#[cfg(test)] mod tests` in `tabs.rs`, mirroring `button_group.rs:164-222` — same-file tests have direct access to private fields, no public accessor required):
- `tabs_constructs_without_panic` — basic `Tabs::new(...)`.
- `tabs_counts_items` — `.item_count()` returns expected length.
- `tabs_empty_items` — empty `Vec` constructs cleanly.
- `tabs_builder_chains` — every builder method recorded into the `Option<f32>` field.
- `tab_item_constructs_without_badge` — `TabItem::new(...)` then `assert_eq!(item.badge, None)` (private-field access from same-file test, like `button_group.rs:216-219` does for `group.size`).
- `tab_item_with_badge_records_count` — `TabItem::new(...).with_badge(3)` then `assert_eq!(item.badge, Some(3))`.
- `tabs_mixed_items_count` — vec of items with mixed badge presence; verify `item_count`.

No render-output assertions — iced widgets aren't render-testable without a windowed harness. Visual verification is done by running `cargo run --example tabs_demo -p midas-ui`.

**Done when**:
- `cargo test -p midas-ui`, `cargo clippy -p midas-ui --all-targets -- -D warnings`, and `cargo fmt --all -- --check` all pass.
- `cargo build --example tabs_demo -p midas-ui` succeeds (verifies Cargo's example auto-discovery picked up the new file before any visual check).
- Widget is publicly re-exported from `lib.rs` and the doc-comment widget list mentions `Tabs`.
- `cargo run --example tabs_demo -p midas-ui` opens a window matching the reference image: tabs at natural label widths with ~16 px gaps, underline only beneath the active label, badge on `Positions`, inactive tabs brighten on hover.

### Dependency Summary

Strictly serial: 1 → 2. Slice 1 is small (six theme fields + two test extensions) and unblocks Slice 2 quickly. There's no parallelization opportunity — and no third slice, because the badge work is small enough to fold into the same change as the widget itself.

## Risks & Unknowns

- **Visual fidelity to the reference image is verifiable but not asserted in tests.** `examples/tabs_demo.rs` is the verification path. Mitigation: any tweak to the theme defaults (Slice 1) or per-tab construction (Slice 2) gets re-validated by re-running the example. The example also doubles as living documentation of how to use the widget. Theme defaults are independent fields (Decision 4), so a future tweak doesn't require code changes to the widget.
- **Hover text-color shift relies on `button::Style.text_color` propagating to inner text widgets.** This is iced 0.14's documented behavior, but if a user explicitly sets `.color()` on a text inside the button, it overrides. The plan deliberately leaves the label `text(label)` uncolored for exactly this reason; the badge `text(n)` does set `.color()` so the badge text stays at `theme.tab_badge_text` regardless of hover. If the inheritance behavior changes between iced patch versions, the example will catch it visually.
- **Badge style closure captures `theme.tab_badge_bg` and `theme.tab_badge_text` by value (Color is Copy).** Pull these into local bindings before the `move` closure so iced's `Fn(...) + 'a` bound is satisfied. Same pattern as `button_group.rs:135-149`.
- **No keyboard focus.** Real focus traversal is out of scope (Decision 6). A user navigating with Tab key won't see a focus ring or be able to activate tabs via keyboard. Acceptable for v1; revisit when a consumer surface needs it.

## Testing Strategy

Match the existing `midas-ui` style: inline `#[cfg(test)] mod tests` per file, ~5–7 tests covering construction, item count, empty input, builder chaining, badge recording, and mixed items. Same-file tests have private-field access; no public accessors needed.

Visual verification:
```bash
cargo run --example tabs_demo -p midas-ui
```

Verification commands (from `desktop/win/`):
```bash
cargo test -p midas-ui
cargo clippy -p midas-ui --all-targets -- -D warnings   # --all-targets covers the example
cargo fmt --all -- --check
```

Full workspace gate before considering the feature done:
```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

## Non-Goals / Out of Scope

- Keyboard focus traversal, focus ring rendering, arrow-key navigation between tabs.
- Animated sliding underline between tabs.
- Closeable tabs, drag-to-reorder, overflow menus, horizontal scrolling for many tabs.
- Tooltips on tabs (use the existing `Tooltip` widget at the callsite if needed).
- Disabled tab states.
- ARIA / accessibility semantics (iced 0.14 does not expose them).
- Wiring `Tabs` into any specific `midas-app` surface. The reference image shows a future ledger / bottom panel that does not exist yet; adding `midas-ui` as a dependency of `midas-app` and building that panel is a separate piece of work.

## Review Notes

- **Item type chose struct over tuple** (Decision 1) so badges feel idiomatic. Mild divergence from `ButtonGroup`'s `Vec<(&'a str, T)>` shape; pays off the moment a callsite has heterogeneous tabs (the common case in the reference image).
- **Underline lives inside the button** (Decision 2). The earlier "outer column" approach would have stretched all tabs to equal widths via `Length::Fill` propagation. The current approach is the iced-idiomatic way to get a label-width underline without reaching for `Stack` or a custom `Widget` impl.
- **Theme grew by 7 fields, not 9.** Padding reuses `button_padding_*` (those values fit). Spacing is dedicated (`tab_spacing: 16.0`) because `button_group_spacing = 1.0` is calibrated for ButtonGroup's contiguous toggle pills — tabs need breathing room. Five `tab_*` colors + `tab_underline_height` + `tab_spacing` remain dedicated.
- **Hover feedback is in scope** (Decision 5) — single `match` arm, no extra theme field. Removes a real UX miss for what is fundamentally a hover-then-click widget.
- **`examples/tabs_demo.rs` ships with the widget.** Solves the previous plan's "5-minute spike" problem (which had no harness to run in) and gives the team a permanent visual-verification path until a real consumer wires up `Tabs`.
