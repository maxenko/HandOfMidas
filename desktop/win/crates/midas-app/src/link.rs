//! Chart linking UI helpers and pure propagation logic.
//!
//! `LinkColor` and `LinkMode` enums live in `midas-core`. This module
//! provides UI-specific helpers (color rendering) and the pure
//! target-matching function used by propagation logic.

use iced::window;
use midas_core::{ChartId, LinkColor, LinkMode, WatchlistId};

// ── Picker target ──────────────────────────────────────────────────

/// Identifies which panel's link picker is open — docked, floating, or watchlist.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum PickerTarget {
    Docked(ChartId),
    Floating(window::Id),
    Watchlist(WatchlistId),
}

// ── Link dimension ──────────────────────────────────────────────────

/// Which link dimension (symbol or timeframe) is being configured.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum LinkDimension {
    Symbol,
    Timeframe,
}

// ── Color rendering (free functions) ────────────────────────────────

/// RGBA color for a link color (sRGB space).
pub const fn link_color_rgba(c: LinkColor) -> [f32; 4] {
    match c {
        LinkColor::Blue   => [0.20, 0.40, 0.90, 1.0],
        LinkColor::Red    => [0.90, 0.15, 0.15, 1.0],
        LinkColor::Orange => [0.95, 0.55, 0.05, 1.0],
        LinkColor::Green  => [0.15, 0.75, 0.25, 1.0],
        LinkColor::Purple => [0.55, 0.15, 0.75, 1.0],
        LinkColor::Violet => [0.70, 0.35, 0.85, 1.0],
        LinkColor::Teal   => [0.15, 0.75, 0.80, 1.0],
        LinkColor::Brown  => [0.55, 0.35, 0.15, 1.0],
    }
}

/// RGBA color for the link button indicator.
/// Unlinked = gray, ListenAll = yellow/gold, Color = that color.
pub fn link_mode_indicator_rgba(mode: LinkMode) -> [f32; 4] {
    match mode {
        LinkMode::Unlinked => [0.40, 0.40, 0.40, 1.0],
        LinkMode::ListenAll => [0.95, 0.85, 0.10, 1.0],
        LinkMode::Color(c) => link_color_rgba(c),
    }
}

// ── Propagation target matching ─────────────────────────────────────

/// Given a source's link mode, find which panel keys should receive the
/// propagated change. Returns an empty vec if the source doesn't broadcast.
///
/// `panels` is an iterator of (key, link_mode) for all candidate panels
/// (excluding the source). This works for both symbol and timeframe linking,
/// and for any key type (`ChartId`, `window::Id`, etc.).
pub fn find_link_targets<K, I>(source_link: LinkMode, panels: I) -> Vec<K>
where
    I: IntoIterator<Item = (K, LinkMode)>,
{
    let source_color = match source_link {
        LinkMode::Color(c) => c,
        _ => return Vec::new(),
    };

    panels
        .into_iter()
        .filter(|(_, link)| match link {
            LinkMode::Color(c) => *c == source_color,
            LinkMode::ListenAll => true,
            LinkMode::Unlinked => false,
        })
        .map(|(id, _)| id)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_targets_same_color() {
        let targets = find_link_targets(
            LinkMode::Color(LinkColor::Blue),
            vec![
                (ChartId::new(1), LinkMode::Color(LinkColor::Blue)),
                (ChartId::new(2), LinkMode::Color(LinkColor::Red)),
                (ChartId::new(3), LinkMode::Color(LinkColor::Blue)),
            ],
        );
        assert_eq!(targets, vec![ChartId::new(1), ChartId::new(3)]);
    }

    #[test]
    fn find_targets_listen_all_receives() {
        let targets = find_link_targets(
            LinkMode::Color(LinkColor::Red),
            vec![
                (ChartId::new(1), LinkMode::ListenAll),
                (ChartId::new(2), LinkMode::Color(LinkColor::Blue)),
            ],
        );
        assert_eq!(targets, vec![ChartId::new(1)]);
    }

    #[test]
    fn listen_all_does_not_broadcast() {
        let targets = find_link_targets(
            LinkMode::ListenAll,
            vec![
                (ChartId::new(1), LinkMode::Color(LinkColor::Blue)),
                (ChartId::new(2), LinkMode::ListenAll),
            ],
        );
        assert!(targets.is_empty());
    }

    #[test]
    fn unlinked_does_not_broadcast() {
        let targets = find_link_targets(
            LinkMode::Unlinked,
            vec![
                (ChartId::new(1), LinkMode::Color(LinkColor::Blue)),
            ],
        );
        assert!(targets.is_empty());
    }

    #[test]
    fn no_matching_panels_returns_empty() {
        let targets = find_link_targets(
            LinkMode::Color(LinkColor::Green),
            vec![
                (ChartId::new(1), LinkMode::Color(LinkColor::Blue)),
                (ChartId::new(2), LinkMode::Unlinked),
            ],
        );
        assert!(targets.is_empty());
    }

    #[test]
    fn indicator_rgba_unlinked_is_gray() {
        let c = link_mode_indicator_rgba(LinkMode::Unlinked);
        assert_eq!(c, [0.40, 0.40, 0.40, 1.0]);
    }

    #[test]
    fn indicator_rgba_listen_all_is_yellow() {
        let c = link_mode_indicator_rgba(LinkMode::ListenAll);
        assert_eq!(c, [0.95, 0.85, 0.10, 1.0]);
    }

    #[test]
    fn indicator_rgba_color_delegates() {
        let c = link_mode_indicator_rgba(LinkMode::Color(LinkColor::Blue));
        assert_eq!(c, link_color_rgba(LinkColor::Blue));
    }
}
