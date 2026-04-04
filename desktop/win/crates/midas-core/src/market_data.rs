//! Market data snapshot types.

/// Point-in-time market data snapshot for one symbol.
///
/// Computed from daily candle data. In Phase 1 (IB integration),
/// updated incrementally from streaming tick data.
#[derive(Debug, Clone, Default)]
pub struct MarketSnapshot {
    /// Last closing price (or last trade price when streaming).
    pub last_price: Option<f64>,
    /// Previous close (for change% computation).
    pub prev_close: Option<f64>,
    /// Percentage change from previous close.
    pub change_pct: Option<f64>,
    /// Gerchik ATR percentage consumed (0.0+, can exceed 100).
    /// Computed from daily bars. See `market_cache::compute_daily_gatr`.
    pub gatr_pct: Option<f32>,
}
