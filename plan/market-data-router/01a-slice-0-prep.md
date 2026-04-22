# Slice 0 — Preparation

**Goal.** Do the non-refactor plumbing that BR-16 and BR-17 require before S1 can land cleanly. Each item below is its own commit; the whole slice runs green on both workspaces before S1 starts.

## Scope

### A. Move `mailbox_processor` from desktop to root workspace (BR-16)

The router (S5) lives in the root workspace and needs `mailbox_processor`. The crate currently lives at `desktop/win/crates/mailbox_processor/` and is referenced by `midas-store`. Moving it up keeps the root-workspace dep path `../mailbox_processor` clean and avoids cross-workspace path deps.

Steps:
1. `git mv desktop/win/crates/mailbox_processor crates/mailbox_processor` (or equivalent move on Windows).
2. Update root `Cargo.toml` `[workspace] members` → add `crates/mailbox_processor`.
3. Remove `mailbox_processor` from `desktop/win/Cargo.toml` `[workspace] members`.
4. Update `desktop/win/crates/midas-store/Cargo.toml`: change `mailbox_processor = { path = "../mailbox_processor" }` to `mailbox_processor = { path = "../../../../crates/mailbox_processor" }` (or workspace dep, depending on the desktop workspace's path layout).
5. Regenerate both `Cargo.lock`s (root + desktop).
6. Full verification:
   - `cargo test --workspace` at root.
   - `cd desktop/win && cargo test --workspace`.
   - `cargo clippy --workspace -- -D warnings` on both.

**Commit:** `chore(workspace): move mailbox_processor from desktop to root workspace`.

### A.5. Verify tokio ≥ 1.29 in both workspaces (NB-3)

The router's M-4 auto-exit logic for tick publishers must check `watch::Sender::receiver_count()` so watchlist-only consumers (who hold `watch::Receiver<Quote>` but no broadcast receiver) keep the upstream tick publisher alive. `watch::Sender::receiver_count` was added in tokio 1.29.

Steps:
1. `cargo tree -p tokio -i` in root workspace — confirm resolved tokio version ≥ 1.29. Bump the `[workspace.dependencies]` tokio entry if lower.
2. Repeat in `desktop/win` workspace.
3. `cargo test --workspace` on both to confirm the bump is clean.

**Commit:** `chore(workspace): bump tokio to >=1.29 for watch::Sender::receiver_count`.

### B. Pin `ibapi = "=2.10"` (BR-10)

In `crates/midas-broker/Cargo.toml`, change the `ibapi` dep to an exact-version pin:

```toml
ibapi = "=2.10"
```

This prevents silent drift when cargo resolves a newer compatible release that may have reshaped the `market_data` builder or historical API (the plan is coded against 2.10's surface specifically). A separate S4 commit may un-pin if 2.11+ proves API-compatible after auditing.

**Commit:** `chore(midas-broker): pin ibapi to =2.10 for router-refactor planning`.

### C. Scratch POC of iced 0.14 subscription channel API (BR-17)

Before S7 commits to `iced::subscription::channel`, prove the API shape matches what the codebase currently uses. The existing app uses `Subscription::run_with` and `Subscription::run`; the plan assumes a `channel(key, cap, closure)` variant is available.

Steps:
1. Add scratch file `desktop/win/crates/midas-app/src/app/scratch_subscription_poc.rs`.
2. Implement a no-op subscription over a trivial `tokio::sync::broadcast::Receiver<u32>` that sends a `Message::PocTick(u32)` per batch.
3. Wire it into `MidasApp::subscription()` under `#[cfg(feature = "_subscription_poc")]`.
4. `cargo build -p midas-app --features _subscription_poc` — must compile.
5. `cargo run -p midas-app --features _subscription_poc` — verify tick messages fire (smoke).
6. Document the exact `iced::subscription` module path and signature in a comment inside the scratch file.
7. **Delete** the scratch file + the feature flag in a follow-up commit (keep the insight in a short note in S7's plan).

If `iced::subscription::channel` does NOT exist in 0.14 or has a different signature, update S7 + S8 plan files to match the real API before proceeding.

**Commit (scratch):** `chore(midas-app): scratch subscription POC for iced 0.14 verification (to be deleted)`.
**Commit (delete):** `chore(midas-app): remove subscription POC after API verified`.

## Acceptance

- Root + desktop workspaces both green on `cargo test --workspace` and `cargo clippy --workspace -- -D warnings`.
- `mailbox_processor` visible from root workspace path deps. Desktop crates (`midas-store`) still compile unchanged.
- `tokio` resolves to ≥ 1.29 in both workspaces (NB-3).
- `ibapi` version locked at `=2.10`.
- iced subscription POC compiled + ran; API shape documented; scratch deleted.

## Risks

- `midas-store`'s path to `mailbox_processor` must remain correct after the move. Confirm by running `cargo build -p midas-store` in desktop workspace.
- `Cargo.lock` regeneration may change other dep versions; scrutinize the diff for unrelated upgrades and cherry-pick back.
- If the iced POC reveals a missing `channel` helper, S7/S8 need to adopt `Subscription::run` with manual unfold — update those slice docs before S3/S4 start.

## Out of scope

- No changes to router, provider traits, or sim/IB backends. All of that is S1+.

## iced 0.14 API verified

Scratch POC (`src/app/scratch_subscription_poc.rs`, since deleted) confirmed the channel/subscription shape against `iced = "0.14"` + `iced_futures = "0.14"`. Summary for S7/S8 implementers:

- **No `iced::subscription::channel` re-export.** The `subscription` module is NOT re-exported from `iced`; only the `Subscription` type is (`iced::Subscription`).
- **Channel helper lives at `iced::stream::channel`** (re-export of `iced_futures::stream::channel`). Signature:
  ```rust
  pub fn channel<T>(
      size: usize,
      f: impl AsyncFnOnce(futures::channel::mpsc::Sender<T>),
  ) -> impl Stream<Item = T>
  ```
  The closure receives an `mpsc::Sender<T>` (from `futures::channel::mpsc`, re-exported via `iced::futures`). Use `.send(msg).await` to emit, where `SinkExt` is in scope via `iced::futures::SinkExt`.
- **To lift a `Stream` into a `Subscription`** use:
  - `Subscription::run(builder: fn() -> S)` — unkeyed.
  - `Subscription::run_with(data: D, builder: fn(&D) -> S)` where `D: Hash + 'static` — keyed; `data` + the `fn` pointer form the subscription identity. Both `run` and `run_with` require a **`fn` pointer, not a closure**, so per-instance state (e.g. a live `broadcast::Receiver`) must be resolved inside the builder from a keyed registry, not captured.
- **`Subscription::run_with_id` does NOT exist** in iced 0.14. Options for adding key/identity:
  - Pass the key through `run_with`'s `data: D: Hash` parameter.
  - Call `.with(value)` on the resulting `Subscription` to append an identity value (`value: Hash + Clone + Send + Sync + 'static`), producing a `Subscription<(A, T)>`.

**Canonical shape for S7/S8 per-chart subscriptions:**

```rust
fn chart_stream(key: &(Symbol, Timeframe)) -> impl iced::futures::Stream<Item = Message> {
    let key = key.clone();
    iced::stream::channel(16, async move |mut output| {
        // Resolve router handle from a global/static registry keyed by `key`.
        // Loop: recv from broadcast + coalesce to ~16 ms frames + output.send(...).
    })
}

Subscription::run_with((symbol, tf), chart_stream)
```

Since `run_with` requires a `fn` pointer, `broadcast::Receiver<_>` cannot be captured directly. S7 must resolve the receiver inside the builder via a registry (e.g., a static `DashMap<Key, SubscriptionHandle>` populated by the router). This matches the "per-chart SubscriptionHandle" pattern already described in the S7 design.

The `async move |mut output|` closure form (AsyncFnOnce) compiles cleanly on our current stable toolchain.
