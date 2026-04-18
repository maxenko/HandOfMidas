//! iced 0.14 shader widget that renders a single chart thumbnail.
//!
//! Wraps [`midas_render::SparklinePipeline`] (mountain / area fill) in
//! an `iced::widget::shader::Primitive` and composes it with a small
//! iced text overlay so the interval label ("1m", "5m", "1d") is
//! rendered by iced rather than the GPU. The whole stack is wrapped in
//! a `mouse_area` that emits the caller-supplied click message on
//! button release.
//!
//! ## Pipeline ownership
//!
//! The [`SparklinePipeline`] per-widget state lives inside an iced
//! `shader::Pipeline` impl ([`ThumbnailPipeline`]). iced's wgpu
//! renderer owns exactly one [`ThumbnailPipeline`] per surface inside
//! its `Storage` map, keyed by `TypeId` of the primitive type.
//! Construction happens lazily via [`shader::Pipeline::new`] on the
//! first thumbnail prepare, so the `wgpu::Device` is only ever
//! consulted after iced has surfaced one. Device recreation is handled
//! by iced itself — when the surface is rebuilt, iced rebuilds
//! `Storage` and our [`shader::Pipeline::new`] fires again with the
//! fresh device. This mirrors the pattern `ChartPipeline` uses in
//! `chart_widget.rs` and satisfies Decision 7 of
//! `plan/feature-chart-thumbnail-cells.md` without a module-level
//! `OnceLock`.
//!
//! Inside [`ThumbnailPipeline`], per-widget [`SparklinePipeline`]
//! instances live in a `HashMap<u64, SparklinePipeline>` keyed by the
//! caller-supplied [`ThumbnailSnapshot::widget_key`]. Per-widget
//! storage is required because iced runs every primitive's
//! `prepare()` up-front and then all `draw()`s in sequence. A single
//! shared storage buffer would be clobbered by the last thumbnail
//! before any of them draw.
//!
//! ## Empty-data handling
//!
//! When [`ThumbnailSnapshot::closes`] has fewer than two samples the
//! `SparklinePipeline` itself skips the draw call (see
//! `SparklinePipeline::render`). The widget still runs through
//! `prepare` + `draw`; the shader simply contributes no fragments, and
//! the label overlay picks up the empty-state glyph through
//! [`ThumbnailSnapshot::effective_label`]. The cell background (set by
//! the grid wrapper, not this widget) shows through unchanged.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use iced::alignment::{Horizontal, Vertical};
use iced::widget::shader::{self, Viewport};
use iced::widget::{button, container, stack, text};
use iced::{mouse, Color, Element, Fill, Length, Rectangle};

use midas_core::Timeframe;
use midas_render::SparklinePipeline;

/// Glyph the widget substitutes when [`ThumbnailSnapshot::closes`] has
/// too few samples to form a triangle strip (loading, zero-row buffer,
/// or a single close).
pub const EMPTY_LABEL: &str = "…";

/// Immutable per-frame snapshot consumed by the thumbnail widget.
///
/// Cheap to clone: the close slice is shared via [`Arc`] and the rest
/// are `Copy` fields or owned `String`s that Slice 4 reuses per row.
/// Built by the caller (watchlist / order-blotter column `cell()`
/// implementation) from
/// [`crate::thumbnail_data::ThumbnailDataStore::fetch`] plus the
/// per-ticker interval label read from
/// [`crate::thumbnail_store::ThumbnailStore`].
#[derive(Clone, Debug)]
pub struct ThumbnailSnapshot {
    /// Stable identifier for this widget cell. Used to key per-widget
    /// GPU state inside [`ThumbnailPipeline`]. Callers assign it — the
    /// watchlist uses `hash((&symbol, tf))`; the demo example uses a
    /// counter.
    pub widget_key: u64,
    /// Tail of close prices to plot (oldest first).
    ///
    /// An empty slice — or one with a single element — tells the
    /// widget to render a placeholder instead of a mountain.
    pub closes: Arc<Vec<f32>>,
    /// Low edge of the price axis (baseline of the mountain fill).
    pub y_min: f32,
    /// High edge of the price axis (top of the mountain fill).
    pub y_max: f32,
    /// Fill color as linear RGBA (0..=1 per channel). Callers pull
    /// this from a theme; the widget never picks a default.
    pub color: [f32; 4],
    /// Version stamp folding in
    /// [`midas_core::CandleBuffer::version`] plus any app-side
    /// invalidators (theme swap, etc.). The widget compares this
    /// against its per-widget state to skip GPU uploads when nothing
    /// changed.
    pub generation: u64,
    /// Interval label (e.g. "1m"). Rendered by the iced text overlay
    /// — never on the GPU.
    pub label: String,
}

impl ThumbnailSnapshot {
    /// Label the widget displays when [`Self::closes`] has fewer than
    /// two samples to plot.
    ///
    /// Exposed as an associated helper so callers that assemble their
    /// own empty-state snapshots (tests, examples, Slice 4's watchlist
    /// column) share the same glyph with the widget itself.
    #[allow(dead_code)] // consumed only by tests + examples via the library target
    pub fn label_for_empty() -> &'static str {
        EMPTY_LABEL
    }

    /// Effective label to show for this snapshot — the caller-supplied
    /// [`Self::label`] when there's data, or the empty-state glyph
    /// when the close slice is too short to plot.
    pub fn effective_label(&self) -> &str {
        if self.closes.len() < 2 {
            EMPTY_LABEL
        } else {
            self.label.as_str()
        }
    }
}

// ── Program ──────────────────────────────────────────────────────────

/// `shader::Program` for a single thumbnail.
///
/// Generic over the caller's message type because the widget itself
/// never emits messages — clicks are handled by the outer `mouse_area`
/// wrapping the returned element, not by the shader. The
/// `PhantomData` placeholder honors the iced `Program<Message>` bound
/// without imposing `Send + Sync` or similar on the caller.
#[derive(Debug)]
pub struct ThumbnailProgram<M> {
    snapshot: ThumbnailSnapshot,
    _message: std::marker::PhantomData<M>,
}

impl<M> ThumbnailProgram<M> {
    /// Build a program from a snapshot.
    pub fn new(snapshot: ThumbnailSnapshot) -> Self {
        Self {
            snapshot,
            _message: std::marker::PhantomData,
        }
    }
}

/// Per-widget state iced threads across frames for a single thumbnail.
///
/// Holds the last uploaded `generation` so [`ThumbnailPrimitive::prepare`]
/// can skip re-uploading the storage buffer when nothing has changed.
#[derive(Debug)]
pub struct ThumbnailWidgetState {
    /// Last generation stamp observed for this widget. `u64::MAX` is
    /// the "never uploaded" sentinel — chosen so any real generation
    /// (seeded from zero) forces an upload on the first frame.
    last_generation: u64,
}

impl ThumbnailWidgetState {
    /// Build a fresh state in the "never uploaded" configuration.
    fn new() -> Self {
        Self {
            last_generation: u64::MAX,
        }
    }
}

impl Default for ThumbnailWidgetState {
    fn default() -> Self {
        Self::new()
    }
}

impl<M> shader::Program<M> for ThumbnailProgram<M>
where
    M: 'static,
{
    type State = ThumbnailWidgetState;
    type Primitive = ThumbnailPrimitive;

    fn draw(
        &self,
        state: &Self::State,
        _cursor: mouse::Cursor,
        _bounds: Rectangle,
    ) -> Self::Primitive {
        ThumbnailPrimitive {
            snapshot: self.snapshot.clone(),
            last_generation: state.last_generation,
        }
    }

    // `update` intentionally falls through to the trait default
    // (returns `None`, no state mutation). The per-widget generation
    // check lives inside [`ThumbnailPipeline`], which owns a
    // `last_generation` field per widget_key — the authoritative
    // "what did we last upload to the GPU" mirror. Duplicating it on
    // `Program::State` would race the pipeline side whenever iced
    // rebuilds `Storage` after a device loss.
}

// ── Primitive ────────────────────────────────────────────────────────

/// Per-frame rendering data for one thumbnail.
#[derive(Debug)]
pub struct ThumbnailPrimitive {
    /// Snapshot the primitive will upload + draw.
    snapshot: ThumbnailSnapshot,
    /// Widget's `last_generation` at the time `Program::draw` ran, for
    /// debug / diagnostics only. The authoritative "what did we last
    /// upload" mirror lives on [`ThumbnailPipelineEntry::generation`].
    #[allow(dead_code)]
    last_generation: u64,
}

impl shader::Primitive for ThumbnailPrimitive {
    type Pipeline = ThumbnailPipeline;

    fn prepare(
        &self,
        pipeline: &mut Self::Pipeline,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _bounds: &Rectangle,
        _viewport: &Viewport,
    ) {
        let entry = pipeline.entry_for(self.snapshot.widget_key, device);

        // Skip the upload when nothing has changed since we last
        // uploaded for this widget_key. Generation is the
        // per-widget-key mirror kept inside the pipeline.
        if entry.generation == Some(self.snapshot.generation) {
            return;
        }

        entry.pipe.update_buffer(
            device,
            queue,
            &self.snapshot.closes,
            self.snapshot.y_min,
            self.snapshot.y_max,
            self.snapshot.color,
        );
        entry.generation = Some(self.snapshot.generation);
    }

    /// We deliberately fall back to [`Self::render`] (return `false`
    /// from `draw`) so iced opens a fresh render pass for us with a
    /// `clip_bounds: &Rectangle<u32>` we can use as both viewport and
    /// scissor. This is the pattern the official
    /// `iced/examples/custom_shader` uses (see PR iced-rs/iced#2738).
    ///
    /// We can't use the shared `draw()` path because iced's shared
    /// render pass sets the wgpu viewport to the shader widget's full
    /// `instance.bounds`. The sparkline shader emits clip-space
    /// `[-1, +1]`, so anything wider than the cell — and `Length::Fill`
    /// inside our `stack!`/`button` chain *can* be — paints over
    /// sibling cells and adjacent panes. A fresh per-instance render
    /// pass with viewport = `clip_bounds` makes that mathematically
    /// impossible: `[-1, +1]` always maps onto exactly the visible
    /// cell, no matter what `instance.bounds` says.
    fn draw(&self, _pipeline: &Self::Pipeline, _render_pass: &mut wgpu::RenderPass<'_>) -> bool {
        false
    }

    fn render(
        &self,
        pipeline: &Self::Pipeline,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        clip_bounds: &Rectangle<u32>,
    ) {
        let Some(entry) = pipeline.entries.get(&self.snapshot.widget_key) else {
            return;
        };
        if clip_bounds.width == 0 || clip_bounds.height == 0 {
            return;
        }

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("midas.thumbnail.sparkline.pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });

        pass.set_viewport(
            clip_bounds.x as f32,
            clip_bounds.y as f32,
            clip_bounds.width as f32,
            clip_bounds.height as f32,
            0.0,
            1.0,
        );
        pass.set_scissor_rect(
            clip_bounds.x,
            clip_bounds.y,
            clip_bounds.width,
            clip_bounds.height,
        );
        entry.pipe.render(&mut pass);
    }
}

// ── Pipeline ─────────────────────────────────────────────────────────

/// Per-widget GPU state held inside [`ThumbnailPipeline`].
struct ThumbnailPipelineEntry {
    /// Owned sparkline GPU pipeline. Each widget_key owns its own
    /// storage + uniform buffers so that iced's
    /// all-prepare-then-all-draw batching cannot make sibling
    /// thumbnails clobber each other's data.
    pipe: SparklinePipeline,
    /// Last [`ThumbnailSnapshot::generation`] uploaded through this
    /// entry. `None` means the entry has been created but nothing has
    /// been uploaded yet (first frame after pipeline construction).
    generation: Option<u64>,
}

impl std::fmt::Debug for ThumbnailPipelineEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThumbnailPipelineEntry")
            .field("generation", &self.generation)
            .finish()
    }
}

/// Shared per-surface storage for every thumbnail widget in the app.
///
/// iced creates one [`ThumbnailPipeline`] per surface through
/// [`shader::Pipeline::new`]. Each thumbnail widget's primitive looks
/// up or inserts its own [`ThumbnailPipelineEntry`] keyed by
/// [`ThumbnailSnapshot::widget_key`]. Separate per-widget entries are
/// required because iced runs all `prepare()`s before any `draw()`,
/// so a single shared storage buffer would be clobbered by the last
/// thumbnail's prepare before any of them draw.
#[derive(Debug)]
pub struct ThumbnailPipeline {
    /// Color format this pipeline renders into. Captured so new
    /// per-widget `SparklinePipeline` instances can match it.
    format: wgpu::TextureFormat,
    /// Per-widget GPU state, keyed by
    /// [`ThumbnailSnapshot::widget_key`].
    entries: HashMap<u64, ThumbnailPipelineEntry>,
}

impl ThumbnailPipeline {
    /// Look up the entry for `widget_key`, creating one on miss.
    fn entry_for(&mut self, widget_key: u64, device: &wgpu::Device) -> &mut ThumbnailPipelineEntry {
        let format = self.format;
        self.entries
            .entry(widget_key)
            .or_insert_with(|| ThumbnailPipelineEntry {
                pipe: SparklinePipeline::new(device, format),
                generation: None,
            })
    }
}

impl shader::Pipeline for ThumbnailPipeline {
    fn new(_device: &wgpu::Device, _queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        tracing::info!("Creating ThumbnailPipeline with format {:?}", format);
        Self {
            format,
            entries: HashMap::new(),
        }
    }
}

// ── Public helper ────────────────────────────────────────────────────

/// Build a drop-in iced [`Element`] for one thumbnail cell.
///
/// Assembles:
/// 1. a `shader` widget carrying a [`ThumbnailProgram`] with the
///    snapshot;
/// 2. a small iced text overlay in the bottom-right corner showing
///    the effective label (the caller's interval string, or `…` when
///    the snapshot has no plottable data);
/// 3. a transparent `button` wrapper that emits `on_click` on button
///    release.
///
/// ## Why `button` (not `mouse_area`)
///
/// iced 0.14's `mouse_area::on_release` publishes the click message
/// but does **not** call `shell.capture_event()`. When a thumbnail
/// lives inside a clickable row (as it does in the watchlist + order
/// blotter grids), the inner `on_release` fires the
/// `ThumbnailIntervalCycle` message and then the outer row's
/// `on_release` also fires, selecting the row as a side effect.
///
/// `button` captures both the `ButtonPressed` and the `ButtonReleased`
/// left-click events, so the outer row never sees the click. The
/// button is styled fully transparent so visually it behaves like a
/// plain shader widget — no hover background, no press chrome.
///
/// The returned element is sized `Fill × Fill`, so the caller decides
/// the final bounding box by wrapping it in a container, grid cell,
/// or fixed-size row.
pub fn thumbnail_cell<M>(snapshot: ThumbnailSnapshot, on_click: M) -> Element<'static, M>
where
    M: 'static + Clone,
{
    let label_color = Color::from_rgba(0.85, 0.85, 0.85, 0.85);
    let label_text = text(snapshot.effective_label().to_string())
        .size(9)
        .color(label_color);

    let label_overlay = container(label_text)
        .padding(2)
        .align_x(Horizontal::Right)
        .align_y(Vertical::Bottom)
        .width(Fill)
        .height(Fill);

    let program = ThumbnailProgram::<M>::new(snapshot);
    let shader_widget = iced::widget::shader(program)
        .width(Length::Fill)
        .height(Length::Fill);

    // `stack!` defaults to `Length::Shrink × Shrink` and per its own
    // docs "will not inspect the [`Vec`], which means it won't
    // automatically adapt to the sizing strategy of its contents.
    // If any of the children have a [`Length::Fill`] strategy, you
    // will need to call [`Stack::width`] or [`Stack::height`]
    // accordingly." Without these, the stack — and therefore the
    // shader inside — can pick up oversized layout bounds, which then
    // become the wgpu viewport, letting the sparkline's clip-space
    // `[-1, +1]` paint over sibling cells.
    let stacked = stack![shader_widget, label_overlay]
        .width(Length::Fill)
        .height(Length::Fill);

    button(stacked)
        .on_press(on_click)
        .padding(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(transparent_button_style)
        .clip(true)
        .into()
}

/// Fully-transparent [`button`] style used by [`thumbnail_cell`]. Keeps
/// the button's event-capture semantics without painting any chrome on
/// top of the shader widget.
fn transparent_button_style(_theme: &iced::Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: Color::WHITE,
        border: iced::Border::default(),
        shadow: iced::Shadow::default(),
        snap: false,
    }
}

// ── Widget-key + label helpers ──────────────────────────────────────

/// Stable [`ThumbnailSnapshot::widget_key`] derived from `(symbol, tf)`.
///
/// Used by the watchlist + order-blotter column builders so the same
/// cell keeps the same key across frames. iced runs every primitive's
/// `prepare()` up-front before any `draw()`, so per-widget GPU storage
/// buffers inside [`ThumbnailPipeline`] must be keyed by something
/// stable — otherwise sibling thumbnails clobber each other's uploads.
pub fn widget_key(symbol: &str, tf: Timeframe) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    symbol.hash(&mut h);
    tf.hash(&mut h);
    h.finish()
}

/// Short label (e.g. `"1m"`, `"5m"`, `"1d"`) the thumbnail overlay
/// shows for `tf`.
///
/// Ticks down to lowercase `"1d"` for `D1` to match the minichart
/// conventions in TradingView / Bloomberg Launchpad; every other
/// timeframe falls through to the domain's canonical
/// [`Timeframe::display_name`] (which already lowercases the sub-daily
/// variants but uses uppercase `"1D"` / `"1W"` / `"1M"`).
pub fn label_for_tf(tf: Timeframe) -> String {
    match tf {
        Timeframe::D1 => "1d".to_string(),
        Timeframe::W1 => "1w".to_string(),
        Timeframe::MN1 => "1mo".to_string(),
        other => other.display_name().to_string(),
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot(closes: Vec<f32>, generation: u64, widget_key: u64) -> ThumbnailSnapshot {
        ThumbnailSnapshot {
            widget_key,
            closes: Arc::new(closes),
            y_min: 0.0,
            y_max: 1.0,
            color: [0.1, 0.7, 0.3, 1.0],
            generation,
            label: "1m".to_string(),
        }
    }

    #[test]
    fn label_for_empty_is_ellipsis() {
        assert_eq!(ThumbnailSnapshot::label_for_empty(), EMPTY_LABEL);
    }

    #[test]
    fn effective_label_for_empty_slice_is_ellipsis() {
        let snap = sample_snapshot(Vec::new(), 0, 0);
        assert_eq!(snap.effective_label(), EMPTY_LABEL);
    }

    #[test]
    fn effective_label_for_singleton_slice_is_ellipsis() {
        // A single sample cannot form a triangle strip — the shader
        // skips the draw and we fall back to the empty-state glyph so
        // the user still sees *something* while more data loads.
        let snap = sample_snapshot(vec![42.0], 7, 1);
        assert_eq!(snap.effective_label(), EMPTY_LABEL);
    }

    #[test]
    fn effective_label_for_populated_slice_uses_provided() {
        let snap = sample_snapshot(vec![1.0, 2.0, 3.0], 1, 2);
        assert_eq!(snap.effective_label(), "1m");
    }

    #[test]
    fn snapshot_equality_by_generation() {
        // Two snapshots that share a generation are treated as
        // "unchanged" by the per-widget upload check, regardless of
        // their close slices — this asserts the invariant the widget
        // relies on at its gate.
        let a = sample_snapshot(vec![1.0, 2.0, 3.0], 42, 0);
        let b = sample_snapshot(vec![9.0, 8.0, 7.0], 42, 0);

        let mut state = ThumbnailWidgetState::new();
        state.last_generation = a.generation;

        assert_eq!(state.last_generation, b.generation);
    }

    #[test]
    fn snapshot_change_bumps_last_generation() {
        let a = sample_snapshot(vec![1.0, 2.0, 3.0], 1, 0);
        let b = sample_snapshot(vec![1.0, 2.0, 3.0], 2, 0);

        let mut state = ThumbnailWidgetState::new();
        state.last_generation = a.generation;
        assert_ne!(state.last_generation, b.generation);
        state.last_generation = b.generation;
        assert_eq!(state.last_generation, b.generation);
    }

    #[test]
    fn default_state_never_matches_any_real_generation() {
        // The "never uploaded" sentinel must not collide with the
        // first real generation the app emits (which starts at 0).
        let state = ThumbnailWidgetState::default();
        assert_ne!(state.last_generation, 0);
    }

    #[test]
    fn thumbnail_cell_returns_non_panicking_element() {
        // Contract test: the helper must assemble a valid element
        // without panicking for both empty and populated snapshots.
        let _empty: Element<'_, u32> = thumbnail_cell(sample_snapshot(Vec::new(), 0, 0), 7u32);
        let _populated: Element<'_, u32> =
            thumbnail_cell(sample_snapshot(vec![1.0, 2.0, 3.0, 4.0], 1, 1), 9u32);
    }

    #[test]
    fn widget_key_is_stable_for_same_input() {
        let a = widget_key("AAPL", Timeframe::M5);
        let b = widget_key("AAPL", Timeframe::M5);
        assert_eq!(a, b);
    }

    #[test]
    fn widget_key_differs_between_symbols() {
        let a = widget_key("AAPL", Timeframe::M5);
        let b = widget_key("MSFT", Timeframe::M5);
        assert_ne!(a, b);
    }

    #[test]
    fn widget_key_differs_between_timeframes() {
        let a = widget_key("AAPL", Timeframe::M5);
        let b = widget_key("AAPL", Timeframe::D1);
        assert_ne!(a, b);
    }

    #[test]
    fn label_for_tf_covers_cycle_members() {
        assert_eq!(label_for_tf(Timeframe::M1), "1m");
        assert_eq!(label_for_tf(Timeframe::M5), "5m");
        assert_eq!(label_for_tf(Timeframe::D1), "1d");
    }
}
