# Slice 2 — Candle session hooks

**Goal.** Add `session_kind: Option<SessionKind>` to `Bar` and `CandleData::session(idx) -> SessionKind` to the trait. This is the small-but-load-bearing type-layer change that rendering + aggregator slices all depend on.

## Scope

### `midas-broker-core::market_data::Bar`

```rust
pub struct Bar {
    // ...existing fields...
    pub session_kind: Option<SessionKind>,  // None => unspecified (producer didn't classify)
}
```

`SessionKind` is re-exported from `midas-calendar` — or duplicated as a minimal enum in `midas-broker-core` to avoid the dep. Decision: **duplicate** a minimal `SessionKind` in `midas-broker-core::market_data::session`, then `midas-calendar` re-exports it. Rationale: `midas-broker-core` is the lower-level crate; keeping it dep-free from `midas-calendar` preserves the topology. The duplication is 6 enum variants — acceptable.

```rust
// midas-broker-core::market_data::session
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum SessionKind {
    #[default] Regular, PreMarket, PostMarket, Break, Overnight, Closed,
}
```

`midas-calendar` does:
```rust
pub use midas_broker_core::market_data::session::SessionKind;
```

Serde-compat: existing persisted `Bar` records get `session_kind: None` via `#[serde(default)]`. Verify no breakage with `01a-slice-0` fixtures.

### `midas-core::CandleData`

```rust
pub trait CandleData {
    // ...existing methods...

    /// Session classification for the candle at `idx`.
    /// Default: Regular (pre-refactor behaviour, compatible with non-session-aware producers).
    fn session(&self, _idx: usize) -> SessionKind {
        SessionKind::Regular
    }
}
```

Import path: `midas_core` re-exports `SessionKind` from `midas-broker-core` via a `midas-core::market_data` module.

### `midas-core::CandleBuffer`

Add a parallel vec:

```rust
pub struct CandleBuffer {
    // ...existing vecs...
    pub sessions: Vec<u8>,   // SessionKind as u8
    version: AtomicU64,
}

impl CandleBuffer {
    pub fn push_with_session(&mut self, ts: i64, o: f32, h: f32, l: f32, c: f32, v: u32, session: SessionKind) {
        self.push(ts, o, h, l, c, v);
        self.sessions.push(session as u8);
    }

    pub fn apply_bar_with_session(&mut self, ts_open_ms: i64, o: f32, h: f32, l: f32, c: f32, v: u32, session: SessionKind) {
        // same as apply_bar but updates/pushes session too
    }
}

impl CandleData for CandleBuffer {
    fn session(&self, idx: usize) -> SessionKind {
        self.sessions.get(idx).copied()
            .and_then(|b| u8_to_session_kind(b))
            .unwrap_or(SessionKind::Regular)
    }
}
```

Keep the existing `push` / `apply_bar` methods as pass-throughs that default to `SessionKind::Regular`. Existing call sites don't change behaviour.

### `from_bars` constructor (needed by S0)

```rust
impl CandleBuffer {
    pub fn from_bars(bars: &[Bar]) -> Self {
        let mut buf = CandleBuffer::with_capacity(bars.len());
        for bar in bars {
            buf.push_with_session(
                bar.ts_open.timestamp_millis(),
                bar.o as f32, bar.h as f32, bar.l as f32, bar.c as f32,
                bar.volume.min(u32::MAX as u64) as u32,
                bar.session_kind.unwrap_or_default(),
            );
        }
        buf
    }
}
```

## Tests

- `Bar` serde roundtrip with and without `session_kind`.
- `CandleBuffer::push_with_session` + `session(idx)` returns the stored kind.
- `CandleBuffer::from_bars` round-trips a `Vec<Bar>` → `CandleBuffer` → back to bars with session preserved.
- Legacy `CandleBuffer::push` path: `session(idx)` returns `Regular` for every pushed candle.
- Existing `apply_bar` tests still pass (default `Regular`).

## Acceptance

- `cargo test --workspace` green on both workspaces.
- `cargo clippy --workspace --all-targets -- -D warnings` clean.
- `cargo fmt --all`.
- Existing `Bar` / `CandleBuffer` tests unchanged.

## Commit

Single commit: `feat(core): add SessionKind to Bar + CandleData trait + CandleBuffer`.

## Risks

- Mmap binary format for `CandleBuffer` on-disk (if any): adding `sessions: Vec<u8>` changes the serialised layout. Verify no persisted fixtures break; add a format-version bump if needed. The plan assumes the on-disk format is JSON-like serde (not bincode/mmap); verify during implementation.
- Trait default method on `CandleData::session` preserves backward compat — every existing `impl CandleData for X` without an override gets `SessionKind::Regular` for free.
