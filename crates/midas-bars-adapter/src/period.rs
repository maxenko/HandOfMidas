//! Bidirectional map between the legacy `midas_broker_core::Timeframe`
//! enum and the session-aware `midas_calendar::BarPeriod`.
//!
//! Every `Timeframe` has a `BarPeriod` mapping. The reverse is partial:
//! `Session(Extended)`, `Session(Eth)`, `Calendar(Quarter)`, and
//! `Calendar(Year)` have no `Timeframe` counterpart and return
//! [`AdapterError::NoTimeframeMapping`].

use midas_broker_core::Timeframe;
use midas_calendar::{BarPeriod, CalendarSpan, ClockInterval, SessionSpan};

use crate::AdapterError;

/// Map a legacy [`Timeframe`] into a [`BarPeriod`].
///
/// Rules:
/// - All sub-daily `Timeframe` variants map to
///   `BarPeriod::Clock(ClockInterval::*)`.
/// - `Timeframe::D1` → `BarPeriod::Session(SessionSpan::Regular)` —
///   matches IB's `use_rth=true` convention on daily bars.
/// - `Timeframe::W1` → `BarPeriod::Calendar(CalendarSpan::Week)`.
/// - `Timeframe::MN1` → `BarPeriod::Calendar(CalendarSpan::Month)`.
#[inline]
pub fn timeframe_to_period(tf: Timeframe) -> BarPeriod {
    match tf {
        Timeframe::S1 => BarPeriod::Clock(ClockInterval::Seconds(1)),
        Timeframe::S5 => BarPeriod::Clock(ClockInterval::Seconds(5)),
        Timeframe::S15 => BarPeriod::Clock(ClockInterval::Seconds(15)),
        Timeframe::S30 => BarPeriod::Clock(ClockInterval::Seconds(30)),
        Timeframe::M1 => BarPeriod::Clock(ClockInterval::Minutes(1)),
        Timeframe::M5 => BarPeriod::Clock(ClockInterval::Minutes(5)),
        Timeframe::M15 => BarPeriod::Clock(ClockInterval::Minutes(15)),
        Timeframe::M30 => BarPeriod::Clock(ClockInterval::Minutes(30)),
        Timeframe::H1 => BarPeriod::Clock(ClockInterval::Hours(1)),
        Timeframe::H4 => BarPeriod::Clock(ClockInterval::Hours(4)),
        Timeframe::D1 => BarPeriod::Session(SessionSpan::Regular),
        Timeframe::W1 => BarPeriod::Calendar(CalendarSpan::Week),
        Timeframe::MN1 => BarPeriod::Calendar(CalendarSpan::Month),
    }
}

/// Inverse map. Returns [`AdapterError::NoTimeframeMapping`] for
/// `BarPeriod` variants with no legacy equivalent.
pub fn period_to_timeframe(p: BarPeriod) -> Result<Timeframe, AdapterError> {
    match p {
        BarPeriod::Clock(ClockInterval::Seconds(1)) => Ok(Timeframe::S1),
        BarPeriod::Clock(ClockInterval::Seconds(5)) => Ok(Timeframe::S5),
        BarPeriod::Clock(ClockInterval::Seconds(15)) => Ok(Timeframe::S15),
        BarPeriod::Clock(ClockInterval::Seconds(30)) => Ok(Timeframe::S30),
        BarPeriod::Clock(ClockInterval::Minutes(1)) => Ok(Timeframe::M1),
        BarPeriod::Clock(ClockInterval::Minutes(5)) => Ok(Timeframe::M5),
        BarPeriod::Clock(ClockInterval::Minutes(15)) => Ok(Timeframe::M15),
        BarPeriod::Clock(ClockInterval::Minutes(30)) => Ok(Timeframe::M30),
        BarPeriod::Clock(ClockInterval::Hours(1)) => Ok(Timeframe::H1),
        BarPeriod::Clock(ClockInterval::Hours(4)) => Ok(Timeframe::H4),
        BarPeriod::Session(SessionSpan::Regular) => Ok(Timeframe::D1),
        BarPeriod::Calendar(CalendarSpan::Week) => Ok(Timeframe::W1),
        BarPeriod::Calendar(CalendarSpan::Month) => Ok(Timeframe::MN1),
        // Extended/Eth and Quarter/Year are not expressible in Timeframe.
        other => Err(AdapterError::NoTimeframeMapping(other)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeframe_to_period_clock_family() {
        let cases = [
            (Timeframe::S1, ClockInterval::Seconds(1)),
            (Timeframe::S5, ClockInterval::Seconds(5)),
            (Timeframe::S15, ClockInterval::Seconds(15)),
            (Timeframe::S30, ClockInterval::Seconds(30)),
            (Timeframe::M1, ClockInterval::Minutes(1)),
            (Timeframe::M5, ClockInterval::Minutes(5)),
            (Timeframe::M15, ClockInterval::Minutes(15)),
            (Timeframe::M30, ClockInterval::Minutes(30)),
            (Timeframe::H1, ClockInterval::Hours(1)),
            (Timeframe::H4, ClockInterval::Hours(4)),
        ];
        for (tf, ci) in cases {
            assert_eq!(timeframe_to_period(tf), BarPeriod::Clock(ci));
        }
    }

    #[test]
    fn timeframe_d1_maps_to_session_regular() {
        assert_eq!(
            timeframe_to_period(Timeframe::D1),
            BarPeriod::Session(SessionSpan::Regular)
        );
    }

    #[test]
    fn timeframe_w1_maps_to_calendar_week() {
        assert_eq!(
            timeframe_to_period(Timeframe::W1),
            BarPeriod::Calendar(CalendarSpan::Week)
        );
    }

    #[test]
    fn timeframe_mn1_maps_to_calendar_month() {
        assert_eq!(
            timeframe_to_period(Timeframe::MN1),
            BarPeriod::Calendar(CalendarSpan::Month)
        );
    }

    #[test]
    fn period_to_timeframe_clock_roundtrip() {
        // Every clock-interval Timeframe round-trips.
        for tf in [
            Timeframe::S1,
            Timeframe::S5,
            Timeframe::S15,
            Timeframe::S30,
            Timeframe::M1,
            Timeframe::M5,
            Timeframe::M15,
            Timeframe::M30,
            Timeframe::H1,
            Timeframe::H4,
            Timeframe::D1,
            Timeframe::W1,
            Timeframe::MN1,
        ] {
            let p = timeframe_to_period(tf);
            let back = period_to_timeframe(p).expect("lossless round-trip");
            assert_eq!(back, tf);
        }
    }

    #[test]
    fn period_to_timeframe_session_extended_no_mapping() {
        let p = BarPeriod::Session(SessionSpan::Extended);
        let err = period_to_timeframe(p).unwrap_err();
        match err {
            AdapterError::NoTimeframeMapping(bp) => assert_eq!(bp, p),
            other => panic!("expected NoTimeframeMapping, got {other:?}"),
        }
    }

    #[test]
    fn period_to_timeframe_session_eth_no_mapping() {
        let p = BarPeriod::Session(SessionSpan::Eth);
        let err = period_to_timeframe(p).unwrap_err();
        assert!(matches!(err, AdapterError::NoTimeframeMapping(bp) if bp == p));
    }

    #[test]
    fn period_to_timeframe_calendar_quarter_no_mapping() {
        let p = BarPeriod::Calendar(CalendarSpan::Quarter);
        let err = period_to_timeframe(p).unwrap_err();
        assert!(matches!(err, AdapterError::NoTimeframeMapping(bp) if bp == p));
    }

    #[test]
    fn period_to_timeframe_calendar_year_no_mapping() {
        let p = BarPeriod::Calendar(CalendarSpan::Year);
        let err = period_to_timeframe(p).unwrap_err();
        assert!(matches!(err, AdapterError::NoTimeframeMapping(bp) if bp == p));
    }

    #[test]
    fn period_to_timeframe_unusual_clock_no_mapping() {
        // Seconds(2) is representable as BarPeriod but not as Timeframe.
        let p = BarPeriod::Clock(ClockInterval::Seconds(2));
        let err = period_to_timeframe(p).unwrap_err();
        assert!(matches!(err, AdapterError::NoTimeframeMapping(_)));
    }
}
