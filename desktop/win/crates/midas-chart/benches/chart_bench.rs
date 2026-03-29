use criterion::{criterion_group, criterion_main, Criterion};

fn placeholder_bench(c: &mut Criterion) {
    c.bench_function("chart_placeholder", |b| {
        b.iter(|| {
            // TODO: Add real benchmarks
            std::hint::black_box(42)
        })
    });
}

criterion_group!(benches, placeholder_bench);
criterion_main!(benches);
