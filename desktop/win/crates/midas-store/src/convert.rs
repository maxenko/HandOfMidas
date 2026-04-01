use midas_core::Timeframe;

/// Reconstruct a Timeframe from its `as_secs()` value.
///
/// Returns `None` for unrecognized values.
pub fn timeframe_from_secs(secs: u32) -> Option<Timeframe> {
    match secs {
        1 => Some(Timeframe::S1),
        5 => Some(Timeframe::S5),
        15 => Some(Timeframe::S15),
        30 => Some(Timeframe::S30),
        60 => Some(Timeframe::M1),
        300 => Some(Timeframe::M5),
        900 => Some(Timeframe::M15),
        1800 => Some(Timeframe::M30),
        3600 => Some(Timeframe::H1),
        14400 => Some(Timeframe::H4),
        86400 => Some(Timeframe::D1),
        604800 => Some(Timeframe::W1),
        2592000 => Some(Timeframe::MN1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timeframe_secs_roundtrip() {
        let all = [
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
        ];
        for tf in all {
            let secs = tf.as_secs();
            let restored = timeframe_from_secs(secs)
                .unwrap_or_else(|| panic!("failed to restore {tf:?} from {secs}"));
            assert_eq!(tf, restored);
        }
    }

    #[test]
    fn test_unknown_timeframe_returns_none() {
        assert!(timeframe_from_secs(42).is_none());
        assert!(timeframe_from_secs(0).is_none());
        assert!(timeframe_from_secs(u32::MAX).is_none());
    }
}
