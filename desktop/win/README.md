# Hand of Midas

Native desktop stock charting application. Rust + wgpu + iced.

## Layout

- [`CLAUDE.md`](CLAUDE.md) — workspace conventions, build commands, architecture rules.
- [`plan/`](plan/) — active design plans. Implemented work moves to [`plan/archive/`](plan/archive/).
- [`research/`](research/) — investigation notes, baseline screenshots, plus [`research/knowledge.md`](research/knowledge.md) (hard-won debugging lessons — read before re-fighting iced layout / wgpu / decorator battles).
- [`tools/`](tools/) — dev-loop smoke scripts and harness tooling.
- [`tests/`](tests/) — integration tests, fixtures, and visual-regression artifacts.
- [`benches/`](benches/) — workspace benchmarks.
- [`crates/`](crates/) — the 11 workspace crates (see CLAUDE.md for the dependency tree).

## Build

```bash
cargo run -p midas-app                              # normal launch
cargo run -p midas-app --features dev_harness       # with TCP harness on 127.0.0.1:9898
cargo test --workspace
cargo clippy --workspace -- -D warnings
```
