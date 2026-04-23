//! `dump_state` implementation. Serialises a slim projection of
//! [`crate::app::MidasApp`] state for introspection without screenshots.
//!
//! The projection is hand-rolled because `MidasApp` as a whole is not
//! serde-ready (iced types inside), but the fields the devloop cares
//! about — tickers, bracket state, chart symbol/timeframe/camera — all
//! are.

use midas_devloop_proto::CameraSnapshot;
use serde::Serialize;

use crate::app::{ChartPanel, MidasApp};

#[derive(Serialize)]
pub struct StateProjection<'a> {
    pub tickers: std::collections::BTreeMap<String, &'a crate::ticker_state::TickerState>,
    pub active_chart_id: Option<u32>,
    pub charts: Vec<ChartProjection<'a>>,
    pub window_size: (u32, u32),
    pub window_position: Option<(i32, i32)>,
    pub order_blotter: OrderBlotterProjection<'a>,
    pub account_panels: Vec<AccountPanelProjection<'a>>,
    pub recent_symbols: Vec<&'a str>,
    /// Broker-connection state projection. Exposed so devloop tests
    /// (and human debuggers) can assert the sim-auto-spawn path
    /// reached Ready.
    pub broker: BrokerProjection<'a>,
    /// Router state snapshot (S7e / BR-15). Replaces the legacy
    /// `active_market_subs` field; the router owns refcounting
    /// natively and the app no longer tracks a diff-set.
    pub router_state: RouterStateProjection,
    /// Watchlist projections: one entry per panel with just the
    /// fields devloop assertions need (name + symbol list + the
    /// cached `last_price` the watchlist row renders).
    pub watchlists: Vec<WatchlistProjection<'a>>,
    /// Feature-gated session-chart projection. Present when
    /// `session_chart` is enabled; omitted from the dump otherwise.
    /// App-harden M6.
    #[cfg(feature = "session_chart")]
    pub session_charts: Vec<SessionChartProjection>,
}

/// Projection of a single floating session-chart window (app-harden M6).
/// Carries the minimum a devloop test needs to assert a chart is
/// live and healthy — ticker, period, calendar, EH policy, series
/// length, and driver version.
///
/// `window_id` is rendered via `Debug` because `iced::window::Id` is
/// opaque — we don't know its wire representation, but the `Debug`
/// form is stable for any given iced version and that's all the
/// devloop needs for disambiguation.
#[cfg(feature = "session_chart")]
#[derive(Serialize)]
pub struct SessionChartProjection {
    pub window_id: String,
    pub ticker: String,
    pub period: String,
    pub calendar_id: String,
    pub eh_policy: String,
    pub series_len: usize,
    pub version: u64,
    pub axis_kind: String,
}

/// Router state projection for the devloop dump (S7e / BR-15).
///
/// Sourced from `MarketDataRouter::debug_dump`; exposes the set of
/// symbols the router is currently serving and aggregator counts
/// so integration tests can assert subscription bookkeeping
/// without reaching into router internals.
#[derive(Serialize)]
pub struct RouterStateProjection {
    /// Whether the router has been constructed yet (`false` while
    /// an IB connection is still handshaking).
    pub ready: bool,
    /// Sorted list of symbols with at least one live refcount.
    pub subscribed_symbols: Vec<String>,
    /// Count of live symbol hubs — matches
    /// `router.state.per_symbol.len()`.
    pub active_symbol_count: usize,
}

#[derive(Serialize)]
pub struct BrokerProjection<'a> {
    pub connection_display: &'a str,
    pub sim_child_running: bool,
    pub sim_tws_port: Option<u16>,
}

#[derive(Serialize)]
pub struct WatchlistProjection<'a> {
    pub id: u32,
    pub name: &'a str,
    pub tickers: Vec<WatchlistTickerProjection<'a>>,
}

#[derive(Serialize)]
pub struct WatchlistTickerProjection<'a> {
    pub symbol: &'a str,
    pub last_price: Option<f64>,
    pub change_pct: Option<f64>,
}

#[derive(Serialize)]
pub struct AccountPanelProjection<'a> {
    pub id: u32,
    pub name: &'a str,
    pub active_tab: String,
    pub disconnect_banner_ack: bool,
    pub history_cached_rows: usize,
}

#[derive(Serialize)]
pub struct OrderBlotterProjection<'a> {
    pub generation: u64,
    pub len: usize,
    pub rows: Vec<&'a crate::order_blotter::OrderRow>,
}

#[derive(Serialize)]
pub struct ChartProjection<'a> {
    pub chart_id: u32,
    pub symbol: &'a str,
    pub timeframe: String,
    pub camera: CameraSnapshot,
}

pub fn build(app: &MidasApp) -> serde_json::Value {
    let tickers = app
        .tickers
        .iter()
        .map(|(key, state)| (key.as_str().to_owned(), state))
        .collect::<std::collections::BTreeMap<_, _>>();

    let charts = app
        .charts
        .iter()
        .map(|(id, chart)| ChartProjection {
            chart_id: id.0,
            symbol: &chart.symbol,
            timeframe: format!("{:?}", chart.timeframe),
            camera: snapshot_camera(chart),
        })
        .collect();

    let order_blotter = OrderBlotterProjection {
        generation: app.order_blotter.generation(),
        len: app.order_blotter.len(),
        rows: app.order_blotter.rows().collect(),
    };

    let account_panels: Vec<AccountPanelProjection<'_>> = app
        .account_panels
        .iter()
        .map(|(id, p)| AccountPanelProjection {
            id: id.0,
            name: &p.name,
            active_tab: format!("{:?}", p.active_tab),
            disconnect_banner_ack: p.disconnect_banner_ack,
            history_cached_rows: p.history.cached_rows().len(),
        })
        .collect();

    let recent_symbols: Vec<&str> = app
        .recent_symbols
        .iter()
        .map(|e| e.symbol.as_str())
        .collect();

    let broker = BrokerProjection {
        connection_display: &app.broker_connection_display,
        sim_child_running: app.sim_child.is_some(),
        sim_tws_port: app.sim_child.as_ref().map(|s| s.tws_port),
    };

    let router_state = if let Some(router) = app.router.as_ref() {
        // `debug_dump` is async; the devloop uses a blocking shim
        // since this projection is built from an iced `update()`
        // context where spawning a runtime is cheap.
        let debug = futures::executor::block_on(router.debug_dump());
        let mut syms: Vec<String> = debug.iter().map(|d| d.symbol.symbol.clone()).collect();
        syms.sort();
        RouterStateProjection {
            ready: true,
            active_symbol_count: debug.len(),
            subscribed_symbols: syms,
        }
    } else {
        RouterStateProjection {
            ready: false,
            active_symbol_count: 0,
            subscribed_symbols: Vec::new(),
        }
    };

    let watchlists: Vec<WatchlistProjection<'_>> = app
        .watchlists
        .iter()
        .map(|(id, wl)| WatchlistProjection {
            id: id.0,
            name: &wl.name,
            tickers: wl
                .tickers
                .iter()
                .map(|t| {
                    let key = crate::annotation_store::SymbolKey::new(&t.symbol);
                    let snap = app.market_cache.get(&key);
                    WatchlistTickerProjection {
                        symbol: &t.symbol,
                        last_price: snap.and_then(|s| s.last_price),
                        change_pct: snap.and_then(|s| s.change_pct),
                    }
                })
                .collect(),
        })
        .collect();

    #[cfg(feature = "session_chart")]
    let session_charts: Vec<SessionChartProjection> = app
        .floating_session_charts
        .iter()
        .map(|(win_id, state)| {
            // `state.widget` is now an `Arc<RwLock<SessionChart>>`
            // (Phase D shader rewire); take a short-lived read-guard
            // to pull scalar fields. `series()` returns a fresh Arc
            // clone — we drop the outer guard before reading the
            // inner series so we never hold two guards concurrently.
            let (eh_policy, axis_kind, series_arc) = {
                let g = state.widget.read();
                (g.eh_policy(), g.axis_kind(), g.series())
            };
            let (series_len, version) = {
                let g = series_arc.read();
                (g.len(), g.version())
            };
            SessionChartProjection {
                window_id: format!("{win_id:?}"),
                ticker: state.request.ticker.clone(),
                period: format!("{:?}", state.request.period),
                calendar_id: state.request.calendar_id.0.to_string(),
                eh_policy: eh_policy.short_label().to_string(),
                series_len,
                version,
                axis_kind: axis_kind_label(axis_kind).to_string(),
            }
        })
        .collect();

    let projection = StateProjection {
        tickers,
        active_chart_id: app.workspace.focused_chart_id().map(|id| id.0),
        charts,
        window_size: app.window.size(),
        window_position: app.window.position(),
        order_blotter,
        account_panels,
        recent_symbols,
        broker,
        router_state,
        watchlists,
        #[cfg(feature = "session_chart")]
        session_charts,
    };

    serde_json::to_value(&projection).unwrap_or(serde_json::Value::Null)
}

#[cfg(feature = "session_chart")]
fn axis_kind_label(kind: crate::session_chart::AxisKind) -> &'static str {
    match kind {
        crate::session_chart::AxisKind::Continuous => "Continuous",
        crate::session_chart::AxisKind::Compressed => "Compressed",
        crate::session_chart::AxisKind::SessionIndex => "SessionIndex",
    }
}

fn snapshot_camera(chart: &ChartPanel) -> CameraSnapshot {
    let cam = &chart.chart_state.camera;
    CameraSnapshot {
        time_start: cam.time_start as i64,
        time_end: cam.time_end as i64,
        price_low: cam.price_low,
        price_high: cam.price_high,
        viewport_width: cam.viewport_width,
        viewport_height: cam.viewport_height,
        dpi_scale: cam.dpi_scale,
    }
}

/// Walk a dotted path into a `serde_json::Value`. `"tickers.AAPL.live_bracket"`
/// returns the nested value or `None` if any segment is missing. Integer
/// segments index arrays.
pub fn walk_path<'a>(value: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut cursor = value;
    for seg in path.split('.') {
        if seg.is_empty() {
            continue;
        }
        cursor = match cursor {
            serde_json::Value::Object(map) => map.get(seg)?,
            serde_json::Value::Array(arr) => {
                let idx: usize = seg.parse().ok()?;
                arr.get(idx)?
            }
            _ => return None,
        };
    }
    Some(cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn walk_path_nested_object() {
        let v = json!({"a": {"b": {"c": 42}}});
        assert_eq!(walk_path(&v, "a.b.c"), Some(&json!(42)));
    }

    #[test]
    fn walk_path_array_index() {
        let v = json!({"items": [10, 20, 30]});
        assert_eq!(walk_path(&v, "items.1"), Some(&json!(20)));
    }

    #[test]
    fn walk_path_missing_segment() {
        let v = json!({"a": {"b": 1}});
        assert_eq!(walk_path(&v, "a.c"), None);
    }

    #[test]
    fn walk_path_empty_returns_root() {
        let v = json!({"a": 1});
        assert_eq!(walk_path(&v, ""), Some(&v));
    }

    /// Regression: app-harden M6. The dev-harness dump MUST include a
    /// `session_charts` array when the feature is on so devloop tests
    /// can inspect floating session-chart windows. We assert the
    /// projection struct serialises cleanly with `serde_json` — the
    /// full `build(app)` path is covered by existing dev-harness
    /// integration tests.
    #[cfg(feature = "session_chart")]
    #[test]
    fn session_chart_projection_serialises() {
        let p = SessionChartProjection {
            window_id: "Id(1)".into(),
            ticker: "AAPL".into(),
            period: "Clock(Minutes(1))".into(),
            calendar_id: "XNYS".into(),
            eh_policy: "EH".into(),
            series_len: 42,
            version: 100,
            axis_kind: "Compressed".into(),
        };
        let v = serde_json::to_value(&p).unwrap();
        assert_eq!(v["ticker"], json!("AAPL"));
        assert_eq!(v["period"], json!("Clock(Minutes(1))"));
        assert_eq!(v["calendar_id"], json!("XNYS"));
        assert_eq!(v["series_len"], json!(42));
        assert_eq!(v["version"], json!(100));
        assert_eq!(v["axis_kind"], json!("Compressed"));
    }
}
