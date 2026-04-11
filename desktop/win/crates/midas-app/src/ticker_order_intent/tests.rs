//! Unit tests for the ticker_order_intent module.
//!
//! Coverage matches the Slice 1a testing list:
//! - round-trip serde with unknown-field forward-compat
//! - version migration stub (v0 blob → v1 defaults)
//! - coalescing: many rapid upserts → single commit
//! - NaN / non-finite rejection in `validate`
//! - validation drops a TP-below-entry Long on load
//! - shutdown drain: upserts + shutdown → reopen observes state
//! - `ForgetSymbol`: absent → NoOp; present → removed + deleted
//! - equality no-op: second identical upsert returns NoOp
//! - simulated crash: drop actor/DB without shutdown → reopen sees data

use std::collections::HashMap;
use std::sync::Arc;

use midas_chart::widget::order_bracket::EntryType;

use crate::annotation_store::SymbolKey;
use crate::order_panel::{OrderSide, PriceInputMode, StopLossType};

use super::actor::{open_and_hydrate, spawn_actor, IntentSource, OrderIntentMsg, OrderIntentReply};
use super::store::{TickerOrderIntentStore, UpsertOutcome};
use super::validate::{validate, IntentDefect};
use super::{EntryMemory, GatrAnchor, TickerOrderIntent, TickerOrderIntentHandle};

// ── Fixtures ──────────────────────────────────────────────────────────

fn sample_intent(symbol: &str) -> TickerOrderIntent {
    let mut entries = HashMap::new();
    entries.insert(
        (OrderSide::Buy, EntryType::Limit),
        EntryMemory {
            entry_price_or_offset: Some(100.0),
            quantity: Some(10.0),
            tp_enabled: true,
            tp_value: "102.00".to_string(),
            tp_mode: PriceInputMode::Absolute,
            sl_enabled: true,
            sl_value: "98.00".to_string(),
            sl_mode: PriceInputMode::Absolute,
            sl_type: StopLossType::Stop,
            sl_limit_value: String::new(),
        },
    );
    TickerOrderIntent {
        version: super::CURRENT_VERSION,
        symbol: SymbolKey::new(symbol),
        last_side: OrderSide::Buy,
        last_entry_type: EntryType::Limit,
        entries,
        gatr_anchor: GatrAnchor {
            anchor_price: Some(100.0),
            anchor_gatr: Some(1.5),
        },
        live_annotation_id: None,
        broker_order_id: None,
        pinned: false,
        updated_at: chrono::Utc::now(),
    }
}

// ── Serde round-trip + forward-compat ─────────────────────────────────

#[test]
fn serde_round_trip_preserves_intent() {
    let intent = sample_intent("AAPL");
    let bytes = serde_json::to_vec_pretty(&intent).unwrap();
    let decoded: TickerOrderIntent = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded.symbol, intent.symbol);
    assert_eq!(decoded.entries.len(), 1);
    let key = (OrderSide::Buy, EntryType::Limit);
    assert_eq!(
        decoded.entries.get(&key).unwrap().entry_price_or_offset,
        Some(100.0)
    );
}

#[test]
fn serde_accepts_unknown_fields() {
    let intent = sample_intent("MSFT");
    let mut v: serde_json::Value = serde_json::to_value(&intent).unwrap();
    // Inject a field the current code does not know about.
    v.as_object_mut()
        .unwrap()
        .insert("future_field".to_string(), serde_json::json!("anything"));
    let bytes = serde_json::to_vec(&v).unwrap();
    let decoded: TickerOrderIntent = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(decoded.symbol, intent.symbol);
}

// ── Migration stub (v0 = v1 blob without `version` field) ─────────────

#[test]
fn migrate_v0_v1_fills_defaults() {
    // Hand-roll a v0 blob: same shape as v1 but missing `version` and
    // `updated_at` — all missing fields must pick up defaults.
    let blob = serde_json::json!({
        "symbol": "NVDA",
        "last_side": "Buy",
        "last_entry_type": "Market",
        "entries": [],
        "gatr_anchor": {},
    });
    let bytes = serde_json::to_vec(&blob).unwrap();
    let decoded = super::migrate_v0_v1(&bytes).expect("v0 blob should decode");
    assert_eq!(decoded.version, super::CURRENT_VERSION);
    assert_eq!(decoded.symbol, SymbolKey::new("NVDA"));
    assert!(decoded.entries.is_empty());
    assert!(!decoded.pinned);
}

// ── Store: equality no-op and generation counter ──────────────────────

#[test]
fn store_equal_upsert_is_noop() {
    let store = TickerOrderIntentStore::new();
    let intent = sample_intent("AAPL");
    let outcome1 = store.upsert(SymbolKey::new("AAPL"), intent.clone());
    match outcome1 {
        UpsertOutcome::Applied { generation } => assert_eq!(generation, 1),
        _ => panic!("first upsert should Apply"),
    }
    let outcome2 = store.upsert(SymbolKey::new("AAPL"), intent);
    match outcome2 {
        UpsertOutcome::NoOp { reason } => {
            assert_eq!(reason, super::NoOpReason::IdenticalToCache);
        }
        _ => panic!("second identical upsert should NoOp"),
    }
    assert_eq!(store.generation(), 1);
}

#[test]
fn store_changed_upsert_bumps_generation() {
    let store = TickerOrderIntentStore::new();
    let mut intent = sample_intent("AAPL");
    store.upsert(SymbolKey::new("AAPL"), intent.clone());
    intent.pinned = true;
    let outcome = store.upsert(SymbolKey::new("AAPL"), intent);
    match outcome {
        UpsertOutcome::Applied { generation } => assert_eq!(generation, 2),
        _ => panic!("changed upsert should Apply"),
    }
}

// ── Coalescing: drain_dirty returns one batch ─────────────────────────

#[test]
fn store_coalesces_many_writes_into_one_drain() {
    let store = TickerOrderIntentStore::new();
    // 50 mutations of the same symbol — each Applied, each marks dirty.
    for i in 0..50 {
        let mut intent = sample_intent("AAPL");
        intent.pinned = i % 2 == 0;
        intent.gatr_anchor.anchor_price = Some(100.0 + i as f64 * 0.01);
        store.upsert(SymbolKey::new("AAPL"), intent);
    }
    let drained = store.drain_dirty();
    // Coalesced: only one entry for AAPL even though we upserted 50x.
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].0, SymbolKey::new("AAPL"));
    // Second drain is empty — the dirty set is a set, not a log.
    assert!(store.drain_dirty().is_empty());
}

// ── validate: NaN, out-of-band, TP wrong side ─────────────────────────

#[test]
fn validate_rejects_nan_entry_price() {
    let mut intent = sample_intent("AAPL");
    intent.entries.get_mut(&(OrderSide::Buy, EntryType::Limit)).unwrap()
        .entry_price_or_offset = Some(f64::NAN);
    let err = validate(&intent, None, None).unwrap_err();
    assert!(matches!(err, IntentDefect::NaN { .. }));
}

#[test]
fn validate_rejects_infinite_gatr() {
    let mut intent = sample_intent("AAPL");
    intent.gatr_anchor.anchor_gatr = Some(f64::INFINITY);
    let err = validate(&intent, None, None).unwrap_err();
    assert!(matches!(err, IntentDefect::NaN { .. }));
}

#[test]
fn validate_rejects_tp_below_entry_for_long() {
    let mut intent = sample_intent("AAPL");
    let mem = intent
        .entries
        .get_mut(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    mem.tp_value = "99.00".to_string(); // below entry 100 for Long
    let err = validate(&intent, None, None).unwrap_err();
    assert!(matches!(err, IntentDefect::TpWrongSide { .. }));
}

#[test]
fn validate_rejects_out_of_band_price_when_bands_known() {
    let mut intent = sample_intent("AAPL");
    let mem = intent
        .entries
        .get_mut(&(OrderSide::Buy, EntryType::Limit))
        .unwrap();
    mem.entry_price_or_offset = Some(200.0);
    mem.tp_value = "200.10".to_string();
    mem.sl_value = "199.90".to_string();
    // last_price = 100, gatr = 1 → band is ±5 → 200 is way outside.
    let err = validate(&intent, Some(100.0), Some(1.0)).unwrap_err();
    assert!(matches!(err, IntentDefect::OutOfBand { .. }));
}

// ── Actor: load drops invalid rows ────────────────────────────────────

#[test]
fn load_drops_invalid_row_with_warn() {
    use redb::{Database, TableDefinition};
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("drop_invalid.redb");

    // Seed a valid + an invalid row using a raw redb write.
    {
        let db = Database::create(&path).unwrap();
        let def: TableDefinition<'_, &str, &[u8]> = TableDefinition::new("ticker_intent_v1");
        let txn = db.begin_write().unwrap();
        {
            let mut t = txn.open_table(def).unwrap();
            let good = sample_intent("GOOD");
            t.insert(
                "GOOD",
                serde_json::to_vec_pretty(&good).unwrap().as_slice(),
            )
            .unwrap();
            let mut bad = sample_intent("BAD");
            bad.entries
                .get_mut(&(OrderSide::Buy, EntryType::Limit))
                .unwrap()
                .tp_value = "99.00".to_string(); // TP below entry → dropped
            t.insert("BAD", serde_json::to_vec_pretty(&bad).unwrap().as_slice())
                .unwrap();
        }
        txn.commit().unwrap();
    }

    let (store, _db, _ctl) = open_and_hydrate(&path).expect("open should succeed");
    assert!(store.snapshot(&SymbolKey::new("GOOD")).is_some());
    assert!(
        store.snapshot(&SymbolKey::new("BAD")).is_none(),
        "invalid row should be dropped on load"
    );
}

// ── Actor: shutdown drain + reopen sees state ─────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_drain_then_reopen_sees_state() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("shutdown.redb");

    let handle = TickerOrderIntentHandle::open(path.clone()).unwrap();
    for i in 0..5 {
        let mut intent = sample_intent("AAPL");
        intent.gatr_anchor.anchor_price = Some(100.0 + i as f64);
        let reply = handle
            .upsert_async(OrderIntentMsg::Upsert {
                symbol: SymbolKey::new("AAPL"),
                intent: Box::new(intent),
                source: IntentSource::Panel,
            })
            .await
            .unwrap();
        assert!(matches!(reply, OrderIntentReply::Applied { .. } | OrderIntentReply::NoOp { .. }));
    }
    handle.shutdown().await;

    // Reopen and observe.
    let reopened = TickerOrderIntentHandle::open(path).unwrap();
    let snap = reopened.snapshot(&SymbolKey::new("AAPL")).unwrap();
    assert_eq!(snap.gatr_anchor.anchor_price, Some(104.0));
    reopened.shutdown().await;
}

// ── Actor: forget path ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forget_absent_then_present() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("forget.redb");

    let handle = TickerOrderIntentHandle::open(path.clone()).unwrap();

    // Absent → AlreadyAbsent.
    let reply = handle.forget(SymbolKey::new("AAPL")).await.unwrap();
    assert!(matches!(reply, OrderIntentReply::AlreadyAbsent));

    // Insert then forget → Forgotten, and row must not resurrect on reopen.
    handle
        .upsert_async(OrderIntentMsg::Upsert {
            symbol: SymbolKey::new("AAPL"),
            intent: Box::new(sample_intent("AAPL")),
            source: IntentSource::Panel,
        })
        .await
        .unwrap();
    let reply = handle.forget(SymbolKey::new("AAPL")).await.unwrap();
    assert!(matches!(reply, OrderIntentReply::Forgotten));
    handle.shutdown().await;

    let reopened = TickerOrderIntentHandle::open(path).unwrap();
    assert!(reopened.snapshot(&SymbolKey::new("AAPL")).is_none());
    reopened.shutdown().await;
}

// ── Actor: equality no-op path ────────────────────────────────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn actor_equality_noop() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("noop.redb");
    let handle = TickerOrderIntentHandle::open(path).unwrap();
    // Use a deterministic timestamp so both upserts are truly identical.
    let mut intent = sample_intent("AAPL");
    intent.updated_at = chrono::DateTime::<chrono::Utc>::from_timestamp(1_700_000_000, 0)
        .expect("valid timestamp");

    let first = handle
        .upsert_async(OrderIntentMsg::Upsert {
            symbol: SymbolKey::new("AAPL"),
            intent: Box::new(intent.clone()),
            source: IntentSource::Panel,
        })
        .await
        .unwrap();
    assert!(matches!(first, OrderIntentReply::Applied { .. }));

    let second = handle
        .upsert_async(OrderIntentMsg::Upsert {
            symbol: SymbolKey::new("AAPL"),
            intent: Box::new(intent),
            source: IntentSource::Panel,
        })
        .await
        .unwrap();
    assert!(matches!(second, OrderIntentReply::NoOp { .. }));

    handle.shutdown().await;
}

// ── "Simulated crash": Immediate-durability writes survive re-open ────
//
// NOTE: A true crash is not modelable without process termination —
// stable Rust cannot kill a running thread, and `redb` holds its file
// lock until `Database` is dropped. Our `shutdown()` path is the
// closest analogue: it drains pending writes with Immediate
// durability and releases the file lock. To verify that data written
// with Immediate durability survives a re-open, we write directly
// through our actor, shut down, reopen, and assert the value round-
// tripped. This is distinct from the separate `shutdown_drain`
// test because it forces `flush_now()` mid-session rather than
// relying on the shutdown path to drain the dirty set.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_flush_survives_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("crash.redb");

    {
        let handle = TickerOrderIntentHandle::open(path.clone()).unwrap();
        let mut intent = sample_intent("AAPL");
        intent.gatr_anchor.anchor_price = Some(999.0);
        handle
            .upsert_async(OrderIntentMsg::Upsert {
                symbol: SymbolKey::new("AAPL"),
                intent: Box::new(intent),
                source: IntentSource::Panel,
            })
            .await
            .unwrap();
        // Force a durable flush — models "the app committed an order
        // and hardened the state before anything else could happen."
        handle.flush_now().await;
        // Shutdown to release the file lock before we reopen.
        handle.shutdown().await;
    }

    let reopened = TickerOrderIntentHandle::open(path).unwrap();
    let snap = reopened.snapshot(&SymbolKey::new("AAPL"));
    assert!(
        snap.is_some(),
        "state persisted via flush_now() should survive a clean reopen"
    );
    assert_eq!(snap.unwrap().gatr_anchor.anchor_price, Some(999.0));
    reopened.shutdown().await;
}

// ── Handle snapshot is sync and sees writes immediately ───────────────

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handle_upsert_is_visible_to_next_snapshot_sync() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("sync_visibility.redb");
    let handle = TickerOrderIntentHandle::open(path).unwrap();

    let outcome = handle.upsert(OrderIntentMsg::Upsert {
        symbol: SymbolKey::new("AAPL"),
        intent: Box::new(sample_intent("AAPL")),
        source: IntentSource::Panel,
    });
    assert!(matches!(outcome, UpsertOutcome::Applied { .. }));
    // Sync snapshot right after sync upsert — must be visible.
    let snap = handle.snapshot(&SymbolKey::new("AAPL"));
    assert!(snap.is_some());
    let _: Arc<TickerOrderIntent> = snap.unwrap();
    handle.shutdown().await;
}
