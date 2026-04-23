//! `Symbol` — bar-level identity for the session-aware chart stack.
//!
//! A `Symbol` couples a ticker with the `CalendarId` that governs its bar
//! semantics. Per the R2-G-9 `SymbolResolver` resolution (see
//! `plan/session-aware-charts/00a-ideal-design.md`), calendar is resolved
//! explicitly — there is no hard-coded string matching against "BTC" or
//! "SPY". Callers run a ticker through a provider-specific resolver and
//! get a fully-typed `Symbol` back.
//!
//! `ticker` is `&'static str` so `Symbol` is `Copy` and sub-nanosecond to
//! clone. Tests that need per-test tickers can `Box::leak` a `String`.
//! Dynamic tickers arriving over the wire (deserialization) are interned
//! via `Symbol::from_ticker_leak`, mirroring the `CalendarId` pattern.

use midas_calendar::CalendarId;

/// Bar-level identity. Pair of a ticker and the calendar that scopes its
/// session semantics.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct Symbol {
    ticker: &'static str,
    calendar: CalendarId,
}

impl Symbol {
    /// Construct a `Symbol` from a `'static` ticker and its calendar. The
    /// `'static` bound keeps `Symbol` `Copy` and eliminates per-tick
    /// allocation on the hot path.
    #[inline]
    pub const fn new(ticker: &'static str, calendar: CalendarId) -> Self {
        Self { ticker, calendar }
    }

    /// Intern `ticker` into a `&'static str` by `Box::leak`ing and return
    /// the resulting `Symbol`. Intended for fixture replay / wire decode
    /// where the ticker arrives as an owned `String`. The leaked memory
    /// is bounded by the number of distinct tickers a process sees —
    /// typically a few dozen in production.
    pub fn from_ticker_leak(ticker: &str, calendar: CalendarId) -> Self {
        let leaked: &'static str = Box::leak(ticker.to_owned().into_boxed_str());
        Self::new(leaked, calendar)
    }

    #[inline]
    pub fn ticker(&self) -> &'static str {
        self.ticker
    }

    #[inline]
    pub fn calendar(&self) -> CalendarId {
        self.calendar
    }
}

impl std::fmt::Display for Symbol {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.ticker, self.calendar)
    }
}

// --- serde ---
//
// Manual Serialize / Deserialize because `&'static str` cannot be
// produced through serde's derive (the `'de` lifetime is not
// guaranteed `'static`). Deserialize `Box::leak`s the incoming ticker;
// the lifetime is fine because tickers are a tiny, bounded set seen
// once at wire-decode time.

impl serde::Serialize for Symbol {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeStruct;
        let mut st = serializer.serialize_struct("Symbol", 2)?;
        st.serialize_field("ticker", self.ticker)?;
        st.serialize_field("calendar", &self.calendar)?;
        st.end()
    }
}

impl<'de> serde::Deserialize<'de> for Symbol {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Raw {
            ticker: String,
            calendar: CalendarId,
        }
        let raw = Raw::deserialize(deserializer)?;
        Ok(Symbol::from_ticker_leak(&raw.ticker, raw.calendar))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use midas_calendar::XNYS_ID;

    #[test]
    fn symbol_new_is_const_and_copyable() {
        const S: Symbol = Symbol::new("AAPL", XNYS_ID);
        let t = S;
        assert_eq!(S.ticker(), "AAPL");
        assert_eq!(t.calendar(), XNYS_ID);
    }

    #[test]
    fn symbol_from_ticker_leak_is_copy() {
        let s = Symbol::from_ticker_leak(&format!("SYM{}", 7), XNYS_ID);
        let t = s;
        assert_eq!(s, t);
        assert_eq!(s.ticker(), "SYM7");
    }

    #[test]
    fn symbol_display() {
        let s = Symbol::new("SPY", XNYS_ID);
        assert_eq!(format!("{s}"), "SPY@XNYS");
    }

    #[test]
    fn symbol_round_trip_json() {
        let s = Symbol::new("AAPL", XNYS_ID);
        let json = serde_json::to_string(&s).unwrap();
        let back: Symbol = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }
}
