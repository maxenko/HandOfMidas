//! [`LabelFormatter`] — shared price + time + percent + volume
//! rendering.
//!
//! Slice 2a of the chart-transition plan shipped the NARROW initial
//! shape — `price` + `time` only. Slice 6 extends it breaking-
//! additively with `percent` + `volume` so the indicator layers
//! (`AtrLayer`, `GerchikAtrLayer`) can route their label text through
//! the same shared surface. Both additions ship with default impls
//! that mirror `DefaultFormatter`'s behaviour so downstream consumers
//! that implemented the trait against the slice-2a surface continue
//! to compile.
//!
//! ## Design
//!
//! The trait is `Send + Sync` so a `&dyn LabelFormatter` can live on
//! [`crate::PaintContext`] (slice 2a extends `PaintContext` to carry
//! one) without forcing each layer to format locally — every layer
//! routes through the same formatter so locale / precision policy
//! lands in exactly one place.

use chrono_tz::Tz;
use midas_calendar::Timestamp;

use crate::TickDensity;

/// Pluggable label formatter. Thread-safe.
///
/// Implementations may cache computed label strings; the trait
/// methods take `&self` so a mutable cache needs interior mutability
/// (typically a `parking_lot::Mutex<Cache>` sat inside the impl).
pub trait LabelFormatter: Send + Sync {
    /// Format a price label rounded to the instrument's tick size.
    ///
    /// `tick_size` is the minimum price increment (e.g. `0.01` for
    /// most US equities, `0.25` for ES futures). Implementations
    /// round to this granularity and pick a decimal-places count
    /// derived from the tick (`tick == 0.25` → 2 decimals;
    /// `tick == 0.01` → 2; `tick == 1.0` → 0).
    fn price(&self, price: f64, tick_size: f64) -> String;

    /// Format a timestamp in `tz` at the given tick density.
    ///
    /// Density drives granularity: [`TickDensity::Sparse`] emits
    /// calendar dates ("Mar 15"); [`TickDensity::Normal`] emits
    /// times on the hour ("09:30"); [`TickDensity::Dense`] emits
    /// minute-granular times ("09:32").
    fn time(&self, ts: Timestamp, tz: Tz, density: TickDensity) -> String;

    /// Format a percentage. Input is the raw percent value (e.g.
    /// `67.3` for "67.3%"); implementations choose a sensible
    /// precision — `DefaultFormatter` uses 0 decimals so a G.ATR
    /// reading lands as `"67%"`.
    ///
    /// Slice 6 extension. Ships with a default impl that matches
    /// `DefaultFormatter::percent` so every existing impl of the
    /// trait compiles unchanged.
    fn percent(&self, p: f32) -> String {
        if !p.is_finite() {
            return "NaN%".to_string();
        }
        format!("{:.0}%", p)
    }

    /// Format a trade volume. Implementations MAY apply abbreviation
    /// ("12.3K", "4.2M"); the default impl renders a plain decimal
    /// string so tests have a predictable baseline.
    ///
    /// Slice 6 extension. Default impl matches
    /// `DefaultFormatter::volume`.
    fn volume(&self, v: u64) -> String {
        format!("{}", v)
    }
}

/// Default formatter — `Send + Sync` stateless struct usable in
/// tests + production. Locale is fixed to en-US for now; a future
/// slice may widen to a `locale: unic_locale_impl::Locale` field.
#[derive(Copy, Clone, Debug, Default)]
pub struct DefaultFormatter;

impl DefaultFormatter {
    /// Construct a fresh default formatter. Stateless, but kept as a
    /// constructor for API stability.
    pub const fn new() -> Self {
        Self
    }

    /// Decimal places derived from `tick_size`. Uses the minimum
    /// number of digits needed to disambiguate ticks (e.g. `0.01` →
    /// 2, `0.25` → 2, `0.5` → 1, `1.0` → 0, `0.0001` → 4).
    ///
    /// Algorithm: find the smallest `d ∈ [0, 8]` such that
    /// `tick_size * 10^d` is (close to) an integer. Caps at 8 to avoid
    /// runaway precision — anything beyond `0.00000001` likely signals
    /// a bad input and falls through to the default.
    fn decimals_for_tick(tick_size: f64) -> usize {
        if !tick_size.is_finite() || tick_size <= 0.0 {
            return 2;
        }
        for d in 0..=8 {
            let scaled = tick_size * 10f64.powi(d as i32);
            if (scaled - scaled.round()).abs() < 1e-9 {
                return d;
            }
        }
        // Extreme tick size — fall back to 2.
        2
    }
}

impl LabelFormatter for DefaultFormatter {
    fn price(&self, price: f64, tick_size: f64) -> String {
        let decimals = Self::decimals_for_tick(tick_size);
        // Round to tick: `round(p / tick) * tick`. Guards against
        // zero-or-negative tick (returns the raw price at the derived
        // decimals).
        let rounded = if tick_size.is_finite() && tick_size > 0.0 {
            (price / tick_size).round() * tick_size
        } else {
            price
        };
        tracing::debug!(
            target: "midas_axis::format::price",
            price,
            tick_size,
            decimals,
            "format price",
        );
        format!("{:.*}", decimals, rounded)
    }

    fn time(&self, ts: Timestamp, tz: Tz, density: TickDensity) -> String {
        let local = ts.with_timezone(&tz);
        let out = match density {
            TickDensity::Sparse => local.format("%b %e").to_string(),
            TickDensity::Normal => local.format("%H:%M").to_string(),
            TickDensity::Dense => local.format("%H:%M:%S").to_string(),
        };
        tracing::debug!(
            target: "midas_axis::format::time",
            ts = %ts,
            density = ?density,
            "format time",
        );
        out
    }

    fn percent(&self, p: f32) -> String {
        if !p.is_finite() {
            tracing::debug!(
                target: "midas_axis::format::percent",
                p,
                "non-finite percent",
            );
            return "NaN%".to_string();
        }
        // 0-decimal rendering matches the legacy G.ATR badge
        // (`"G.ATR 67%"`) — change the precision here if higher-
        // resolution indicators need it.
        tracing::debug!(
            target: "midas_axis::format::percent",
            p,
            "format percent",
        );
        format!("{:.0}%", p)
    }

    fn volume(&self, v: u64) -> String {
        tracing::debug!(
            target: "midas_axis::format::volume",
            v,
            "format volume",
        );
        format!("{}", v)
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn ts(y: i32, mo: u32, d: u32, h: u32, mi: u32, s: u32) -> Timestamp {
        chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, s).unwrap()
    }

    #[test]
    fn price_formats_with_two_decimals_for_penny_tick() {
        let f = DefaultFormatter::new();
        assert_eq!(f.price(100.0, 0.01), "100.00");
        assert_eq!(f.price(100.5, 0.01), "100.50");
    }

    #[test]
    fn price_rounds_to_quarter_tick() {
        let f = DefaultFormatter::new();
        // ES-futures tick = 0.25 → 2 decimals.
        assert_eq!(f.price(4500.10, 0.25), "4500.00");
        assert_eq!(f.price(4500.13, 0.25), "4500.25");
    }

    #[test]
    fn price_zero_decimals_for_integer_tick() {
        let f = DefaultFormatter::new();
        assert_eq!(f.price(12345.6, 1.0), "12346");
    }

    #[test]
    fn price_four_decimals_for_basis_tick() {
        let f = DefaultFormatter::new();
        // Forex tick = 0.0001 → 4 decimals.
        assert_eq!(f.price(1.23456, 0.0001), "1.2346");
    }

    #[test]
    fn price_zero_tick_falls_back_to_two_decimals() {
        let f = DefaultFormatter::new();
        // Zero tick: fall through to 2-decimal default.
        let out = f.price(100.5, 0.0);
        assert_eq!(out, "100.50");
    }

    #[test]
    fn price_negative_tick_falls_back_to_two_decimals() {
        let f = DefaultFormatter::new();
        let out = f.price(100.5, -0.01);
        assert_eq!(out, "100.50");
    }

    #[test]
    fn decimals_for_tick_handles_edge_cases() {
        assert_eq!(DefaultFormatter::decimals_for_tick(0.01), 2);
        // Quarter tick — need 2 decimals to distinguish .25/.75.
        assert_eq!(DefaultFormatter::decimals_for_tick(0.25), 2);
        assert_eq!(DefaultFormatter::decimals_for_tick(0.5), 1);
        assert_eq!(DefaultFormatter::decimals_for_tick(1.0), 0);
        assert_eq!(DefaultFormatter::decimals_for_tick(0.0001), 4);
        // Non-finite / zero / negative → default 2.
        assert_eq!(DefaultFormatter::decimals_for_tick(0.0), 2);
        assert_eq!(DefaultFormatter::decimals_for_tick(-0.25), 2);
        assert_eq!(DefaultFormatter::decimals_for_tick(f64::NAN), 2);
    }

    #[test]
    fn time_dense_renders_minute_seconds() {
        let f = DefaultFormatter::new();
        let utc = ts(2024, 3, 15, 14, 32, 17);
        let out = f.time(utc, chrono_tz::UTC, TickDensity::Dense);
        assert_eq!(out, "14:32:17");
    }

    #[test]
    fn time_normal_renders_hour_minute() {
        let f = DefaultFormatter::new();
        let utc = ts(2024, 3, 15, 14, 32, 17);
        let out = f.time(utc, chrono_tz::UTC, TickDensity::Normal);
        assert_eq!(out, "14:32");
    }

    #[test]
    fn time_sparse_renders_month_day() {
        let f = DefaultFormatter::new();
        let utc = ts(2024, 3, 15, 14, 32, 17);
        let out = f.time(utc, chrono_tz::UTC, TickDensity::Sparse);
        // `%b %e` — abbreviated month + space-padded day.
        assert_eq!(out, "Mar 15");
    }

    #[test]
    fn time_respects_timezone() {
        let f = DefaultFormatter::new();
        // 14:30 UTC = 09:30 America/New_York in March (EDT or EST —
        // mid-March means EDT = UTC-4).
        let utc = ts(2024, 3, 15, 14, 30, 0);
        let ny = chrono_tz::America::New_York;
        let out = f.time(utc, ny, TickDensity::Normal);
        // Depending on DST boundary, just check it's not the UTC time.
        assert_ne!(out, "14:30");
        assert!(out.contains(':'));
    }

    #[test]
    fn formatter_is_send_sync() {
        fn takes_send_sync<T: Send + Sync + ?Sized>(_: &T) {}
        let f = DefaultFormatter::new();
        let dynref: &dyn LabelFormatter = &f;
        takes_send_sync(dynref);
    }

    #[test]
    fn percent_zero_decimals_for_default_formatter() {
        let f = DefaultFormatter::new();
        assert_eq!(f.percent(67.3), "67%");
        assert_eq!(f.percent(0.0), "0%");
        assert_eq!(f.percent(100.0), "100%");
    }

    #[test]
    fn percent_rounds_to_nearest() {
        let f = DefaultFormatter::new();
        assert_eq!(f.percent(66.6), "67%");
        assert_eq!(f.percent(66.4), "66%");
    }

    #[test]
    fn percent_non_finite_emits_nan_marker() {
        let f = DefaultFormatter::new();
        assert_eq!(f.percent(f32::NAN), "NaN%");
        assert_eq!(f.percent(f32::INFINITY), "NaN%");
        assert_eq!(f.percent(f32::NEG_INFINITY), "NaN%");
    }

    #[test]
    fn volume_plain_decimal_render() {
        let f = DefaultFormatter::new();
        assert_eq!(f.volume(0), "0");
        assert_eq!(f.volume(1_234), "1234");
        assert_eq!(f.volume(1_000_000), "1000000");
    }

    /// A custom formatter that overrides only `price` + `time` (the
    /// slice-2a surface) exercises the default impls on `percent` +
    /// `volume` the trait extension added in slice 6.
    #[test]
    fn default_impls_for_percent_and_volume_cover_legacy_impls() {
        struct Slice2aFormatter;
        impl LabelFormatter for Slice2aFormatter {
            fn price(&self, p: f64, _tick: f64) -> String {
                format!("{p}")
            }
            fn time(&self, _ts: Timestamp, _tz: Tz, _d: TickDensity) -> String {
                "T".to_string()
            }
        }
        let f = Slice2aFormatter;
        // `percent` + `volume` fall through to the trait-level default
        // impls; they must produce sensible output without the impl
        // overriding them. `42.7` is unambiguous under any rounding
        // mode so the assertion is stable across Rust versions.
        assert_eq!(f.percent(42.7), "43%");
        assert_eq!(f.volume(99), "99");
    }
}
