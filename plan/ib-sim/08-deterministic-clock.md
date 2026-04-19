# Stage 08 — Deterministic Clock + Event Scheduler

*Virtual time, the event scheduler priority queue, and the three clock modes (real / accelerated / virtual). Every time-dependent behavior in the sim routes through the `Clock` trait.*

**Depends on**: 01 (scaffold)
**Blocks**: every stage that schedules future events (03, 04, 05, 06)
**Parallel-safe with**: 02, 07

## Scope

Build the time abstraction layer. Provides:

- A `Clock` trait with `now()`, `sleep_until()`, `advance()` (virtual only).
- Three implementors: `RealClock`, `AcceleratedClock`, `VirtualClock`.
- An `EventScheduler` that orders future events in virtual time and hands them to the engine actor.
- A `VirtualInstant` type isolating tests from `std::time::Instant`'s real-wall-clock behavior.

## The `Clock` trait

```rust
#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> VirtualInstant;
    async fn sleep_until(&self, deadline: VirtualInstant);
    async fn sleep(&self, duration: Duration);
    fn mode(&self) -> ClockMode;
}

pub enum ClockMode { Real, Accelerated(f64), Virtual }

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct VirtualInstant(Duration); // since session start

impl VirtualInstant {
    pub fn from_millis(ms: u64) -> Self { Self(Duration::from_millis(ms)) }
    pub fn saturating_sub(self, other: Self) -> Duration { /* ... */ }
    pub fn as_wall_time(self, epoch: chrono::DateTime<chrono::Utc>) -> chrono::DateTime<chrono::Utc> { /* ... */ }
}
```

### `RealClock`

Thin wrapper over `tokio::time::Instant`. For the default dev-loop scenario: sim runs at real-time speed, humans interact at human speed. `now()` returns `Instant::now()` minus session start. `sleep_until(t)` delegates to `tokio::time::sleep_until(real_t)`.

### `AcceleratedClock`

Wraps `RealClock` with a time multiplier:

```rust
pub struct AcceleratedClock {
    base: RealClock,
    multiplier: f64, // 10.0 = 10× faster than real-time
}

impl Clock for AcceleratedClock {
    fn now(&self) -> VirtualInstant {
        let real_elapsed = self.base.now().0;
        VirtualInstant(real_elapsed.mul_f64(self.multiplier))
    }
    async fn sleep_until(&self, deadline: VirtualInstant) {
        let real_deadline = Duration::from_secs_f64(deadline.0.as_secs_f64() / self.multiplier);
        self.base.sleep_until(VirtualInstant(real_deadline)).await;
    }
}
```

Useful for demos ("show what a 9:30-10:00 session looks like in 3 minutes") and for running long scenarios in bounded real time.

### `VirtualClock`

For CI and integration tests. Time only advances when explicitly requested.

```rust
pub struct VirtualClock {
    state: Arc<Mutex<VirtualClockState>>,
}

struct VirtualClockState {
    now: VirtualInstant,
    waiters: BinaryHeap<Reverse<Waiter>>, // priority queue on deadline
}

struct Waiter {
    deadline: VirtualInstant,
    waker: tokio::sync::oneshot::Sender<()>,
}

impl Clock for VirtualClock {
    fn now(&self) -> VirtualInstant {
        self.state.lock().unwrap().now
    }

    async fn sleep_until(&self, deadline: VirtualInstant) {
        let (tx, rx) = tokio::sync::oneshot::channel();
        {
            let mut state = self.state.lock().unwrap();
            if state.now >= deadline {
                // Already past; return immediately
                return;
            }
            state.waiters.push(Reverse(Waiter { deadline, waker: tx }));
        }
        let _ = rx.await;
    }
}

impl VirtualClock {
    /// Advance to the deadline of the next waiter (or `until` if sooner).
    pub fn advance(&self, until: VirtualInstant) {
        let mut state = self.state.lock().unwrap();
        while let Some(Reverse(waiter)) = state.waiters.peek() {
            if waiter.deadline > until { break; }
            let Reverse(waiter) = state.waiters.pop().unwrap();
            state.now = waiter.deadline;
            let _ = waiter.waker.send(());
        }
        state.now = state.now.max(until);
    }

    pub fn advance_to_next_event(&self) {
        let mut state = self.state.lock().unwrap();
        if let Some(Reverse(waiter)) = state.waiters.pop() {
            state.now = waiter.deadline;
            let _ = waiter.waker.send(());
        }
    }
}
```

Two advance modes:
- `advance(until)` — fast-forward to a specific virtual time, firing every waiter up to it
- `advance_to_next_event()` — fire just the next due event (useful for step-by-step debugging)

### Integration with `tokio::time::pause`

In tests we can also use tokio's built-in paused clock:

```rust
#[tokio::test(start_paused = true)]
async fn my_test() {
    let clock = VirtualClock::new();
    // ... scenario ...
    tokio::time::advance(Duration::from_secs(60)).await;
    clock.advance(VirtualInstant::from_millis(60_000));
}
```

The two clocks are kept in sync by the test harness. Production code only sees the `Clock` trait.

## Event scheduler

The sim's engine has a priority queue of scheduled events: "emit this tick in 50ms," "complete this order fill in 200ms," "emit this farm-status bulletin at 09:35:00." One scheduler, all events, deterministic ordering.

```rust
pub struct EventScheduler {
    queue: BinaryHeap<Reverse<ScheduledEvent>>,
    clock: Arc<dyn Clock>,
}

pub struct ScheduledEvent {
    pub deadline: VirtualInstant,
    pub seq: u64, // monotonic tie-breaker for events at the same instant
    pub action: EngineAction,
}

impl EventScheduler {
    pub fn schedule(&mut self, deadline: VirtualInstant, action: EngineAction) {
        let seq = self.next_seq();
        self.queue.push(Reverse(ScheduledEvent { deadline, seq, action }));
    }

    /// Returns the deadline of the next event without popping it. Pairs with
    /// caller-side `clock.sleep_until(deadline)` to keep scheduler cancel-safe.
    pub fn peek_deadline(&self) -> Option<VirtualInstant> {
        self.queue.peek().map(|Reverse(e)| e.deadline)
    }

    /// Pop the next event — assumes the caller has already waited past its
    /// deadline. Never does its own `sleep_until`, so it's cheap and cancel-safe.
    pub fn pop_if_due(&mut self, now: VirtualInstant) -> Option<EngineAction> {
        match self.queue.peek() {
            Some(Reverse(e)) if e.deadline <= now => {
                let Reverse(event) = self.queue.pop().unwrap();
                Some(event.action)
            }
            _ => None,
        }
    }
}
```

### Cancel-safety in the engine `select!`

The engine's main loop interleaves three sources: session commands, control-plane commands, and scheduled events. The scheduler primitives above are deliberately *pull*-only (peek + pop_if_due) so `select!` never parks on an un-cancel-safe future holding an event that could be lost:

```rust
async fn run(&mut self) {
    loop {
        let next_deadline = self.scheduler.peek_deadline();

        tokio::select! {
            // Cancel-safe: mpsc::Receiver::recv() is cancel-safe.
            Some(cmd) = self.command_rx.recv() => {
                self.handle_command(cmd);
            }

            // Cancel-safe: sleep_until is cancel-safe; on wake, we re-peek the scheduler
            // under the lock and pop only if still due. If a new earlier-deadline event
            // was scheduled meanwhile, the next iteration's sleep_until picks it up.
            _ = async {
                if let Some(d) = next_deadline { self.clock.sleep_until(d).await; }
                else { std::future::pending::<()>().await; }
            } => {
                while let Some(action) = self.scheduler.pop_if_due(self.clock.now()) {
                    self.handle_scheduled(action);
                }
            }
        }
    }
}
```

**Why this is cancel-safe**: the `select!` arms only await cancel-safe primitives (`mpsc::recv`, `sleep_until`). The scheduler operations (`peek_deadline`, `pop_if_due`) are synchronous and non-awaiting, so they never hold scheduler state across an await point. A command arrival that interrupts the sleep-until arm simply causes the next loop iteration to re-peek and re-sleep — the scheduler's events are safe in the queue the entire time.

**Wake-latency bound**: if a command handler schedules a new event with a deadline *earlier* than the one currently being awaited, the existing `sleep_until` does not automatically shorten — the command arm completes, the loop iterates, re-peeks the scheduler (now seeing the earlier deadline), and re-sleeps. This adds at most one loop iteration of wake latency relative to the new deadline. Under `VirtualClock` this is exactly zero wall-clock time (the advance mechanism fires waiters by deadline order). Under `RealClock` the added latency is bounded by the time to execute one command handler — typically sub-microsecond. Not a correctness issue; worth knowing when profiling tight real-time loops.

**Regression test**: integration test that schedules 1000 events, interleaves 1000 commands arriving at random virtual times, and asserts every scheduled event is delivered exactly once in the expected order.

### Determinism invariant

Given the same sequence of `schedule()` calls with the same `(deadline, action)`, the scheduler delivers them in the same order. The `seq` tie-breaker guarantees this even when two events share a deadline.

Tested explicitly: run the scheduler twice with the same input sequence under `VirtualClock`, assert identical output order.

## Wall-clock mapping

Virtual time ticks from 0. For scenarios anchored to calendar time (e.g., "open at 09:30 ET"), provide a session anchor:

```rust
pub struct SessionAnchor {
    pub start_wall_time: chrono::DateTime<chrono::Utc>, // e.g., 2026-04-18T13:30:00Z (09:30 ET)
}

impl SessionAnchor {
    pub fn to_wall(&self, vi: VirtualInstant) -> chrono::DateTime<chrono::Utc> {
        self.start_wall_time + chrono::Duration::from_std(vi.0).unwrap()
    }
    pub fn from_wall(&self, dt: chrono::DateTime<chrono::Utc>) -> Option<VirtualInstant> {
        let delta = (dt - self.start_wall_time).to_std().ok()?;
        Some(VirtualInstant(delta))
    }
}
```

The U-shape table (Stage 03) uses `SessionAnchor` to locate virtual time in the trading day. Historical data requests use it to map ISO timestamps to internal virtual instants.

## Why not just use `tokio::time::pause`?

`tokio::time::pause` gives a global paused clock for Tokio primitives, but:

- It's thread-local-ish and doesn't compose well across multiple test modules running in parallel.
- It only intercepts `tokio::time::sleep`, not arbitrary `Clock::sleep_until`.
- The sim needs to support non-Tokio clock control (scenario DSL `advance` commands, the control-plane HTTP API, external debugging tools).
- We want the engine code to be agnostic: the same code runs against `RealClock` in production and `VirtualClock` in tests without `#[cfg]` branches.

So we layer: our `VirtualClock` uses `tokio::time::pause` under the hood in tests for Tokio interop, but the rest of the sim only knows the `Clock` trait.

## Parallelism within this stage

Small, so one sub-team owns it end-to-end (~400 LOC). Merges ahead of anyone who wants to schedule events.

## Rollback signals

- Tests using `VirtualClock` still wait on real wall time → someone's calling `std::thread::sleep` or `tokio::time::sleep` directly; audit imports.
- `VirtualClock::advance` causes missed events (some waiters don't fire) → race in the waker send; use `tokio::sync::Notify` instead of one-shots.
- Scheduled events arrive in non-deterministic order → missing `seq` tie-breaker, or `HashMap` iteration leaked in.

## Kill criteria

- **Virtual clock can't run a 1-hour scenario in under 10s of real time** → the advance loop is doing unnecessary work; profile.
- **Real clock and virtual clock require different engine code** → abstraction broken; the `Clock` trait boundary was wrong.

## Deliverables

- All three clock implementations, with tests
- `EventScheduler` with determinism test (same input = same output × 3 runs)
- Bench: schedule 10,000 events, advance to end, < 100ms wall time under `VirtualClock`
- Documentation: `Clock` trait doc-comment includes guidance on when to use each mode
