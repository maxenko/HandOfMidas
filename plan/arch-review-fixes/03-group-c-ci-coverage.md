# Group C — CI Coverage

The CI workflow today exercises 2 of 7 feature flags and runs only on Linux. Five feature flags — including `session_chart_tests` and `chart_parity_tests`, written specifically as harness self-validation — never run on push, so the harness can silently false-positive. Target platform per `desktop/win/CLAUDE.md` is Windows 11; CI never runs on it.

Three slices, all independent.

---

## Slice C1 — Feature-flag CI jobs (Linux)

**Goal:** Add CI coverage for `session_chart`, `session_chart_tests`, and `dev_harness` on the Linux runner. The fourth review-flagged feature, `chart_parity_tests`, is platform-sensitive (font rendering) and runs on the Windows runner from C2 instead — see slice A3.

**Depends on:** None.

### Files to modify

- `.github/workflows/rust.yml` — append four new jobs after `desktop`

### Existing template (verbatim, for reference)

```yaml
broker_ib_live_feature_lint:
  name: Broker IB-live feature lint (non-blocking)
  runs-on: ubuntu-latest
  if: github.event_name != 'schedule'
  needs: broker
  continue-on-error: true
  steps:
    - uses: actions/checkout@v5
    - name: Install Rust toolchain
      uses: dtolnay/rust-toolchain@stable
      with:
        components: clippy
    - name: Cache cargo registry + target
      uses: Swatinem/rust-cache@v2
      with:
        shared-key: broker
    - name: Clippy with ib_live_tests
      run: cargo clippy -p midas-broker --features ib_live_tests --all-targets -- -D warnings
```

### Three new jobs (skeleton — exact wording at implementation time)

```yaml
desktop_session_chart_lint:
  name: Desktop session_chart feature lint (non-blocking)
  runs-on: ubuntu-latest
  if: github.event_name != 'schedule'
  needs: desktop
  continue-on-error: true
  steps:
    - uses: actions/checkout@v5
    - uses: dtolnay/rust-toolchain@stable
      with: { components: clippy }
    - uses: Swatinem/rust-cache@v2
      with:
        shared-key: desktop
        workspaces: desktop/win
    - name: Clippy desktop with session_chart
      working-directory: desktop/win
      run: cargo clippy -p midas-app --features session_chart --all-targets -- -D warnings

desktop_dev_harness_lint:
  # ... same skeleton, --features dev_harness ...

desktop_session_chart_tests:
  name: Desktop session_chart_tests (non-blocking)
  runs-on: ubuntu-latest
  if: github.event_name != 'schedule'
  needs: desktop
  continue-on-error: true
  steps:
    - uses: actions/checkout@v5
    - uses: dtolnay/rust-toolchain@stable
      with: { components: clippy }
    - uses: Swatinem/rust-cache@v2
      with:
        shared-key: desktop
        workspaces: desktop/win
    - name: Test session_chart_tests
      working-directory: desktop/win
      run: cargo test --workspace --features session_chart_tests
```

(`chart_parity_tests` is intentionally absent — it runs on the Windows runner in C2 because PNG-diff is platform-sensitive.)

### Key implementation details

- **`continue-on-error: true` for all three.** Per design decision D5 in `00-index.md`: ship non-blocking. The PR description records a hard date 14 days after merge for the flip-to-required follow-up; that follow-up flips C1, C2, AND the existing `broker_ib_live_feature_lint` job in one go.
- **`needs: desktop` ordering.** Lets the new jobs reuse the desktop job's cache hit. Without `needs:`, they'd run in parallel against a cold cache and waste CI time.
- **`shared-key: desktop` + `workspaces: desktop/win`.** Both are required for desktop-workspace caching per existing pattern at the `desktop` job.
- **`if: github.event_name != 'schedule'`** matches existing pattern; cron jobs already cover the nightly drift checks.
- **`session_chart_tests` is a workspace-level feature** declared at `desktop/win/Cargo.toml` (around line 67). The test invocation needs `--workspace` to reach the test files (they live under `desktop/win/tests/`, not under a single crate).
- **No new `working-directory: desktop/win` for the lint jobs** if their `cargo clippy -p midas-app` doesn't need workspace context — but per CLAUDE.md the desktop workspace requires `cd desktop/win` before any cargo invocation. Keep it for consistency with the existing `desktop` job.

### Testing

- **Workflow lint:** open the PR with the workflow change; GitHub Actions validates YAML on push. Iterate until the three new jobs appear in the Actions tab.
- **First-run pass:** all three jobs must succeed on the PR's HEAD or be a flake. If they red — fix the underlying feature ON-path issue before merging (the whole point is they should be ON-path-clean today).

### Done when

- All three jobs visible in the Actions tab on push to a feature branch.
- All three jobs green on a PR that doesn't touch the gated code.
- `.github/workflows/rust.yml` carries an inline comment block above the new jobs explaining the non-blocking rollout policy and the scheduled flip-to-required date.

### Risks & mitigations

- **Risk:** A feature flag's ON-path code has a clippy warning we never knew about. **Mitigation:** Run `cargo clippy --features <flag> -- -D warnings` locally first; fix any issues. The whole goal is to start enforcing this — fixing the first round is unavoidable.
- **Risk:** `session_chart_tests` requires fixtures or env vars not available in CI. **Mitigation:** Look at existing test files (`session_chart_e2e.rs`, etc.) for any external resource needs. If found, either bundle them with the test or skip individual tests via `#[ignore]` initially.
- **Risk:** Cache contention between desktop, desktop_session_chart_lint, etc. **Mitigation:** They share `shared-key: desktop` intentionally; `Swatinem/rust-cache@v2` handles concurrent reads.

### Rollback signal

If any of the three jobs is flaky on identical commits (passes once, fails next push), don't ignore — investigate. A flaky non-blocking job teaches developers to ignore it, defeating the purpose. Either stabilise or remove until stable.

---

## Slice C2 — Windows runner

**Goal:** Run desktop fmt + clippy + relevant tests on `windows-latest`, including the `chart_parity_tests` (which is platform-sensitive and lives here, not on the Linux runner). Target platform per `desktop/win/CLAUDE.md:6` is Windows 11; today the CI never runs there.

**Depends on:** None for the basic Windows job. The parity-test step depends on A3 landing — until then, that step is `if: false`-gated.

### Files to modify

- `.github/workflows/rust.yml` — append `desktop_windows` job after the existing `desktop` job

### Skeleton

```yaml
desktop_windows:
  name: Desktop Windows (non-blocking)
  runs-on: windows-latest
  if: github.event_name != 'schedule'
  needs: desktop
  continue-on-error: true
  steps:
    - uses: actions/checkout@v5
    - uses: dtolnay/rust-toolchain@stable
      with:
        components: clippy, rustfmt
        targets: x86_64-pc-windows-msvc
    - uses: Swatinem/rust-cache@v2
      with:
        shared-key: desktop-windows
        workspaces: desktop/win
    - name: fmt
      working-directory: desktop/win
      run: cargo fmt --all --check
    - name: clippy
      working-directory: desktop/win
      run: cargo clippy --workspace --all-targets -- -D warnings
    - name: build
      working-directory: desktop/win
      run: cargo build --workspace
    - name: test (default features)
      working-directory: desktop/win
      run: cargo test --workspace
    - name: test app_sim_e2e (Windows-only)
      working-directory: desktop/win
      run: cargo test --test app_sim_e2e -- --ignored
    # Gate this step on A3 having landed. Flip `if: false` to
    # `if: github.event_name != 'schedule'` in A3's PR if A3 lands
    # after C2, or directly in C2's initial PR if A3 is already merged.
    - name: test chart_parity_tests (Windows-only)
      working-directory: desktop/win
      if: false
      run: cargo test --workspace --features chart_parity_tests --test parity_baseline
```

### Key implementation details

- **`shared-key: desktop-windows`** — separate from the Linux desktop cache (different target triple, different binary cache).
- **`targets: x86_64-pc-windows-msvc`** — matches `desktop/win/rust-toolchain.toml`.
- **`app_sim_e2e -- --ignored`** is critical — these tests `#[ignore]` themselves on every platform per the `cfg_attr` stack at `desktop/win/tests/app_sim_e2e.rs:206-214`. The Windows variant is "spawns subprocesses; run with --ignored". Without `--ignored`, they don't run at all.
- **`continue-on-error: true`.** Same non-blocking rollout as C1 jobs.
- **`needs: desktop`** lets the Linux desktop job act as the fast-feedback loop; Windows runs after to catch platform skew.
- **No `bash`-specific flags** — GitHub Actions on Windows defaults to PowerShell, but `cargo` invocations don't care about shell. The existing `rust_ibapi_drift` cron job uses bash here-docs; that's not in this PR's scope.
- **`wgpu` device on Windows runners:** GitHub's `windows-latest` images include DirectX support; `wgpu` 27 on `x86_64-pc-windows-msvc` works. If a test needs an actual surface (rare in this codebase — `chart_parity_tests` uses headless via the dev harness), mark with `#[ignore]` and pull in via `--ignored`.

### Testing

- **First push of the workflow:** observe Windows job execution in Actions tab. Compile-time will be slow (~10 min cold cache); this is normal.
- **Iterate locally if it fails:** if the Windows job fails for platform-specific reasons, reproduce locally on Windows (the dev environment is Windows 11 per the project context).

### Done when

- Windows runner job visible in Actions tab.
- Job green on a clean push.
- Job runs `cargo fmt --check`, `clippy`, `build`, `test`, and `test --test app_sim_e2e -- --ignored`.
- `continue-on-error: true` is in place; flip-to-required scheduled separately.

### Risks & mitigations

- **Risk:** Windows compile is slow → CI times out at 6 hours. **Mitigation:** With `Swatinem/rust-cache@v2` warmed, expect ~10-15 min for desktop workspace; well within timeout. First-ever run will be slow.
- **Risk:** `app_sim_e2e` has un-ignored hangs on Windows. **Mitigation:** All four tests are explicitly `#[cfg_attr(target_os = "windows", ignore = "spawns subprocesses; run with --ignored")]`; default `cargo test` passes them by skipping. The `--ignored` invocation deliberately runs them; if they hang, that's a real bug to fix.
- **Risk:** Cache invalidation between Linux and Windows. **Mitigation:** Distinct `shared-key`; no overlap.

### Rollback signal

If the Windows job exposes a flaky failure mode (passes locally, fails in CI consistently), don't suppress with `if: false` — investigate the platform-skew root cause. Linux-only CI is already the bug we're fixing.

---

## Slice C3 — Pin root toolchain via `rust-toolchain.toml`

**Goal:** Reproducibility. The desktop workspace pins via `desktop/win/rust-toolchain.toml`; the root workspace floats stable. Every root CI failure today carries an "is it the floating stable that bumped?" debugging tax.

**Depends on:** None.

### Files to create

- `rust-toolchain.toml` (new, at repo root)

### Content

```toml
[toolchain]
channel = "stable"
components = ["clippy", "rustfmt"]
profile = "minimal"
```

### Key implementation details

- **No `targets` array at root** — root crates build for the host. Desktop's `targets = ["x86_64-pc-windows-msvc"]` is correct for the Windows binary; root crates don't have a single canonical target.
- **`profile = "minimal"`** keeps install footprint small (no unnecessary `rust-docs`/`rust-std` for non-host targets).
- **`channel = "stable"`** mirrors desktop and the existing CI `dtolnay/rust-toolchain@stable`. Pinning to a specific version (e.g., `1.95`) would force a manual bump cadence; stable matches the implicit current behaviour but makes it explicit.
- **CI behaviour change:** `dtolnay/rust-toolchain@stable` action respects `rust-toolchain.toml` if present. After this slice lands, all root jobs will use the toolchain declared in the file rather than the action's `@stable` argument. The action's argument is then redundant but harmless.
- **rustup-respecting tools:** local `cargo`, `rustc`, `clippy` invocations from any directory under repo root will pick up the toolchain. This is what desktop does today; the root simply matches.

### Testing

- **Local:** run `cargo --version` from repo root after creating the file; should succeed and show stable. `cargo test --workspace` should run unchanged (same channel as before).
- **CI:** push with the file present; observe job logs show "Using rust-toolchain.toml"; jobs run identically.

### Done when

- `rust-toolchain.toml` exists at repo root.
- Local `cargo --version` from repo root works.
- CI run on the PR is green.
- The file is mentioned in `CLAUDE.md` (root) under a "Toolchain" subsection so contributors know it exists.

### Risks & mitigations

- **Risk:** A contributor without `rustup` (raw `rustc` install) breaks. **Mitigation:** Project already requires `rustup` for the existing desktop pin; this is consistent.
- **Risk:** `stable` floats forward and breaks something specific to the root workspace. **Mitigation:** This is the situation we have today. The pin file makes the floating behaviour explicit but doesn't change it. Pinning to a specific version is a separate decision (not in this slice).

### Rollback signal

If after the file is added, a contributor reports cargo failing because their toolchain doesn't have rustfmt or clippy, the components list is right but the user's rustup needs `rustup component add` (one-time). This is rustup-canonical behaviour — not a rollback signal, just contributor onboarding.
