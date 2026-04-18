//! Recent Instruments tab inside the [`super::AccountPanel`].
//!
//! Data lives on [`crate::app::MidasApp::recent_symbols`] and is
//! projected into display rows inside
//! [`crate::app::views::view_account_recents_tab`]. Clicking a row
//! emits [`super::AccountMsg::RecentClicked`]; the handler re-selects
//! the symbol on the focused chart via the same code path a manual
//! entry uses (`propagate_symbol_change`).
//!
//! Design notes
//! ============
//!
//! - The MRU is bounded at `crate::app::MAX_RECENTS`; the grid body
//!   never grows unbounded.
//! - Timestamps (`last_seen`) are session-only. Entries loaded from
//!   config render `"—"` until the user switches to the symbol again
//!   and the entry is re-pushed with a fresh `Instant`.
//! - Column widths live on [`RecentsTab::grid_state`] and are
//!   runtime-only (not persisted) — same convention as History and
//!   Positions per plan Decision 4.
//! - `scrollable::Id` is keyed by `AccountPanelId` so multiple Account
//!   panes don't share scroll state.

use std::time::{Duration, Instant};

use midas_grid::{ColumnId, GridState};

// ── Column IDs ───────────────────────────────────────────────────────

/// Ticker symbol (primary column — takes remaining width).
pub const COL_RECENTS_TICKER: ColumnId = ColumnId("recents_ticker");
/// Elapsed-since-last-seen text (fixed-width right column).
pub const COL_RECENTS_LAST_SEEN: ColumnId = ColumnId("recents_last_seen");

/// Default widths. Ticker takes the bulk of the row; Last Seen fits
/// the widest realistic value ("999d ago" ≈ 70 px at size 12).
pub fn default_widths() -> Vec<(ColumnId, f32)> {
    vec![(COL_RECENTS_TICKER, 220.0), (COL_RECENTS_LAST_SEEN, 100.0)]
}

pub fn column_ids() -> Vec<ColumnId> {
    vec![COL_RECENTS_TICKER, COL_RECENTS_LAST_SEEN]
}

/// View-model for the Recents tab.
///
/// Carries only `grid_state` for column widths; the row data lives on
/// [`crate::app::MidasApp::recent_symbols`] and is projected at render
/// time in [`crate::app::views::view_account_recents_tab`].
#[derive(Debug, Clone)]
pub struct RecentsTab {
    /// Column widths (runtime-only; not persisted to `AppConfig`).
    pub grid_state: GridState,
}

impl Default for RecentsTab {
    fn default() -> Self {
        Self::new()
    }
}

impl RecentsTab {
    pub fn new() -> Self {
        use std::collections::HashMap;
        let ids = column_ids();
        let widths: HashMap<ColumnId, f32> = default_widths().into_iter().collect();
        Self {
            grid_state: GridState::new(ids, widths),
        }
    }
}

/// Format a `last_seen` instant as a short "N unit ago" suffix.
///
/// Returns `"—"` when the timestamp is `None` (entry hydrated from
/// persisted config). Otherwise picks the largest round unit that
/// produces a non-zero count:
///
/// - `< 1 min` → `"just now"`
/// - minutes → `"3m ago"`
/// - hours → `"12h ago"`
/// - days → `"2d ago"`
pub(crate) fn format_elapsed(last_seen: Option<Instant>, now: Instant) -> String {
    let Some(ts) = last_seen else {
        return "—".to_string();
    };
    // `saturating_duration_since` guards against clock weirdness
    // (e.g. a fixture that claims a future `last_seen`).
    let elapsed: Duration = now.saturating_duration_since(ts);
    let secs = elapsed.as_secs();
    if secs < 60 {
        return "just now".to_string();
    }
    let minutes = secs / 60;
    if minutes < 60 {
        return format!("{minutes}m ago");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("{hours}h ago");
    }
    let days = hours / 24;
    format!("{days}d ago")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Construct a "late" base `Instant` by stacking adds on top of
    /// `Instant::now()`. Subtracting up to a few days from this base
    /// is always safe — a bare `Instant::now() - Duration::from_days(2)`
    /// underflows on a freshly-booted host where the monotonic clock
    /// is newer than two days.
    fn late_now() -> Instant {
        // 30 days of headroom — larger than any unit this module
        // formats — and `Instant::checked_add` keeps the helper sane
        // on exotic platforms where future times might saturate.
        Instant::now()
            .checked_add(Duration::from_secs(30 * 24 * 60 * 60))
            .expect("Instant + 30d must fit")
    }

    #[test]
    fn format_elapsed_none_is_em_dash() {
        assert_eq!(format_elapsed(None, late_now()), "—");
    }

    #[test]
    fn format_elapsed_under_a_minute_is_just_now() {
        let now = late_now();
        let then = now - Duration::from_secs(30);
        assert_eq!(format_elapsed(Some(then), now), "just now");
    }

    #[test]
    fn format_elapsed_minutes() {
        let now = late_now();
        let then = now - Duration::from_secs(3 * 60);
        assert_eq!(format_elapsed(Some(then), now), "3m ago");
    }

    #[test]
    fn format_elapsed_hours() {
        let now = late_now();
        let then = now - Duration::from_secs(12 * 60 * 60);
        assert_eq!(format_elapsed(Some(then), now), "12h ago");
    }

    #[test]
    fn format_elapsed_days() {
        let now = late_now();
        let then = now - Duration::from_secs(2 * 24 * 60 * 60);
        assert_eq!(format_elapsed(Some(then), now), "2d ago");
    }

    #[test]
    fn format_elapsed_future_timestamp_is_just_now() {
        // Guard: a bad fixture could carry a future `last_seen`.
        // `saturating_duration_since` must clamp to zero so the UI
        // renders "just now" instead of panicking on subtraction.
        let now = late_now();
        let future = now + Duration::from_secs(30);
        assert_eq!(format_elapsed(Some(future), now), "just now");
    }
}
