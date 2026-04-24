//! Slice 4 of chart-transition: level-annotation integration tests.
//!
//! Each test exercises the full `SessionChart` → `ToolEffect` →
//! `AnnotationStore` round-trip WITHOUT spinning up the iced event
//! loop. The app's view / update cycle is simulated directly by:
//!
//! 1. Construct an `AnnotationStore`.
//! 2. Activate the level tool on the `SessionChart`.
//! 3. Dispatch input events via `SessionChart::handle_level_input`.
//! 4. Drain projected effects.
//! 5. Translate each effect into the same `AnnotationStore` call the
//!    app's handler does.
//! 6. Assert persistence / round-trip semantics on the store.
//!
//! Gated on `session_chart_tests` so the default test invocation
//! doesn't pay the extra build cost.

#![cfg(feature = "session_chart_tests")]

use std::sync::Arc;

use async_trait::async_trait;
use chrono::TimeZone;
use midas_app::annotation_store::{AnnotationStore, StoredLevel};
use midas_app::session_chart::{LevelEditPopup, ProjectedEffect, SessionChart, SessionChartDriver};
use midas_axis::{PriceRange, Viewport};
use midas_bars::{BarPeriod, CandleSeries, Symbol};
use midas_calendar::{crypto_spot, Timestamp};
use midas_chart::widget::level::LineStyle;
use midas_chart::widget::price_line::{LineExtent, LineStroke, PriceLine};
use midas_chart::{HorizontalLevel, LevelIcon};
use midas_scene::input::{InputEvent, Modifiers, MouseButton, Point};
use midas_scene::layers::{LevelView, SharedLevelDrag};
use midas_scene::tools::ContextMenuAction;
use midas_scene::ThemePalette;
use midas_stream::{BarStream, BarStreamMeta, StreamError, TimeRange};
use parking_lot::RwLock;
use tokio::sync::mpsc;

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> Timestamp {
    chrono::Utc.with_ymd_and_hms(y, mo, d, h, mi, 0).unwrap()
}

/// Mock bar stream that never emits — tests for this slice don't need
/// live candle data.
struct EmptyBarStream {
    meta: BarStreamMeta,
    rx: mpsc::Receiver<midas_bars::Candle>,
}

impl EmptyBarStream {
    fn btc_m1() -> (mpsc::Sender<midas_bars::Candle>, Self) {
        let cal = crypto_spot();
        let sym = Symbol::new("BTC-USD", cal.id());
        let (tx, rx) = mpsc::channel(1);
        let meta = BarStreamMeta::new(sym, cal, BarPeriod::m1());
        (tx, Self { meta, rx })
    }
}

#[async_trait]
impl BarStream for EmptyBarStream {
    fn meta(&self) -> &BarStreamMeta {
        &self.meta
    }
    async fn next(&mut self) -> Option<midas_bars::Candle> {
        self.rx.recv().await
    }
    async fn snapshot(
        &mut self,
        _range: TimeRange,
    ) -> Result<Vec<midas_bars::Candle>, StreamError> {
        Err(StreamError::NotSeekable)
    }
}

fn fresh_btc_series() -> Arc<RwLock<CandleSeries>> {
    let cal = crypto_spot();
    let sym = Symbol::new("BTC-USD", cal.id());
    Arc::new(RwLock::new(CandleSeries::new(
        cal.id(),
        BarPeriod::m1(),
        sym,
    )))
}

fn mk_widget() -> SessionChart {
    let (_tx, stream) = EmptyBarStream::btc_m1();
    let driver = Arc::new(SessionChartDriver::spawn(fresh_btc_series(), stream));
    let cal = crypto_spot();
    let start = utc(2024, 3, 1, 0, 0);
    let end = utc(2024, 3, 2, 0, 0);
    let pr = PriceRange::new(49_900.0, 50_200.0).unwrap();
    let vp = Viewport::new(1000.0, 400.0);
    SessionChart::new(
        driver,
        cal,
        BarPeriod::m1(),
        pr,
        vp,
        ThemePalette::dark_default(),
        (start, end),
    )
    .expect("widget construction succeeds on canonical inputs")
}

/// Build a `StoredLevel` the way `AnnotationStore::add_level` expects.
fn stored(id: u64, price: f64, locked: bool) -> StoredLevel {
    StoredLevel {
        level: HorizontalLevel {
            id,
            line: PriceLine {
                price,
                extent: LineExtent::default(),
                stroke: LineStroke {
                    color: [1.0, 1.0, 1.0, 1.0],
                    width: 1.0,
                    style: LineStyle::default(),
                },
            },
            label: None,
            icon: LevelIcon::default(),
        },
        locked,
    }
}

/// Translate one `ProjectedEffect` into the annotation store, returning
/// the annotation ids touched for assertion convenience.
fn apply_effect(store: &mut AnnotationStore, symbol: &str, effect: ProjectedEffect) -> Option<u64> {
    match effect {
        ProjectedEffect::CreateLevel { price, lock } => {
            let id = store.alloc_level_id();
            store.add_level(symbol, stored(id, price, lock));
            Some(id)
        }
        ProjectedEffect::UpdateLevel { id, price } => {
            let ok = store.update_level(symbol, id, |lv, _locked| {
                lv.line.price = price;
            });
            ok.then_some(id)
        }
        ProjectedEffect::DeleteLevel { id } => {
            store.remove_level(symbol, id);
            Some(id)
        }
        _ => None,
    }
}

fn set_scene_level_views(widget: &mut SessionChart, store: &AnnotationStore, symbol: &str) {
    let levels: Vec<LevelView> = store
        .levels_for(symbol)
        .iter()
        .map(|sl| LevelView {
            id: sl.level.id,
            price: sl.level.line.price,
            label: sl.level.label.clone().unwrap_or_default().into(),
            color: [0xff, 0xff, 0xff, 0xff],
            locked: sl.locked,
        })
        .collect();
    widget.set_level_views(levels);
}

// ── Tests ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn place_persists_and_round_trips() {
    // Simulate the "Add Level → click → window close → reopen" flow.
    let mut widget = mk_widget();
    let mut store = AnnotationStore::new();
    let symbol = "BTC-USD";

    widget.activate_level_tool();
    // Feed a snap so the tool has a finite price.
    widget.update_level_snap(50_000.0, 200.0);

    widget.handle_level_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(500.0, 200.0),
        modifiers: Modifiers::default(),
    });

    let effects = widget.drain_level_effects();
    assert_eq!(effects.len(), 1);
    assert_eq!(
        effects[0],
        ProjectedEffect::CreateLevel {
            price: 50_000.0,
            lock: false,
        }
    );

    // Persist via AnnotationStore.
    let new_id =
        apply_effect(&mut store, symbol, effects[0].clone()).expect("create returns an id");
    assert_eq!(store.levels_for(symbol).len(), 1);

    // Simulate app restart: construct a new AnnotationStore from the
    // current one's level-config projection. `from_level_configs` is
    // the round-trip entry point the app uses on disk restore.
    let snapshot = store.to_level_configs();
    let mut restored = AnnotationStore::new();
    restored.import_level_configs(&snapshot);

    let restored_levels = restored.levels_for(symbol);
    assert_eq!(restored_levels.len(), 1);
    // `import_level_configs` allocates fresh ids; the important
    // invariant is that the price + symbol round-trip.
    assert!((restored_levels[0].level.line.price - 50_000.0).abs() < 1e-6);
    let _ = new_id;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drag_emits_update_and_persists() {
    let mut widget = mk_widget();
    let mut store = AnnotationStore::new();
    let symbol = "BTC-USD";

    // Pre-seed a level.
    let id = store.alloc_level_id();
    store.add_level(symbol, stored(id, 50_000.0, false));
    set_scene_level_views(&mut widget, &store, symbol);

    // Level 50_000 → price range 49_900..50_200 → y = (50_200 - 50_000)/300 * 400 ≈ 266.67.
    let level_y = {
        let pr = widget.price_range();
        let h = widget.viewport().height_px;
        ((pr.high() - 50_000.0) / (pr.high() - pr.low()) * h as f64) as f32
    };

    // Start drag on the line.
    widget.handle_level_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(500.0, level_y),
        modifiers: Modifiers::default(),
    });
    let drag: SharedLevelDrag = widget.level_drag_state();
    assert_eq!(drag.lock().dragging, Some(id), "drag session started");

    // Drag down by ~50 px → price drops.
    widget.handle_level_input(InputEvent::MouseMove {
        pt: Point::new(500.0, level_y + 50.0),
    });
    let effects = widget.drain_level_effects();
    assert_eq!(effects.len(), 1);
    let projected_id = apply_effect(&mut store, symbol, effects[0].clone());
    assert_eq!(projected_id, Some(id));

    // Release.
    widget.handle_level_input(InputEvent::MouseUp {
        button: MouseButton::Left,
        pt: Point::new(500.0, level_y + 50.0),
    });
    assert!(drag.lock().dragging.is_none());

    // Persistence round-trip.
    let snapshot = store.to_level_configs();
    let mut restored = AnnotationStore::new();
    restored.import_level_configs(&snapshot);
    let levels = restored.levels_for(symbol);
    assert_eq!(levels.len(), 1);
    // Price went down (cursor moved down in screen coords).
    assert!(levels[0].level.line.price < 50_000.0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn locked_level_drag_is_rejected_at_widget() {
    let mut widget = mk_widget();
    let mut store = AnnotationStore::new();
    let symbol = "BTC-USD";

    // Pre-seed a LOCKED level.
    let id = store.alloc_level_id();
    store.add_level(symbol, stored(id, 50_000.0, true));
    set_scene_level_views(&mut widget, &store, symbol);

    let level_y = {
        let pr = widget.price_range();
        let h = widget.viewport().height_px;
        ((pr.high() - 50_000.0) / (pr.high() - pr.low()) * h as f64) as f32
    };

    widget.handle_level_input(InputEvent::MouseDown {
        button: MouseButton::Left,
        pt: Point::new(500.0, level_y),
        modifiers: Modifiers::default(),
    });
    let drag: SharedLevelDrag = widget.level_drag_state();
    assert!(
        drag.lock().dragging.is_none(),
        "locked level must reject drag"
    );
    // MouseMove should NOT emit any UpdateLevel.
    widget.handle_level_input(InputEvent::MouseMove {
        pt: Point::new(500.0, level_y + 50.0),
    });
    let effects = widget.drain_level_effects();
    assert!(effects.is_empty(), "no effects emitted for locked level");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn right_click_opens_context_menu_and_delete_persists() {
    let mut widget = mk_widget();
    let mut store = AnnotationStore::new();
    let symbol = "BTC-USD";

    let id = store.alloc_level_id();
    store.add_level(symbol, stored(id, 50_000.0, false));
    set_scene_level_views(&mut widget, &store, symbol);

    let level_y = {
        let pr = widget.price_range();
        let h = widget.viewport().height_px;
        ((pr.high() - 50_000.0) / (pr.high() - pr.low()) * h as f64) as f32
    };

    // Right-click on the level.
    widget.handle_level_input(InputEvent::MouseDown {
        button: MouseButton::Right,
        pt: Point::new(500.0, level_y),
        modifiers: Modifiers::default(),
    });
    let effects = widget.drain_level_effects();
    assert_eq!(effects.len(), 3, "three menu items projected");

    // Find the Delete action.
    let delete_effect = effects
        .iter()
        .find(|e| {
            matches!(
                e,
                ProjectedEffect::OpenContextMenu {
                    action: ContextMenuAction::Delete { .. },
                    ..
                }
            )
        })
        .expect("Delete item present");
    let delete_id = match delete_effect {
        ProjectedEffect::OpenContextMenu {
            action: ContextMenuAction::Delete { id },
            ..
        } => *id,
        _ => unreachable!(),
    };

    // Simulate the app's context-menu "Delete" handler.
    apply_effect(
        &mut store,
        symbol,
        ProjectedEffect::DeleteLevel { id: delete_id },
    );
    assert!(
        store.levels_for(symbol).is_empty(),
        "delete removes the level"
    );

    // Round-trip persistence confirms the delete survived a save/load.
    let snapshot = store.to_level_configs();
    let mut restored = AnnotationStore::new();
    restored.import_level_configs(&snapshot);
    assert!(
        restored.levels_for(symbol).is_empty(),
        "persistence snapshot reflects delete"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn level_edit_popup_commits_new_price_via_projected_effect() {
    // Smaller focused test: the `LevelEditPopup` constructs correctly
    // and parses back a user-edited price, which the host then routes
    // via `ProjectedEffect::UpdateLevel`.
    let mut store = AnnotationStore::new();
    let symbol = "AAPL";
    let id = store.alloc_level_id();
    store.add_level(symbol, stored(id, 150.0, false));

    let mut popup = LevelEditPopup::new(id, symbol, 150.0);
    assert_eq!(popup.parsed_price(), Some(150.0));
    popup.price_text = "151.25".to_string();
    assert_eq!(popup.parsed_price(), Some(151.25));

    // Host-side commit.
    apply_effect(
        &mut store,
        symbol,
        ProjectedEffect::UpdateLevel {
            id,
            price: popup.parsed_price().expect("valid price"),
        },
    );
    let levels = store.levels_for(symbol);
    assert_eq!(levels.len(), 1);
    assert!((levels[0].level.line.price - 151.25).abs() < 1e-6);
}
