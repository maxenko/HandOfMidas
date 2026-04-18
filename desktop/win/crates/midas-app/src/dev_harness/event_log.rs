//! JSONL event log for every `TickerMsg` processed by the app.
//!
//! Ground-truth trace of domain transitions. Read by `wait_for_event`
//! over the socket, read by hand via `tail -f .devloop/events.jsonl`.
//!
//! ## Design
//!
//! `TickerMsg` and `TickerEffect` do not implement `Serialize` today
//! and the plan defers the full derive cascade to Step 7. For Step 3
//! we log a reduced, human-readable representation:
//!
//! - `variant` — the enum variant name, used by `wait_for_event`
//!   matching (e.g. `"SetLegPrice"`).
//! - `debug` — the `Debug`-formatted payload for manual inspection.
//!
//! When Step 7 lands full serde derives, we upgrade the `msg` / `effects`
//! fields to carry structured JSON without breaking the wire schema:
//! `wait_for_event` still matches on `variant`.
//!
//! ## Concurrency
//!
//! The log is shared between the iced UI thread (appender) and the
//! tokio listener tasks (waiters). File writes sit behind a
//! `parking_lot::Mutex`; the cursor is an `AtomicU64`; waiters park on
//! a `tokio::sync::Notify` and a small ring buffer.

use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use chrono::Utc;
use parking_lot::Mutex;
use tokio::sync::Notify;

use crate::ticker_state::{TickerEffect, TickerMsg};

use super::variant_names;

/// Maximum live event log size before rotation.
const ROTATE_AT_BYTES: u64 = 100 * 1024 * 1024;

/// How many recent entries to keep in-memory for `wait_for_event`.
const RING_CAPACITY: usize = 1024;

/// Global event log. `None` outside the `dev_harness` feature gate.
static EVENT_LOG: OnceLock<Arc<EventLog>> = OnceLock::new();

/// Install the global. Fine to call multiple times — subsequent calls
/// are no-ops (OnceLock semantics). Returns `true` on the first call.
pub fn init_global(log: Arc<EventLog>) -> bool {
    EVENT_LOG.set(log).is_ok()
}

/// Fetch the global event log, `None` if uninitialised. Every caller
/// guards against the `None` case — the harness can start listening
/// (ping works) before the log is fully wired.
pub fn try_global() -> Option<Arc<EventLog>> {
    EVENT_LOG.get().cloned()
}

/// An event log backed by a JSONL file + an in-memory ring buffer for
/// fast `wait_for_event` matching.
pub struct EventLog {
    path: PathBuf,
    cursor: AtomicU64,
    notify: Notify,
    inner: Mutex<Inner>,
}

struct Inner {
    file: BufWriter<File>,
    current_size: u64,
    rotation_count: u32,
    recent: VecDeque<RecentEntry>,
}

/// Slim record retained in memory so `wait_for_event` doesn't re-read
/// the file.
#[derive(Debug, Clone)]
struct RecentEntry {
    cursor: u64,
    variant: &'static str,
}

impl EventLog {
    /// Open (and truncate) the event log at `path`. The containing
    /// directory must already exist — see `dev_harness::init`.
    pub fn new(path: impl Into<PathBuf>) -> std::io::Result<Self> {
        let path = path.into();
        let file = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)?;
        let inner = Inner {
            file: BufWriter::new(file),
            current_size: 0,
            rotation_count: 0,
            recent: VecDeque::with_capacity(RING_CAPACITY),
        };
        Ok(Self {
            path,
            cursor: AtomicU64::new(0),
            notify: Notify::new(),
            inner: Mutex::new(inner),
        })
    }

    /// Current log cursor (event count since process start).
    pub fn cursor(&self) -> u64 {
        self.cursor.load(Ordering::Acquire)
    }

    /// Append one `TickerMsg` + its effects. Returns the new cursor.
    ///
    /// Market-data / tick-rate variants are filtered here to keep the
    /// file bounded under IB-attached sessions.
    pub fn append_ticker(&self, symbol: &str, msg: &TickerMsg, effects: &[TickerEffect]) -> u64 {
        if variant_names::is_tick_rate(msg) {
            return self.cursor();
        }

        let cursor = self.cursor.fetch_add(1, Ordering::AcqRel) + 1;
        let variant = variant_names::ticker_msg_variant(msg);

        let entry = serde_json::json!({
            "log_cursor": cursor,
            "ts_mono_ns": mono_ns(),
            "ts_wall": Utc::now().to_rfc3339(),
            "symbol": symbol,
            "variant": variant,
            "debug": format!("{msg:?}"),
            "effects": effects
                .iter()
                .map(|e| serde_json::json!({
                    "variant": variant_names::ticker_effect_variant(e),
                    "debug": format!("{e:?}"),
                }))
                .collect::<Vec<_>>(),
        });

        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("devloop: event log serialise failed: {e}");
                return cursor;
            }
        };

        {
            let mut inner = self.inner.lock();
            if let Err(e) = writeln!(inner.file, "{line}") {
                tracing::error!("devloop: event log write failed: {e}");
                return cursor;
            }
            if let Err(e) = inner.file.flush() {
                tracing::warn!("devloop: event log flush failed: {e}");
            }
            inner.current_size += line.len() as u64 + 1;

            if inner.recent.len() >= RING_CAPACITY {
                inner.recent.pop_front();
            }
            inner.recent.push_back(RecentEntry { cursor, variant });

            if inner.current_size >= ROTATE_AT_BYTES {
                if let Err(e) = rotate(&self.path, &mut inner) {
                    tracing::warn!("devloop: event log rotate failed: {e}");
                }
            }
        }

        self.notify.notify_waiters();
        cursor
    }

    /// Append one `BrokerEvent` from the broker engine's broadcast
    /// stream. Uses `Debug`-formatted payload because `BrokerEvent`
    /// doesn't derive `Serialize`.
    ///
    /// Market-data tick-rate events (`Tick`, `RealtimeBar`, `BarUpdated`,
    /// `DepthUpdate`) are filtered here — same reason as ticker-msg
    /// filtering.
    pub fn append_broker(&self, event: &midas_broker::BrokerEvent) {
        let variant = variant_names::broker_event_variant(event);
        if variant_names::is_tick_rate_broker(event) {
            return;
        }

        let cursor = self.cursor.fetch_add(1, Ordering::AcqRel) + 1;

        let symbol = variant_names::broker_event_symbol(event).unwrap_or_default();

        let entry = serde_json::json!({
            "log_cursor": cursor,
            "ts_mono_ns": mono_ns(),
            "ts_wall": Utc::now().to_rfc3339(),
            "source": "broker",
            "symbol": symbol,
            "variant": variant,
            "debug": format!("{event:?}"),
        });

        let line = match serde_json::to_string(&entry) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("devloop: broker event log serialise failed: {e}");
                return;
            }
        };

        {
            let mut inner = self.inner.lock();
            if let Err(e) = writeln!(inner.file, "{line}") {
                tracing::error!("devloop: event log write failed: {e}");
                return;
            }
            if let Err(e) = inner.file.flush() {
                tracing::warn!("devloop: event log flush failed: {e}");
            }
            inner.current_size += line.len() as u64 + 1;

            if inner.recent.len() >= RING_CAPACITY {
                inner.recent.pop_front();
            }
            inner.recent.push_back(RecentEntry { cursor, variant });

            if inner.current_size >= ROTATE_AT_BYTES {
                if let Err(e) = rotate(&self.path, &mut inner) {
                    tracing::warn!("devloop: event log rotate failed: {e}");
                }
            }
        }

        self.notify.notify_waiters();
    }

    /// Wait for an event whose variant matches `target_variant` and
    /// whose cursor is strictly greater than `since_cursor`. Returns the
    /// matched cursor, or `None` on timeout.
    pub async fn wait_for_event(
        &self,
        target_variant: &str,
        since_cursor: u64,
        timeout: Duration,
    ) -> Option<u64> {
        let deadline = Instant::now() + timeout;

        loop {
            // Subscribe to notifications BEFORE scanning the ring, so
            // we don't miss an event that arrives between scan and park.
            let notified = self.notify.notified();
            tokio::pin!(notified);

            if let Some(cursor) = self.scan_ring(target_variant, since_cursor) {
                return Some(cursor);
            }

            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return None;
            }

            tokio::select! {
                _ = &mut notified => continue,
                _ = tokio::time::sleep(remaining) => return None,
            }
        }
    }

    fn scan_ring(&self, target_variant: &str, since_cursor: u64) -> Option<u64> {
        let inner = self.inner.lock();
        inner
            .recent
            .iter()
            .find(|e| e.cursor > since_cursor && e.variant == target_variant)
            .map(|e| e.cursor)
    }
}

fn rotate(path: &Path, inner: &mut Inner) -> std::io::Result<()> {
    // Flush + drop current writer before renaming.
    inner.file.flush()?;
    let stamp = Utc::now().format("%Y%m%dT%H%M%S");
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("events");
    let rotated = parent.join(format!("{stem}-{stamp}.jsonl"));

    // Close by replacing with a temporary writer first.
    let placeholder = OpenOptions::new()
        .write(true)
        .truncate(true)
        .create(true)
        .open(path)?;
    let old_writer = std::mem::replace(&mut inner.file, BufWriter::new(placeholder));
    drop(old_writer);

    // Now the file handle is released — rename the placeholder's path.
    if let Err(e) = std::fs::rename(path, &rotated) {
        tracing::warn!("devloop: rotate rename failed: {e}");
    } else {
        tracing::info!("devloop: event log rotated to {}", rotated.display());
    }

    // Open fresh.
    let fresh = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(path)?;
    inner.file = BufWriter::new(fresh);
    inner.current_size = 0;
    inner.rotation_count += 1;
    Ok(())
}

fn mono_ns() -> u64 {
    use std::time::SystemTime;
    // Monotonic-ish: we want a per-process counter stamp for ordering.
    // SystemTime is close enough and avoids bringing in a quanta-like
    // dep. Loses a bit of precision vs. Instant but serializes easily.
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}
