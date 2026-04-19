//! Pure tests for [`super::WindowGeometry`].

use iced::window;
use midas_core::config::WindowConfig;

use super::{Effect, WindowGeometry, WindowGeometryMsg};

const FALLBACK: (u32, u32) = (1280, 720);

#[test]
fn new_uses_initial_size_and_no_window_id() {
    let g = WindowGeometry::new(FALLBACK);
    assert_eq!(g.size(), FALLBACK);
    assert!(g.position().is_none());
    assert!(g.monitor_size().is_none());
    assert!(g.main_window().is_none());
}

#[test]
fn from_config_round_trips_size_position_monitor() {
    let cfg = WindowConfig {
        width: 1920,
        height: 1080,
        maximized: false,
        x: Some(100),
        y: Some(200),
        monitor_width: Some(2560),
        monitor_height: Some(1440),
    };
    let g = WindowGeometry::from_config(&cfg, FALLBACK);
    assert_eq!(g.size(), (1920, 1080));
    assert_eq!(g.position(), Some((100, 200)));
    assert_eq!(g.monitor_size(), Some((2560, 1440)));
    let back = g.to_config();
    assert_eq!(back.width, 1920);
    assert_eq!(back.height, 1080);
    assert_eq!(back.x, Some(100));
    assert_eq!(back.y, Some(200));
    assert_eq!(back.monitor_width, Some(2560));
    assert_eq!(back.monitor_height, Some(1440));
}

#[test]
fn from_config_zero_size_falls_back() {
    // Old/blank config with no size — use the parent-supplied
    // fallback so iced gets a sane default.
    let cfg = WindowConfig::default();
    let g = WindowGeometry::from_config(&cfg, FALLBACK);
    assert_eq!(g.size(), FALLBACK);
    assert!(g.position().is_none());
}

#[test]
fn from_config_partial_position_drops_to_none() {
    // Hand-edited config with only one of x/y — must not panic
    // or store a half-set position.
    let cfg = WindowConfig {
        width: 800,
        height: 600,
        x: Some(10),
        y: None,
        ..Default::default()
    };
    let g = WindowGeometry::from_config(&cfg, FALLBACK);
    assert!(g.position().is_none());
}

#[test]
fn main_window_opened_stores_id_and_emits_query() {
    let mut g = WindowGeometry::new(FALLBACK);
    let effects = g.update(WindowGeometryMsg::MainWindowOpened(window::Id::unique()));
    assert!(g.main_window().is_some());
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::QueryMonitor(_)));
}

#[test]
fn moved_emits_dirty_and_query_when_window_known() {
    let mut g = WindowGeometry::new(FALLBACK);
    let id = window::Id::unique();
    g.update(WindowGeometryMsg::MainWindowOpened(id));
    let effects = g.update(WindowGeometryMsg::Moved(50, 60));
    assert_eq!(g.position(), Some((50, 60)));
    assert_eq!(effects.len(), 2);
    assert!(matches!(effects[0], Effect::MarkConfigDirty));
    assert!(matches!(effects[1], Effect::QueryMonitor(_)));
}

#[test]
fn moved_before_main_window_opened_just_marks_dirty() {
    // Boot race: WM_MOVE on Windows can arrive before iced emits
    // Opened. The controller stores the position but skips the
    // monitor re-query (no id yet). Position survives until the
    // Opened arrives later.
    let mut g = WindowGeometry::new(FALLBACK);
    let effects = g.update(WindowGeometryMsg::Moved(10, 20));
    assert_eq!(g.position(), Some((10, 20)));
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::MarkConfigDirty));
}

#[test]
fn resized_updates_size_and_marks_dirty() {
    let mut g = WindowGeometry::new(FALLBACK);
    let effects = g.update(WindowGeometryMsg::Resized(1024, 768));
    assert_eq!(g.size(), (1024, 768));
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::MarkConfigDirty));
}

#[test]
fn monitor_size_result_some_stores_and_marks_dirty() {
    let mut g = WindowGeometry::new(FALLBACK);
    let effects = g.update(WindowGeometryMsg::MonitorSizeResult(Some(iced::Size::new(
        2560.0, 1440.0,
    ))));
    assert_eq!(g.monitor_size(), Some((2560, 1440)));
    assert_eq!(effects.len(), 1);
    assert!(matches!(effects[0], Effect::MarkConfigDirty));
}

#[test]
fn monitor_size_result_none_is_noop() {
    let mut g = WindowGeometry::new(FALLBACK);
    let effects = g.update(WindowGeometryMsg::MonitorSizeResult(None));
    assert!(effects.is_empty());
    assert!(g.monitor_size().is_none());
}

#[test]
fn round_trip_new_to_config_to_new_preserves_size() {
    let g = WindowGeometry::new((1440, 900));
    let cfg = g.to_config();
    let g2 = WindowGeometry::from_config(&cfg, (0, 0));
    assert_eq!(g.size(), g2.size());
    assert_eq!(g.position(), g2.position());
    assert_eq!(g.monitor_size(), g2.monitor_size());
}
