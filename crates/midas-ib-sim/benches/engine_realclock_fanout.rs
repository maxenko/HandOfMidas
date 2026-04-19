//! Wave 2 throughput target: 100 symbols, 5 000 ticks/sec aggregate, no dropped
//! ticks, engine loop p99 < 10 ms.
//!
//! Stage 01 scaffold: declares the bench; Wave 2 fills in the body.

use criterion::{criterion_group, criterion_main, Criterion};

fn bench_fanout(c: &mut Criterion) {
    c.bench_function("engine_realclock_fanout", |b| {
        b.iter(|| {
            // Wave 2 — 100-symbol synthetic tick burst, measure drop rate +
            // engine-loop latency.
            todo!("Wave 2 — engine_realclock_fanout body");
        });
    });
}

criterion_group!(benches, bench_fanout);
criterion_main!(benches);
