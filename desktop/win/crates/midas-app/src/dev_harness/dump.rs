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

    let projection = StateProjection {
        tickers,
        active_chart_id: app.workspace.focused_chart_id().map(|id| id.0),
        charts,
        window_size: app.window.size(),
        window_position: app.window.position(),
        order_blotter,
        account_panels,
        recent_symbols,
    };

    serde_json::to_value(&projection).unwrap_or(serde_json::Value::Null)
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
}
