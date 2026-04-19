//! Stage 08 — determinism + cancel-safety integration tests.
//!
//! 1. **Determinism**: schedule 1 000 events at the same sequence of random
//!    `(deadline, seq)` triples, run the scheduler under `VirtualClock` three
//!    times, assert identical delivery order.
//! 2. **Cancel-safety**: schedule 1 000 events + interleave 1 000 commands
//!    arriving at random virtual times, assert every scheduled event is
//!    delivered exactly once in the expected order.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use midas_broker_core::SymbolKey;
use midas_ib_sim::engine::clock::{Clock, VirtualClock, VirtualInstant};
use midas_ib_sim::engine::scheduler::{EngineAction, EventScheduler};
use midas_ib_sim::engine::types::{ReqId, SessionId, SubKey};

use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

const SEED: u64 = 0x0D15_EA5E_CAFE_F00D;
const EVENT_COUNT: usize = 1_000;

/// Emit `EngineAction::EmitTick` tagged with `idx` via the contract id; we
/// extract the index after delivery to compare against the expected order.
fn tick_action(idx: usize) -> EngineAction {
    EngineAction::EmitTick {
        key: SubKey {
            session: SessionId(1),
            req_id: ReqId(0),
            symbol: SymbolKey {
                contract_id: idx as i32,
                symbol: String::from("TAG"),
            },
        },
    }
}

fn sym_idx(action: &EngineAction) -> usize {
    match action {
        EngineAction::EmitTick { key } => key.symbol.contract_id as usize,
        other => panic!("unexpected action variant: {other:?}"),
    }
}

/// Generate a deterministic `(deadline, idx)` input list of length `n` with
/// deliberate duplicate deadlines to exercise the `seq` tie-break.
fn gen_input(n: usize, seed: u64) -> Vec<(u64, usize)> {
    let mut rng = StdRng::seed_from_u64(seed);
    (0..n)
        .map(|idx| {
            // Draw deadline from a tight pool (0..100ms) to force many ties.
            let ms: u64 = rng.gen_range(0..100);
            (ms, idx)
        })
        .collect()
}

/// Run one pass of the scheduler: schedule `input`, then drain by repeatedly
/// advancing the `VirtualClock` to each next deadline. Returns the sequence of
/// `symbol` indices in the order they were delivered.
async fn run_once(input: &[(u64, usize)]) -> Vec<usize> {
    let clock = Arc::new(VirtualClock::new());
    let mut sched = EventScheduler::new();

    for &(ms, idx) in input {
        sched.schedule(VirtualInstant::from_millis(ms), tick_action(idx));
    }

    let mut delivered = Vec::with_capacity(input.len());

    // Advance to the latest deadline in one shot; `pop_if_due` then drains
    // everything. Because the test uses `VirtualClock` there is no wall-time
    // cost to this.
    let max_ms = input.iter().map(|(ms, _)| *ms).max().unwrap_or(0);
    clock.advance(VirtualInstant::from_millis(max_ms));

    while let Some(action) = sched.pop_if_due(clock.now()) {
        delivered.push(sym_idx(&action));
    }

    assert_eq!(
        delivered.len(),
        input.len(),
        "every scheduled event must be delivered"
    );
    delivered
}

// ---------------------------------------------------------------------------
// 1. Determinism
// ---------------------------------------------------------------------------

#[tokio::test]
async fn scheduler_is_deterministic_across_3_runs() {
    let input = gen_input(EVENT_COUNT, SEED);

    let run_a = run_once(&input).await;
    let run_b = run_once(&input).await;
    let run_c = run_once(&input).await;

    assert_eq!(run_a, run_b, "run A and run B must match");
    assert_eq!(run_b, run_c, "run B and run C must match");

    // Sanity: the delivery order must be stable-sorted by (deadline, seq).
    let mut expected: Vec<(u64, usize)> = input.clone();
    // `seq` is assigned in schedule order, so `input`'s index *is* the seq;
    // a stable sort by `ms` yields the expected order.
    expected.sort_by_key(|a| a.0);
    let expected_indices: Vec<usize> = expected.iter().map(|(_, idx)| *idx).collect();
    assert_eq!(
        run_a, expected_indices,
        "delivery matches stable-sort order"
    );
}

// ---------------------------------------------------------------------------
// 2. Cancel-safety — scheduled events interleaved with commands on the
//    same engine loop must all be delivered exactly once, in order.
// ---------------------------------------------------------------------------

/// Dummy command that exercises the command-arm of the engine-style
/// `select!` loop. We only count arrivals; no payload is required.
#[derive(Clone, Copy, Debug)]
enum TestCmd {
    Noop,
}

/// A small, Stage-08-only engine-style harness: mimics the cancel-safe
/// `select!` body from `Engine::run` but wired to the test's scheduler and
/// command channel directly (so we don't need to plumb the full `Engine`
/// type through Stage-01-only fields).
async fn run_engine_like_loop(
    clock: Arc<VirtualClock>,
    sched: Arc<Mutex<EventScheduler>>,
    mut command_rx: tokio::sync::mpsc::Receiver<TestCmd>,
    delivered: Arc<Mutex<Vec<usize>>>,
    command_count: Arc<Mutex<usize>>,
    done_rx: tokio::sync::oneshot::Receiver<()>,
) {
    let mut done_rx = done_rx;
    loop {
        let next_deadline = sched.lock().unwrap().peek_deadline();

        tokio::select! {
            biased;

            // Shutdown signal — drain remaining due events, then exit.
            _ = &mut done_rx => {
                let now = clock.now();
                let mut guard = sched.lock().unwrap();
                while let Some(action) = guard.pop_if_due(now) {
                    delivered.lock().unwrap().push(sym_idx(&action));
                }
                break;
            }

            // Cancel-safe: mpsc::Receiver::recv is cancel-safe.
            maybe_cmd = command_rx.recv() => {
                match maybe_cmd {
                    Some(TestCmd::Noop) => {
                        *command_count.lock().unwrap() += 1;
                    }
                    None => {
                        // Command channel closed — wait for explicit shutdown
                        // so remaining scheduled events still drain.
                    }
                }
            }

            // Cancel-safe: VirtualClock::sleep_until parks on a oneshot which
            // is safe to drop (no event is lost — it lives in `sched`).
            _ = async {
                match next_deadline {
                    Some(d) => clock.sleep_until(d).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let now = clock.now();
                let mut guard = sched.lock().unwrap();
                while let Some(action) = guard.pop_if_due(now) {
                    delivered.lock().unwrap().push(sym_idx(&action));
                }
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scheduled_events_delivered_exactly_once_under_command_interleaving() {
    let clock = Arc::new(VirtualClock::new());
    let sched = Arc::new(Mutex::new(EventScheduler::new()));
    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<TestCmd>(4096);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();

    let delivered = Arc::new(Mutex::new(Vec::<usize>::new()));
    let command_count = Arc::new(Mutex::new(0usize));

    // Pre-populate the scheduler with EVENT_COUNT events at deadlines in
    // [1ms, 1000ms]. Save the input so we can assert full coverage.
    let mut rng = StdRng::seed_from_u64(SEED ^ 0xA5A5);
    let mut scheduled_input: Vec<(u64, usize)> = Vec::with_capacity(EVENT_COUNT);
    {
        let mut s = sched.lock().unwrap();
        for idx in 0..EVENT_COUNT {
            let ms: u64 = rng.gen_range(1..=1_000);
            scheduled_input.push((ms, idx));
            s.schedule(VirtualInstant::from_millis(ms), tick_action(idx));
        }
    }

    // Spawn the engine-like loop.
    let engine = tokio::spawn(run_engine_like_loop(
        Arc::clone(&clock),
        Arc::clone(&sched),
        cmd_rx,
        Arc::clone(&delivered),
        Arc::clone(&command_count),
        done_rx,
    ));

    // Spawn the command-injection task: sends 1000 commands at random
    // intervals while the clock advances. The spacing uses `tokio::task::yield_now`
    // so the engine gets frequent chances to both drain events and handle
    // commands.
    let cmd_tx_clone = cmd_tx.clone();
    let commander = tokio::spawn(async move {
        let mut rng = StdRng::seed_from_u64(SEED ^ 0xC0DE);
        for _ in 0..EVENT_COUNT {
            cmd_tx_clone.send(TestCmd::Noop).await.expect("send cmd");
            // 50% of the time, yield so the engine can pick up the message.
            if rng.gen_bool(0.5) {
                tokio::task::yield_now().await;
            }
        }
    });

    // Advance the virtual clock in small steps so we actively interleave
    // event delivery with command processing. At each step, yield to the
    // engine so it can drain.
    let clock_driver = {
        let clock = Arc::clone(&clock);
        let delivered = Arc::clone(&delivered);
        tokio::spawn(async move {
            for ms in (0..=1_000).step_by(10) {
                clock.advance(VirtualInstant::from_millis(ms));
                // Yield a couple of times to let the engine catch the wake.
                for _ in 0..4 {
                    tokio::task::yield_now().await;
                }
            }
            // Give the engine one last chance to drain everything; spin
            // yielding until all events are delivered (or we give up after
            // a reasonable number of iterations).
            for _ in 0..10_000 {
                if delivered.lock().unwrap().len() == EVENT_COUNT {
                    break;
                }
                clock.advance(VirtualInstant::from_millis(1_001));
                tokio::task::yield_now().await;
            }
        })
    };

    commander.await.unwrap();
    clock_driver.await.unwrap();

    // Close the command channel and signal shutdown.
    drop(cmd_tx);
    let _ = done_tx.send(());
    engine.await.unwrap();

    // Assertions.
    let delivered_guard = delivered.lock().unwrap();
    assert_eq!(
        delivered_guard.len(),
        EVENT_COUNT,
        "every scheduled event must be delivered exactly once"
    );

    // Every index must appear exactly once.
    let unique: HashSet<usize> = delivered_guard.iter().copied().collect();
    assert_eq!(
        unique.len(),
        EVENT_COUNT,
        "delivered set must contain every index exactly once"
    );

    // Delivery order must match the stable-sort by (deadline, seq); seq ==
    // scheduling order == idx here.
    let mut expected = scheduled_input.clone();
    expected.sort_by_key(|a| a.0);
    let expected_order: Vec<usize> = expected.iter().map(|(_, idx)| *idx).collect();
    assert_eq!(
        *delivered_guard, expected_order,
        "delivery must match stable-sort by deadline"
    );

    // All commands must have been handled too.
    assert_eq!(
        *command_count.lock().unwrap(),
        EVENT_COUNT,
        "every command must have been received"
    );
}

// ---------------------------------------------------------------------------
// 3. Re-peek after earlier-deadline schedule (smaller-scope cancel-safety).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn engine_loop_picks_up_earlier_deadline_scheduled_during_command_handling() {
    let clock = Arc::new(VirtualClock::new());
    let sched = Arc::new(Mutex::new(EventScheduler::new()));
    sched
        .lock()
        .unwrap()
        .schedule(VirtualInstant::from_millis(500), tick_action(0));

    let (cmd_tx, cmd_rx) = tokio::sync::mpsc::channel::<TestCmd>(8);
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let delivered = Arc::new(Mutex::new(Vec::<usize>::new()));
    let command_count = Arc::new(Mutex::new(0usize));

    let engine = tokio::spawn(run_engine_like_loop(
        Arc::clone(&clock),
        Arc::clone(&sched),
        cmd_rx,
        Arc::clone(&delivered),
        Arc::clone(&command_count),
        done_rx,
    ));

    // Kick the engine into its first `sleep_until(500ms)` by yielding.
    tokio::task::yield_now().await;

    // Send a command that (when received) inserts an earlier-deadline event
    // at t=100ms. The scheduler is populated from the command side of the
    // select!; the event must still fire when the clock crosses 100ms.
    cmd_tx.send(TestCmd::Noop).await.unwrap();
    // Let the engine process the command.
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    sched
        .lock()
        .unwrap()
        .schedule(VirtualInstant::from_millis(100), tick_action(42));

    // Advance clock to just past 100ms. The engine's active sleep was for
    // 500ms; per the plan, it only re-peeks on the next iteration. Triggering
    // that iteration requires either the current sleep to complete OR a
    // command arrival. We use a command arrival + a later advance.
    cmd_tx.send(TestCmd::Noop).await.unwrap();
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
    clock.advance(VirtualInstant::from_millis(100));
    // Drain cycle.
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    // Now advance to 500ms to release the last event.
    clock.advance(VirtualInstant::from_millis(500));
    for _ in 0..16 {
        tokio::task::yield_now().await;
    }

    drop(cmd_tx);
    let _ = done_tx.send(());
    engine.await.unwrap();

    let delivered_guard = delivered.lock().unwrap();
    assert_eq!(
        *delivered_guard,
        vec![42, 0],
        "earlier-scheduled event must fire first"
    );
}

// ---------------------------------------------------------------------------
// 4. Performance target: 10 000 events, advance to end, < 100ms wall.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ten_thousand_events_advance_in_under_100ms_wall() {
    use std::time::Instant as WallInstant;

    let clock = Arc::new(VirtualClock::new());
    let mut sched = EventScheduler::new();

    let mut rng = StdRng::seed_from_u64(SEED ^ 0xBEEF);
    for idx in 0..10_000 {
        let ms: u64 = rng.gen_range(0..1_000);
        sched.schedule(VirtualInstant::from_millis(ms), tick_action(idx));
    }

    let start = WallInstant::now();
    clock.advance(VirtualInstant::from_millis(1_000));
    let mut count = 0;
    while sched.pop_if_due(clock.now()).is_some() {
        count += 1;
    }
    let elapsed = start.elapsed();

    assert_eq!(count, 10_000);
    assert!(
        elapsed < Duration::from_millis(100),
        "target: 10 000 events drained in < 100ms wall time; got {elapsed:?}"
    );
}
