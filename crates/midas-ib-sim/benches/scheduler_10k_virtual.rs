//! Stage 08 target: schedule 10 000 events at random deadlines, advance to
//! the end of the horizon under `VirtualClock`, drain every event — all in
//! less than 100 ms of wall time.
//!
//! The integration test `ten_thousand_events_advance_in_under_100ms_wall`
//! asserts the same budget; this bench reports the actual distribution for
//! regression tracking.

use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, BatchSize, Criterion};
use midas_broker_core::SymbolKey;
use midas_ib_sim::engine::clock::{Clock, VirtualClock, VirtualInstant};
use midas_ib_sim::engine::scheduler::{EngineAction, EventScheduler};
use midas_ib_sim::engine::types::{ReqId, SessionId, SubKey};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const EVENT_COUNT: usize = 10_000;
const SEED: u64 = 0x5EED_0A75;

fn tick(idx: usize) -> EngineAction {
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

fn build_inputs() -> Vec<(u64, usize)> {
    let mut rng = StdRng::seed_from_u64(SEED);
    (0..EVENT_COUNT)
        .map(|idx| (rng.gen_range(0..1_000), idx))
        .collect()
}

fn bench_10k_virtual_drain(c: &mut Criterion) {
    c.bench_function("scheduler_10k_virtual_drain", |b| {
        b.iter_batched(
            || {
                let clock = Arc::new(VirtualClock::new());
                let mut sched = EventScheduler::new();
                for &(ms, idx) in &build_inputs() {
                    sched.schedule(VirtualInstant::from_millis(ms), tick(idx));
                }
                (clock, sched)
            },
            |(clock, mut sched)| {
                clock.advance(VirtualInstant::from_millis(1_000));
                let mut count = 0usize;
                while sched.pop_if_due(clock.now()).is_some() {
                    count += 1;
                }
                assert_eq!(count, EVENT_COUNT);
            },
            BatchSize::SmallInput,
        );
    });
}

fn config() -> Criterion {
    Criterion::default()
        .warm_up_time(Duration::from_millis(500))
        .measurement_time(Duration::from_secs(3))
        .sample_size(20)
}

criterion_group! {
    name = benches;
    config = config();
    targets = bench_10k_virtual_drain
}
criterion_main!(benches);
