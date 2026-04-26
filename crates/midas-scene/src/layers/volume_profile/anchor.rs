//! Render-time `VolumeProfileAnchor` enum for the new chart stack.
//!
//! Mirrors the persisted [`midas_core::VolumeProfileAnchor`] variant
//! set, with no serde derives — this copy is used purely as a
//! render-time enum by [`super::VolumeProfileLayer`]. The duplicate
//! exists because Architecture Rule 9 forbids the root crate
//! `midas-scene` depending on the desktop crate `midas-core`. The
//! `From`/`Into` bridge between the two enums lives in `midas-app`,
//! the only crate that touches both copies.
//!
//! See `plan/volume-profile-anchored/00-index.md` (D5, D9) for the
//! design rationale.
//!
//! # Why duplicate instead of a shared root crate?
//!
//! - Adding a new tiny shared crate for one 6-variant enum is more
//!   maintenance overhead than two ~12-line definitions.
//! - The two consumers (legacy `midas-chart` and new
//!   `midas-scene::VolumeProfileLayer`) live in different workspaces;
//!   the persisted form (`midas-core`) is desktop-only by design.
//! - Render-time and persisted forms can evolve independently — the
//!   render copy may grow `min_period_days()`-style helpers that
//!   would feel out of place on a serde-tagged enum.

/// Anchor mode for the [`super::VolumeProfileLayer`].
///
/// Mirrors [`midas_core::VolumeProfileAnchor`] one-for-one. Marked
/// `#[non_exhaustive]` so future variants can land without breaking
/// downstream `match` arms — the bridge in `midas-app` adds an
/// explicit `_ => Unknown` fallback for forward compatibility.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum VolumeProfileAnchor {
    /// Single histogram across the entire visible viewport (legacy
    /// behaviour).
    #[default]
    Viewport,
    /// One histogram per calendar day.
    Daily,
    /// One histogram per ISO week (Mon-start).
    Weekly,
    /// One histogram per calendar month.
    Monthly,
    /// One histogram per calendar year.
    Yearly,
    /// Forward-compat sink: render code treats `Unknown` exactly like
    /// `Viewport` (single profile, no per-period split).
    Unknown,
}

impl VolumeProfileAnchor {
    /// Minimum bar-period (in days) for which this anchor produces
    /// useful per-period output. Used by Slice 3's
    /// `period_blocks_anchor` to gate the D12 "anchor too coarse"
    /// silent fallback. `Viewport`/`Unknown` return `0` so the gate
    /// never fires for them.
    pub const fn min_period_days(self) -> u32 {
        match self {
            Self::Viewport | Self::Unknown => 0,
            Self::Daily => 1,
            Self::Weekly => 7,
            Self::Monthly => 30,
            Self::Yearly => 365,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_viewport() {
        assert_eq!(
            VolumeProfileAnchor::default(),
            VolumeProfileAnchor::Viewport
        );
    }

    #[test]
    fn min_period_days_matches_calendar_units() {
        assert_eq!(VolumeProfileAnchor::Viewport.min_period_days(), 0);
        assert_eq!(VolumeProfileAnchor::Unknown.min_period_days(), 0);
        assert_eq!(VolumeProfileAnchor::Daily.min_period_days(), 1);
        assert_eq!(VolumeProfileAnchor::Weekly.min_period_days(), 7);
        assert_eq!(VolumeProfileAnchor::Monthly.min_period_days(), 30);
        assert_eq!(VolumeProfileAnchor::Yearly.min_period_days(), 365);
    }
}
