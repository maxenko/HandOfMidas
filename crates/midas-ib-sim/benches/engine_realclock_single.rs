//! Wave 2 throughput target: single session, synthetic command stream over
//! `RealClock`. Target: 2 000 events/sec sustained, p99 < 10 ms.
//!
//! Stage 01 scaffold: declares the benchmark target + a `todo!()`-bodied
//! harness so `cargo bench -p midas-ib-sim` compiles and Wave 2 can just
//! fill in the Criterion `bench_function` body.

use criterion::{criterion_group, criterion_main, Criterion};

fn bench_single_session(c: &mut Criterion) {
    c.bench_function("engine_realclock_single", |b| {
        b.iter(|| {
            // Wave 2 — drive a single-session synthetic command stream through
            // the engine and measure per-event cost.
            todo!("Wave 2 — engine_realclock_single body");
        });
    });
}

criterion_group!(benches, bench_single_session);
criterion_main!(benches);
