//! Connection-lifecycle and quote-coalescing types.
//!
//! * [`ConnectionState`] — the state machine observed on the router's
//!   `connection_state` watch (M-23). `Ready` is the readiness gate
//!   used by order placement: connected AND all farms up AND
//!   `nextValidId` received.
//! * [`Quote`] — a symbol's last coalesced bid/ask/last. The router
//!   publishes these on a per-symbol `watch::Sender<Quote>` so
//!   watchlist cells can render without subscribing to the full tick
//!   stream.
//! * [`IbDuration`] — IB's durational language (M-2). Exposed on the
//!   `historical_bars` / `historical_stream` API so callers don't have
//!   to hand-format the strings.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::Duration;

/// Coalesced bid/ask/last for a single symbol.
///
/// `None` fields mean "not yet observed". `ts` is always the timestamp
/// of the most recent tick that updated any field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Quote {
    /// Best bid.
    pub bid: Option<f64>,
    /// Best ask.
    pub ask: Option<f64>,
    /// Last trade price.
    pub last: Option<f64>,
    /// Timestamp of the latest update.
    pub ts: DateTime<Utc>,
}

impl Default for Quote {
    fn default() -> Self {
        Self {
            bid: None,
            ask: None,
            last: None,
            // Epoch is the conventional "never updated" sentinel —
            // DateTime<Utc> has no Default impl.
            ts: DateTime::<Utc>::from_timestamp(0, 0).expect("epoch is a valid timestamp"),
        }
    }
}

/// Client-broker connection state (M-23).
///
/// `Ready` is the "safe to send orders" state; order-placing code
/// should wait on this via the router's `connection_state` watch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionState {
    /// No TCP connection, no pending attempt.
    Disconnected,
    /// TCP connecting; handshake incomplete.
    Connecting,
    /// Connected but not yet ready — waiting on farm-up /
    /// `nextValidId`.
    Connected {
        /// IB server wire version.
        server_version: i32,
    },
    /// Connected AND all farms up AND `nextValidId` received.
    Ready,
    /// Connection dropped; retrying.
    Reconnecting {
        /// Retry attempt counter (1-based).
        attempt: u32,
    },
}

/// IB bar-request duration (M-2).
///
/// IB's `historical_data` request takes a string like `"60 S"` or
/// `"1 D"`. This enum is the typed form; [`to_ib_string`] renders the
/// wire token.
///
/// [`to_ib_string`]: IbDuration::to_ib_string
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IbDuration {
    /// N seconds.
    Seconds(u32),
    /// N calendar days.
    Days(u32),
    /// N calendar weeks.
    Weeks(u32),
    /// N calendar months.
    Months(u32),
    /// N calendar years.
    Years(u32),
}

impl IbDuration {
    /// Render into IB's wire token (e.g. `"60 S"`, `"3 D"`).
    pub fn to_ib_string(&self) -> String {
        match self {
            IbDuration::Seconds(n) => format!("{n} S"),
            IbDuration::Days(n) => format!("{n} D"),
            IbDuration::Weeks(n) => format!("{n} W"),
            IbDuration::Months(n) => format!("{n} M"),
            IbDuration::Years(n) => format!("{n} Y"),
        }
    }

    /// Round a wall-clock lookback into the coarsest IB duration that
    /// covers it.
    ///
    /// Mapping:
    ///
    /// | Input span             | Output              |
    /// |------------------------|---------------------|
    /// | < 60 days              | `Seconds(ceil(d.as_secs()))` capped at u32::MAX |
    /// | < ~1 year              | `Days(ceil(d / 1 day))` |
    /// | < ~4 years             | `Weeks(ceil(d / 1 week))` |
    /// | < ~40 years            | `Months(ceil(d / 30 day))` |
    /// | otherwise              | `Years(ceil(d / 365 day))` |
    ///
    /// "Coarser than necessary" is never wrong here — IB interprets the
    /// duration as an upper bound on how much history to return, and
    /// the caller always knows the real window they wanted.
    pub fn from_lookback(d: Duration) -> Self {
        const DAY: u64 = 86_400;
        const WEEK: u64 = 7 * DAY;
        const MONTH: u64 = 30 * DAY;
        const YEAR: u64 = 365 * DAY;
        let secs = d.as_secs();
        if secs < 60 * DAY {
            Self::Seconds(secs.min(u32::MAX as u64) as u32)
        } else if secs < YEAR {
            Self::Days(ceil_div(secs, DAY) as u32)
        } else if secs < 4 * YEAR {
            Self::Weeks(ceil_div(secs, WEEK) as u32)
        } else if secs < 40 * YEAR {
            Self::Months(ceil_div(secs, MONTH) as u32)
        } else {
            Self::Years(ceil_div(secs, YEAR).min(u32::MAX as u64) as u32)
        }
    }
}

impl fmt::Display for IbDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_ib_string())
    }
}

fn ceil_div(num: u64, den: u64) -> u64 {
    num.div_ceil(den)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_default_has_epoch_ts() {
        let q = Quote::default();
        assert!(q.bid.is_none());
        assert!(q.ask.is_none());
        assert!(q.last.is_none());
        assert_eq!(q.ts.timestamp(), 0);
    }

    #[test]
    fn quote_serde_roundtrip() {
        let q = Quote {
            bid: Some(100.0),
            ask: Some(100.05),
            last: Some(100.02),
            ts: DateTime::<Utc>::from_timestamp(1_700_000_000, 0).unwrap(),
        };
        let json = serde_json::to_string(&q).unwrap();
        let back: Quote = serde_json::from_str(&json).unwrap();
        assert_eq!(q, back);
    }

    #[test]
    fn connection_state_serde_roundtrip() {
        for state in [
            ConnectionState::Disconnected,
            ConnectionState::Connecting,
            ConnectionState::Connected {
                server_version: 178,
            },
            ConnectionState::Ready,
            ConnectionState::Reconnecting { attempt: 3 },
        ] {
            let json = serde_json::to_string(&state).unwrap();
            let back: ConnectionState = serde_json::from_str(&json).unwrap();
            assert_eq!(state, back);
        }
    }

    #[test]
    fn ib_duration_to_string() {
        assert_eq!(IbDuration::Seconds(60).to_ib_string(), "60 S");
        assert_eq!(IbDuration::Days(3).to_ib_string(), "3 D");
        assert_eq!(IbDuration::Weeks(2).to_ib_string(), "2 W");
        assert_eq!(IbDuration::Months(6).to_ib_string(), "6 M");
        assert_eq!(IbDuration::Years(1).to_ib_string(), "1 Y");
    }

    #[test]
    fn ib_duration_from_lookback_buckets() {
        // < 60 days → Seconds
        assert_eq!(
            IbDuration::from_lookback(Duration::from_secs(3_600)),
            IbDuration::Seconds(3_600)
        );
        assert_eq!(
            IbDuration::from_lookback(Duration::from_secs(59 * 86_400)),
            IbDuration::Seconds(59 * 86_400)
        );
        // 60 days .. 1 year → Days
        assert_eq!(
            IbDuration::from_lookback(Duration::from_secs(60 * 86_400)),
            IbDuration::Days(60)
        );
        // 1y .. 4y → Weeks
        let span = Duration::from_secs(2 * 365 * 86_400);
        assert!(matches!(
            IbDuration::from_lookback(span),
            IbDuration::Weeks(_)
        ));
        // 4y .. 40y → Months
        let span = Duration::from_secs(10 * 365 * 86_400);
        assert!(matches!(
            IbDuration::from_lookback(span),
            IbDuration::Months(_)
        ));
        // >= 40y → Years
        let span = Duration::from_secs(50 * 365 * 86_400);
        assert!(matches!(
            IbDuration::from_lookback(span),
            IbDuration::Years(_)
        ));
    }

    #[test]
    fn ib_duration_roundtrips_through_strings() {
        // Every variant round-trips through Display.
        for d in [
            IbDuration::Seconds(1),
            IbDuration::Days(2),
            IbDuration::Weeks(3),
            IbDuration::Months(4),
            IbDuration::Years(5),
        ] {
            assert_eq!(d.to_string(), d.to_ib_string());
        }
    }

    #[test]
    fn ib_duration_serde_roundtrip() {
        for d in [
            IbDuration::Seconds(60),
            IbDuration::Days(5),
            IbDuration::Years(1),
        ] {
            let json = serde_json::to_string(&d).unwrap();
            let back: IbDuration = serde_json::from_str(&json).unwrap();
            assert_eq!(d, back);
        }
    }
}
