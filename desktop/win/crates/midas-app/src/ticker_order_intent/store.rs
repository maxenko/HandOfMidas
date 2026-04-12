//! In-memory cache for [`super::TickerOrderIntent`].
//!
//! A thin wrapper around a `parking_lot::RwLock<HashMap<_,_>>` + a
//! dirty set. All reads are lock-free after the `Arc` clone; all
//! writes are guarded by a short write-lock. There is deliberately no
//! async here — iced's `update()` loop is sync, so the cache must be
//! too.
//!
//! # Dead-code allowance
//!
//! Slice 1a froze the public surface of the store (`all_symbols`,
//! `generation`, etc.) so Slices 3–5 can consume it without reopening
//! the module. Suppress dead-code at the file level rather than
//! sprinkling per-method attributes — this is intentional frozen API.
#![allow(dead_code)]

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};

use crate::annotation_store::SymbolKey;

use super::TickerOrderIntent;

/// Outcome of a [`TickerOrderIntentStore::upsert`] call.
///
/// Returned synchronously so the reducer can decide whether to
/// kick off downstream side-effects (annotation sync, panel refresh).
/// `NoOp` lets identical updates short-circuit without marking the
/// symbol dirty — a second line of defense against feedback loops.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UpsertOutcome {
    /// The cache changed. `generation` is the post-write counter —
    /// tests observe this to confirm the write actually landed.
    Applied {
        /// Monotonic store-wide generation, bumped on every applied write.
        generation: u64,
    },
    /// The cache was already identical; nothing was written and the
    /// symbol was **not** added to the dirty set.
    NoOp {
        /// Why the write was skipped.
        reason: super::NoOpReason,
    },
}

/// Thread-safe in-memory cache of per-symbol order intents.
///
/// Owned by the [`super::actor::TickerOrderIntentActor`] and exposed
/// read-only to the rest of the app through an `Arc`. Writes happen
/// either through the actor (from messages) or directly through
/// `TickerOrderIntentHandle::upsert` (sync write-through).
pub struct TickerOrderIntentStore {
    /// The actual map. Short-held write locks only; read locks are
    /// released immediately after cloning the `Arc`.
    cache: RwLock<HashMap<SymbolKey, Arc<TickerOrderIntent>>>,
    /// Symbols that have been written but not yet flushed to disk.
    dirty: Mutex<HashSet<SymbolKey>>,
    /// Monotonic write counter — tests use this to verify writes.
    generation: AtomicU64,
}

impl TickerOrderIntentStore {
    /// Construct an empty store.
    pub fn new() -> Self {
        Self {
            cache: RwLock::new(HashMap::new()),
            dirty: Mutex::new(HashSet::new()),
            generation: AtomicU64::new(0),
        }
    }

    /// Read the current intent for a symbol, if any.
    ///
    /// Lock-free after the clone of the `Arc`; the caller can then
    /// read every field without holding any internal lock.
    pub fn snapshot(&self, symbol: &SymbolKey) -> Option<Arc<TickerOrderIntent>> {
        self.cache.read().get(symbol).cloned()
    }

    /// List every symbol that currently has an intent in the cache.
    ///
    /// Used on startup so the flush thread can write any "loaded from
    /// disk but never touched" rows with the right durability.
    pub fn all_symbols(&self) -> Vec<SymbolKey> {
        self.cache.read().keys().cloned().collect()
    }

    /// Insert or replace the intent for a symbol.
    ///
    /// If the new value is byte-identical to the cached one, returns
    /// [`UpsertOutcome::NoOp`] without touching the dirty set. Otherwise
    /// replaces the entry, adds the symbol to the dirty set, bumps
    /// the generation, and returns [`UpsertOutcome::Applied`].
    pub fn upsert(&self, symbol: SymbolKey, intent: TickerOrderIntent) -> UpsertOutcome {
        // Equality check held under a read lock so concurrent readers
        // are not blocked on the common "no-op" path.
        if let Some(existing) = self.cache.read().get(&symbol) {
            if **existing == intent {
                return UpsertOutcome::NoOp {
                    reason: super::NoOpReason::IdenticalToCache,
                };
            }
        }

        let mut cache = self.cache.write();
        // Re-check under the write lock to avoid a racing parallel write.
        if let Some(existing) = cache.get(&symbol) {
            if **existing == intent {
                return UpsertOutcome::NoOp {
                    reason: super::NoOpReason::IdenticalToCache,
                };
            }
        }
        cache.insert(symbol.clone(), Arc::new(intent));
        drop(cache);

        self.dirty.lock().insert(symbol);
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        UpsertOutcome::Applied { generation }
    }

    /// Drain the dirty set, returning every (symbol, snapshot) pair
    /// that needs persisting. Snapshots are `Arc` clones — cheap.
    pub fn drain_dirty(&self) -> Vec<(SymbolKey, Arc<TickerOrderIntent>)> {
        let dirty: Vec<SymbolKey> = {
            let mut guard = self.dirty.lock();
            guard.drain().collect()
        };
        let cache = self.cache.read();
        dirty
            .into_iter()
            .filter_map(|s| cache.get(&s).map(|arc| (s, arc.clone())))
            .collect()
    }

    /// Remove a symbol from the cache entirely. Returns `true` if the
    /// symbol existed, `false` otherwise. The removed symbol is also
    /// added to the dirty set so the flush thread deletes its row.
    pub fn forget(&self, symbol: &SymbolKey) -> bool {
        let removed = self.cache.write().remove(symbol).is_some();
        if removed {
            self.dirty.lock().insert(symbol.clone());
            self.generation.fetch_add(1, Ordering::AcqRel);
        }
        removed
    }

    /// Current generation counter. Tests use this to observe whether
    /// an upsert actually mutated the cache.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Insert without marking dirty. Used by the actor on startup to
    /// seed the cache from disk without scheduling a pointless flush.
    pub(crate) fn seed(&self, symbol: SymbolKey, intent: TickerOrderIntent) {
        self.cache.write().insert(symbol, Arc::new(intent));
    }

    /// Re-mark a batch of symbols as dirty. Used by the flush loop on
    /// disk-full failure so the un-persisted writes get another chance
    /// on the next flush attempt. Symbols whose cache entries have been
    /// evicted (e.g. forgotten) since the drain are silently skipped.
    pub(crate) fn re_mark_dirty<I>(&self, symbols: I)
    where
        I: IntoIterator<Item = SymbolKey>,
    {
        let cache = self.cache.read();
        let mut dirty = self.dirty.lock();
        for s in symbols {
            if cache.contains_key(&s) {
                dirty.insert(s);
            }
        }
    }

    /// Number of symbols currently awaiting a flush. Exposed for tests
    /// that assert a failed commit re-queued its batch.
    pub(crate) fn dirty_len(&self) -> usize {
        self.dirty.lock().len()
    }
}

impl Default for TickerOrderIntentStore {
    fn default() -> Self {
        Self::new()
    }
}
