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

use super::actor::{open_and_hydrate, IntentSource, OrderIntentMsg, OrderIntentReply};
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

    let hydrated = open_and_hydrate(&path).expect("open should succeed");
    assert!(hydrated.store.snapshot(&SymbolKey::new("GOOD")).is_some());
    assert!(
        hydrated.store.snapshot(&SymbolKey::new("BAD")).is_none(),
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

// ── Slice 5a: watchlist hygiene (sync ForgetSymbol dispatch) ──────────

/// Mirrors the Slice 5a call path used by the `Message::WatchlistRemoveTicker`
/// handler: a sync `upsert(OrderIntentMsg::ForgetSymbol)` must evict the
/// in-memory cache entry *and* delete the on-disk row, so the next reopen
/// cannot resurrect the symbol. The sync path fires the mailbox message
/// on a background task; shutting the handle down drains that task before
/// we reopen.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watchlist_remove_evicts_intent_and_persists_deletion() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("watchlist_hygiene.redb");

    // Seed: open the handle, upsert an intent, confirm the cache has it.
    let handle = TickerOrderIntentHandle::open(path.clone()).unwrap();
    handle
        .upsert_async(OrderIntentMsg::Upsert {
            symbol: SymbolKey::new("AAPL"),
            intent: Box::new(sample_intent("AAPL")),
            source: IntentSource::Panel,
        })
        .await
        .unwrap();
    assert!(
        handle.snapshot(&SymbolKey::new("AAPL")).is_some(),
        "seed upsert must land in the cache"
    );

    // Simulate the watchlist-remove call site: a sync dispatch of
    // `ForgetSymbol`. This is what `MidasApp::update()` invokes in
    // `Message::WatchlistRemoveTicker` once the symbol is no longer
    // referenced by any watchlist. The sync path on non-`Upsert`
    // messages hands the work to the mailbox actor via a spawned
    // `fire_and_forget` task; we drive the runtime forward until that
    // spawn has definitely enqueued the message.
    let _ = handle.upsert(OrderIntentMsg::ForgetSymbol {
        symbol: SymbolKey::new("AAPL"),
    });

    // Graceful shutdown drains the mailbox in FIFO order, so the
    // previously-enqueued `ForgetSymbol` is guaranteed to be processed
    // before the `Shutdown` arm fires. A short sleep before shutdown
    // lets the spawned fire-and-forget task land on the mailbox — the
    // tokio multi-thread runtime can otherwise let `shutdown` beat the
    // detached spawn to the queue under contention.
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    handle.shutdown().await;

    // Reopen: the row must not resurrect.
    let reopened = TickerOrderIntentHandle::open(path).unwrap();
    assert!(
        reopened.snapshot(&SymbolKey::new("AAPL")).is_none(),
        "forget must delete the on-disk row, not just the cache entry"
    );
    reopened.shutdown().await;
}

// ── Slice 1b: multi-instance detection ────────────────────────────────

#[test]
fn open_returns_already_open_when_locked() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("locked.redb");

    // First open succeeds.
    let _first = TickerOrderIntentHandle::open(path.clone()).expect("first open");

    // Second open on the same path must surface AlreadyOpen — redb
    // holds an exclusive file lock inside a single process too.
    // We avoid `expect_err` because `TickerOrderIntentHandle: !Debug`.
    match TickerOrderIntentHandle::open(path.clone()) {
        Ok(_) => panic!("second open should have failed"),
        Err(super::IntentError::AlreadyOpen { path: p }) => {
            assert_eq!(p, path);
        }
        Err(other) => panic!("expected AlreadyOpen, got {other:?}"),
    }
    // The mailbox actor owns the `Database` on a separate thread, so
    // dropping `_first` only releases our handle clone. The other tests
    // exercise the full async shutdown sequence; here we only assert
    // the typed-error variant on the second open.
}

// ── Slice 1b: whole-file corruption recovery ──────────────────────────

#[test]
fn open_recovers_from_corrupt_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("ticker_state.redb");

    // Write junk bytes where redb expects a valid header.
    std::fs::write(&path, b"this is not a valid redb file, not at all!").unwrap();

    let handle = TickerOrderIntentHandle::open(path.clone())
        .expect("corrupt file should be recovered into a fresh DB");

    // The recovery toast must be queued.
    let toasts = handle.take_pending_startup_toasts();
    assert_eq!(toasts.len(), 1, "one recovery toast expected");
    assert!(
        toasts[0].contains("Order memory reset"),
        "toast message should describe the reset: got {:?}",
        toasts[0]
    );
    // A second drain is empty.
    assert!(handle.take_pending_startup_toasts().is_empty());

    // A fresh empty DB exists at the original path.
    assert!(path.exists(), "fresh DB should exist at original path");

    // The original (corrupt) file was renamed with a `.corrupt.<ts>`
    // suffix in the same directory.
    let entries: Vec<_> = std::fs::read_dir(tmp.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().into_string().unwrap_or_default())
        .collect();
    // Match without regex: "ticker_state.redb.corrupt." prefix + all
    // remaining characters in [0-9.].
    let aside_matches = |name: &str| {
        let prefix = "ticker_state.redb.corrupt.";
        if let Some(rest) = name.strip_prefix(prefix) {
            !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit() || b == b'.')
        } else {
            false
        }
    };
    assert!(
        entries.iter().any(|name| aside_matches(name)),
        "expected a .corrupt.<ts> sibling, got entries: {entries:?}"
    );

    // The recovered DB is actually usable.
    let outcome = handle.upsert(OrderIntentMsg::Upsert {
        symbol: SymbolKey::new("AAPL"),
        intent: Box::new(sample_intent("AAPL")),
        source: IntentSource::Panel,
    });
    assert!(matches!(outcome, UpsertOutcome::Applied { .. }));
}

// ── Slice 1b: disk-full backoff + force-shutdown bypass ───────────────

mod disk_full {
    use std::fmt;
    use std::io;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// In-memory `redb::StorageBackend` whose `write` call can be
    /// instructed to return `io::ErrorKind::StorageFull` after the
    /// N-th invocation. Used to simulate a full disk without touching
    /// the real filesystem.
    pub(super) struct DiskFullBackend {
        bytes: Mutex<Vec<u8>>,
        writes_before_fail: AtomicUsize,
        write_count: AtomicUsize,
        /// When set, every `write` past `writes_before_fail` returns
        /// `StorageFull`. Cleared to simulate "disk space freed".
        fail_armed: Arc<std::sync::atomic::AtomicBool>,
    }

    impl DiskFullBackend {
        pub(super) fn new(
            writes_before_fail: usize,
            fail_armed: Arc<std::sync::atomic::AtomicBool>,
        ) -> Self {
            Self {
                bytes: Mutex::new(Vec::new()),
                writes_before_fail: AtomicUsize::new(writes_before_fail),
                write_count: AtomicUsize::new(0),
                fail_armed,
            }
        }
    }

    impl fmt::Debug for DiskFullBackend {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("DiskFullBackend").finish()
        }
    }

    impl redb::StorageBackend for DiskFullBackend {
        fn len(&self) -> Result<u64, io::Error> {
            Ok(self.bytes.lock().unwrap().len() as u64)
        }

        fn read(&self, offset: u64, len: usize) -> Result<Vec<u8>, io::Error> {
            let buf = self.bytes.lock().unwrap();
            let start = offset as usize;
            let end = start.checked_add(len).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "overflow")
            })?;
            if end > buf.len() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    format!("read past end: {end} > {}", buf.len()),
                ));
            }
            Ok(buf[start..end].to_vec())
        }

        fn set_len(&self, len: u64) -> Result<(), io::Error> {
            let mut buf = self.bytes.lock().unwrap();
            buf.resize(len as usize, 0);
            Ok(())
        }

        fn sync_data(&self, _eventual: bool) -> Result<(), io::Error> {
            Ok(())
        }

        fn write(&self, offset: u64, data: &[u8]) -> Result<(), io::Error> {
            let n = self.write_count.fetch_add(1, Ordering::AcqRel) + 1;
            let threshold = self.writes_before_fail.load(Ordering::Acquire);
            if self.fail_armed.load(Ordering::Acquire) && n > threshold {
                return Err(io::Error::new(
                    io::ErrorKind::StorageFull,
                    "simulated disk full",
                ));
            }
            let mut buf = self.bytes.lock().unwrap();
            let end = offset as usize + data.len();
            if buf.len() < end {
                buf.resize(end, 0);
            }
            buf[offset as usize..end].copy_from_slice(data);
            Ok(())
        }
    }
}

#[test]
fn flush_requeues_on_disk_full_and_shutdown_guard_fires() {
    use std::sync::atomic::{AtomicBool, Ordering};

    // Start in the "disarmed" state so the initial open + table
    // creation succeed, then arm the failure and force a flush.
    let armed = Arc::new(AtomicBool::new(false));
    let backend = disk_full::DiskFullBackend::new(0, armed.clone());

    let hydrated = super::actor::open_and_hydrate_with_backend(backend)
        .expect("open with in-memory backend should succeed");
    let store = hydrated.store.clone();
    let db = hydrated.db.clone();
    let ctl = hydrated.ctl.clone();

    // Seed a dirty write and assert it exists in the dirty set.
    store.upsert(SymbolKey::new("AAPL"), sample_intent("AAPL"));
    assert_eq!(store.dirty_len(), 1);

    // Arm the failure so any subsequent write returns StorageFull.
    armed.store(true, Ordering::Release);

    let err = super::actor::flush_dirty(&db, &store, redb::Durability::Immediate)
        .expect_err("flush should fail with disk full");
    match err {
        super::actor::FlushError::DiskFull(_) => {}
        other => panic!("expected DiskFull, got {other:?}"),
    }
    // Dirty set preserved so the next flush attempt will retry.
    assert_eq!(store.dirty_len(), 1, "failed flush must re-queue dirty set");

    // Backoff-delay progression (1s → 2s → 5s → 10s → 30s, then
    // capped at 30s on every subsequent failure).
    assert_eq!(
        super::actor::FlushCtl::backoff_delay(1),
        std::time::Duration::from_secs(1)
    );
    assert_eq!(
        super::actor::FlushCtl::backoff_delay(2),
        std::time::Duration::from_secs(2)
    );
    assert_eq!(
        super::actor::FlushCtl::backoff_delay(3),
        std::time::Duration::from_secs(5)
    );
    assert_eq!(
        super::actor::FlushCtl::backoff_delay(4),
        std::time::Duration::from_secs(10)
    );
    assert_eq!(
        super::actor::FlushCtl::backoff_delay(5),
        std::time::Duration::from_secs(30)
    );
    assert_eq!(
        super::actor::FlushCtl::backoff_delay(99),
        std::time::Duration::from_secs(30),
        "backoff caps at the last entry"
    );

    // A subsequent flush against the same poisoned `Database` keeps
    // re-queuing dirty rows; redb's `StorageError::PreviousIo` latches
    // the database into a read-only state until it is reopened, which
    // is the exact behavior the disk-full modal expects: "free space
    // and relaunch." Verify the dirty set still survives the second
    // failure (Slice 1b's contract is preservation, not recovery).
    armed.store(false, Ordering::Release);
    let second_err = super::actor::flush_dirty(&db, &store, redb::Durability::Immediate)
        .expect_err("redb stays poisoned after a write failure");
    match second_err {
        super::actor::FlushError::Other(_) | super::actor::FlushError::DiskFull(_) => {}
    }
    assert_eq!(
        store.dirty_len(),
        1,
        "second failed flush must also re-queue the dirty set"
    );

    // Avoid leaking the flush thread the test did not spawn.
    drop(ctl);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_refuses_when_disk_full_and_force_bypasses() {
    // White-box test of the shutdown guard: we mutate
    // `FlushCtl::disk_full_failures` through a crate-private test
    // helper on the handle (the real `flush_loop` sets the same
    // fields from the background thread), then assert the
    // `Shutdown { force: false }` handler refuses and latches the
    // modal, and `Shutdown { force: true }` bypasses the guard.
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("shutdown_disk_full.redb");
    let handle = TickerOrderIntentHandle::open(path.clone()).expect("open");

    handle.__test_force_disk_full_state();

    // Plain shutdown must refuse and latch the modal message.
    handle.shutdown_best_effort_non_forced().await;
    let modal = handle
        .pending_modal_message()
        .expect("modal should be latched after refused shutdown");
    assert!(
        modal.contains("disk full"),
        "modal text should describe disk-full: {modal:?}"
    );

    // Force-shutdown drops the actor anyway. After it returns the
    // handle is inert; the test just asserts the call completes
    // without hanging.
    handle.shutdown_force().await;
    // The modal message is not auto-cleared — Slice 2 drains it
    // explicitly. Verify the drain accessor works.
    let drained = handle.take_pending_modal_message();
    assert!(drained.is_some(), "drain should return the latched modal");
    assert!(
        handle.take_pending_modal_message().is_none(),
        "second drain should be empty"
    );
}
