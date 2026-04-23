//! Extended-hours policy enum for [`SessionChart`](super::SessionChart).
//!
//! Split out of `widget.rs` in the R3 refactor (arch-audit F2) to keep
//! the widget file focused on the scaffold widget itself. The type
//! lives here because the policy straddles two concerns — stream
//! filtering (equivalent to `midas_stream::Filtered<_, EhFilter>`) and
//! scene-layer config — and both consumers import it from the widget
//! module. Phase D will promote it to `midas-scene` once the legacy
//! chart path is retired.

/// Extended-hours rendering policy per the session-aware-charts ideal
/// design (`plan/session-aware-charts/00a-ideal-design.md` →
/// "Chart — composition root" / "Ideal behaviours → EhPolicy").
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum EhPolicy {
    /// Pre + RTH + Post candles all visible. Full chrome (bands,
    /// separators, holiday markers) enabled.
    #[default]
    ShowAll,
    /// Only RTH candles in the stream; extended-hours bars filtered
    /// upstream via [`midas_stream::Filtered<_, EhFilter>`]. Scene
    /// chrome is identical to `ShowAll` — the band + separator layers
    /// paint only sessions that have candles.
    HideExtended,
    /// Candles render unfiltered (so pre/post bars are still visible,
    /// session-tinted) but session bands + separators do NOT emit.
    /// Gives a "just the price" look with session context carried by
    /// the candle color alone.
    ShowBarsOnly,
}

impl EhPolicy {
    /// Cycle through the three states in the order shown to the user
    /// by the toggle chip: `ShowAll → HideExtended → ShowBarsOnly →
    /// ShowAll`.
    ///
    /// Three applications round-trip back to the start — exercised by
    /// the widget's unit test.
    pub const fn next(self) -> Self {
        match self {
            EhPolicy::ShowAll => EhPolicy::HideExtended,
            EhPolicy::HideExtended => EhPolicy::ShowBarsOnly,
            EhPolicy::ShowBarsOnly => EhPolicy::ShowAll,
        }
    }

    /// Short label for an on-screen chip/button.
    pub const fn short_label(self) -> &'static str {
        match self {
            EhPolicy::ShowAll => "EH",
            EhPolicy::HideExtended => "RTH",
            EhPolicy::ShowBarsOnly => "EH·bars",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_round_trips_after_three_applications() {
        assert_eq!(EhPolicy::default(), EhPolicy::ShowAll);
        assert_eq!(EhPolicy::ShowAll.next(), EhPolicy::HideExtended);
        assert_eq!(EhPolicy::HideExtended.next(), EhPolicy::ShowBarsOnly);
        assert_eq!(EhPolicy::ShowBarsOnly.next(), EhPolicy::ShowAll);
    }

    #[test]
    fn short_labels_are_stable() {
        assert_eq!(EhPolicy::ShowAll.short_label(), "EH");
        assert_eq!(EhPolicy::HideExtended.short_label(), "RTH");
        assert_eq!(EhPolicy::ShowBarsOnly.short_label(), "EH·bars");
    }
}
