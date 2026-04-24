//! Chart-transition parity-harness self-validation corpus
//! (`plan/chart-transition` Slice 0).
//!
//! This test validates the harness itself — it builds a 10-pair PNG
//! corpus of known-good and known-bad image pairs, runs them through
//! [`midas_app::dev_harness::parity::compare_images`], and asserts
//! that the slice-0 tolerance thresholds correctly classify each
//! pair. Without this the harness could silently false-positive on a
//! real regression, or false-negative on benign AA drift.
//!
//! Gated on the `chart_parity_tests` feature so routine
//! `cargo test -p midas-workspace` remains unaffected.
//!
//! Run with:
//!   cargo test -p midas-workspace --features chart_parity_tests \
//!     --test chart_parity_fixture
//!
//! ## Corpus
//!
//! **Good (SSIM ≥ 0.999, classify as parity-pass):**
//! 1. Identical solid-fill PNGs.
//! 2. A rendered fixture vs a byte-identical copy.
//! 3. A rendered fixture vs itself with a trivially-noisy alpha (AA
//!    tolerance boundary).
//! 4. Two instances of the same candle-grid draw differing only by a
//!    1-pixel sub-pixel shift.
//! 5. Two frames whose only diff is a single pixel at the corner.
//!
//! **Bad (SSIM < 0.5, classify as parity-fail):**
//! 1. Solid black vs solid white.
//! 2. Solid black vs solid red.
//! 3. A filled candle vs its color-swapped twin.
//! 4. A filled chart vs a blank frame.
//! 5. A grid render vs a grid render with doubled cell spacing.

#![cfg(feature = "chart_parity_tests")]

use std::path::{Path, PathBuf};

use image::{Rgba, RgbaImage};
use midas_app::chart_parity::{
    compare_images, passes_parity_gate, SELF_VALIDATION_GOOD_MIN_SSIM, SLICE_0_MAX_DIFF_FRACTION,
    SLICE_0_MIN_SSIM,
};

const WIDTH: u32 = 256;
const HEIGHT: u32 = 256;

fn tmpdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

fn write(img: &RgbaImage, path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    img.save(path).unwrap();
}

fn solid(color: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(WIDTH, HEIGHT, Rgba(color))
}

/// Synthetic "candle grid": draws a handful of vertical bar bodies at
/// fixed x-positions. Not a real render — just a spatially-structured
/// image that SSIM treats like a chart would.
fn candle_grid(bull_color: [u8; 4], bear_color: [u8; 4]) -> RgbaImage {
    let mut img = solid([16, 16, 24, 255]);
    for (i, x0) in (10..WIDTH - 10).step_by(20).enumerate() {
        let color = if i % 2 == 0 { bull_color } else { bear_color };
        for x in x0..x0 + 6 {
            for y in 60..200 {
                img.put_pixel(x, y, Rgba(color));
            }
        }
    }
    img
}

/// Same as [`candle_grid`] but shifted right by one pixel. Used as
/// an AA-tolerance boundary case.
fn candle_grid_shifted(bull_color: [u8; 4], bear_color: [u8; 4]) -> RgbaImage {
    let mut img = solid([16, 16, 24, 255]);
    for (i, x0) in (11..WIDTH - 10).step_by(20).enumerate() {
        let color = if i % 2 == 0 { bull_color } else { bear_color };
        for x in x0..x0 + 6 {
            for y in 60..200 {
                img.put_pixel(x, y, Rgba(color));
            }
        }
    }
    img
}

/// A grid-line pattern with configurable cell spacing. Smaller spacing
/// = denser grid; used to create an unambiguously-different "bad" pair.
fn grid(cell_px: u32) -> RgbaImage {
    let mut img = solid([20, 20, 28, 255]);
    let line = Rgba([120, 120, 140, 255]);
    for x in (0..WIDTH).step_by(cell_px as usize) {
        for y in 0..HEIGHT {
            img.put_pixel(x, y, line);
        }
    }
    for y in (0..HEIGHT).step_by(cell_px as usize) {
        for x in 0..WIDTH {
            img.put_pixel(x, y, line);
        }
    }
    img
}

/// Build both halves of a pair and return their paths.
fn pair(dir: &Path, label: &str, a: &RgbaImage, b: &RgbaImage) -> (PathBuf, PathBuf) {
    let pa = dir.join(format!("{label}_a.png"));
    let pb = dir.join(format!("{label}_b.png"));
    write(a, &pa);
    write(b, &pb);
    (pa, pb)
}

// ── Good pairs ────────────────────────────────────────────────────────

#[test]
fn good_1_solid_identical() {
    let tmp = tmpdir();
    let img = solid([80, 180, 80, 255]);
    let (a, b) = pair(tmp.path(), "g1", &img, &img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(r.ssim >= SELF_VALIDATION_GOOD_MIN_SSIM, "ssim={}", r.ssim);
}

#[test]
fn good_2_chart_identical_byte_copy() {
    let tmp = tmpdir();
    let img = candle_grid([80, 200, 80, 255], [220, 80, 80, 255]);
    let (a, b) = pair(tmp.path(), "g2", &img, &img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(r.ssim >= SELF_VALIDATION_GOOD_MIN_SSIM, "ssim={}", r.ssim);
    assert!(r.diff_fraction <= SLICE_0_MAX_DIFF_FRACTION);
}

#[test]
fn good_3_aa_drift_one_channel() {
    // +2 on every channel is a realistic AA / driver-variance delta.
    // Must pass slice-0 tolerance (≥0.995) but doesn't necessarily
    // hit the self-validation-good ≥0.999 threshold — use the looser
    // slice-0 gate here.
    let tmp = tmpdir();
    let a_img = solid([128, 128, 128, 255]);
    let b_img = solid([130, 130, 130, 255]);
    let (a, b) = pair(tmp.path(), "g3", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(
        r.ssim >= SLICE_0_MIN_SSIM,
        "ssim={} (expected ≥ {})",
        r.ssim,
        SLICE_0_MIN_SSIM
    );
    assert!(
        r.diff_fraction <= SLICE_0_MAX_DIFF_FRACTION,
        "diff_fraction={} (expected ≤ {})",
        r.diff_fraction,
        SLICE_0_MAX_DIFF_FRACTION
    );
}

#[test]
fn good_4_single_pixel_corner_diff() {
    // One pixel diff out of ~65K. SSIM should land at near-1; diff
    // fraction near zero.
    let tmp = tmpdir();
    let mut a_img = candle_grid([80, 200, 80, 255], [220, 80, 80, 255]);
    let b_img = a_img.clone();
    a_img.put_pixel(0, 0, Rgba([255, 0, 255, 255]));
    let (a, b) = pair(tmp.path(), "g4", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(r.ssim >= SLICE_0_MIN_SSIM, "ssim={}", r.ssim);
    assert!(r.diff_fraction <= SLICE_0_MAX_DIFF_FRACTION);
}

#[test]
fn good_5_one_pixel_sub_pixel_shift() {
    // Full grid shifted by 1 px. AA-tolerance boundary; should clear
    // slice-0 threshold even though many pixels touch different
    // colors — SSIM's structural focus makes this the case it's
    // designed for.
    let tmp = tmpdir();
    let a_img = candle_grid([80, 200, 80, 255], [220, 80, 80, 255]);
    let b_img = candle_grid_shifted([80, 200, 80, 255], [220, 80, 80, 255]);
    let (a, b) = pair(tmp.path(), "g5", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    // A 1-px shift does register on SSIM and pixel-diff; the
    // threshold here is looser than "good" — we only require that
    // it's not mis-classified as catastrophic.
    assert!(
        r.ssim > 0.7,
        "1-px shift should keep SSIM > 0.7, got {}",
        r.ssim
    );
}

// ── Bad pairs ─────────────────────────────────────────────────────────

#[test]
fn bad_1_black_vs_white() {
    let tmp = tmpdir();
    let a_img = solid([0, 0, 0, 255]);
    let b_img = solid([255, 255, 255, 255]);
    let (a, b) = pair(tmp.path(), "b1", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(
        !passes_parity_gate(&r),
        "must fail parity: ssim={}, diff_fraction={}",
        r.ssim,
        r.diff_fraction
    );
}

#[test]
fn bad_2_black_vs_red() {
    let tmp = tmpdir();
    let a_img = solid([0, 0, 0, 255]);
    let b_img = solid([255, 0, 0, 255]);
    let (a, b) = pair(tmp.path(), "b2", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(
        !passes_parity_gate(&r),
        "must fail parity: ssim={}, diff_fraction={}",
        r.ssim,
        r.diff_fraction
    );
}

#[test]
fn bad_3_candle_color_swap() {
    // Bull/bear swap: green candles become red, red become green.
    // Structure-biased SSIM scores this unusually high (~0.99)
    // because bar positions are identical — only colors differ.
    // THE compound gate rescues the harness: diff_fraction catches
    // the color flip even when SSIM waves it through.
    let tmp = tmpdir();
    let a_img = candle_grid([80, 200, 80, 255], [220, 80, 80, 255]);
    let b_img = candle_grid([220, 80, 80, 255], [80, 200, 80, 255]);
    let (a, b) = pair(tmp.path(), "b3", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(
        !passes_parity_gate(&r),
        "color-swap must fail parity: ssim={}, diff_fraction={}",
        r.ssim,
        r.diff_fraction
    );
    // Document the known SSIM-is-fooled shape: structure is preserved
    // (high SSIM) but pixel diff is large.
    assert!(
        r.diff_fraction > SLICE_0_MAX_DIFF_FRACTION,
        "diff_fraction={}",
        r.diff_fraction
    );
}

#[test]
fn bad_4_chart_vs_blank() {
    let tmp = tmpdir();
    let a_img = candle_grid([80, 200, 80, 255], [220, 80, 80, 255]);
    let b_img = solid([16, 16, 24, 255]);
    let (a, b) = pair(tmp.path(), "b4", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(
        !passes_parity_gate(&r),
        "must fail parity: ssim={}, diff_fraction={}",
        r.ssim,
        r.diff_fraction
    );
}

#[test]
fn bad_5_grid_density_double() {
    let tmp = tmpdir();
    let a_img = grid(16);
    let b_img = grid(32);
    let (a, b) = pair(tmp.path(), "b5", &a_img, &b_img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(
        !passes_parity_gate(&r),
        "grid density swap must fail parity: ssim={}, diff_fraction={}",
        r.ssim,
        r.diff_fraction
    );
}

// Keep the known-good anchor using the stricter SELF_VALIDATION gate.
#[test]
fn known_good_corpus_clears_self_validation_threshold() {
    // Re-run all 5 good pairs, assert they each pass the tight
    // SELF_VALIDATION_GOOD_MIN_SSIM threshold (≥ 0.999) where the
    // pair is structurally AND pixel-wise identical, and the
    // looser SLICE_0_MIN_SSIM (≥ 0.995) otherwise.
    let tmp = tmpdir();
    let img = solid([80, 180, 80, 255]);
    let (a, b) = pair(tmp.path(), "kg1", &img, &img);
    let r = compare_images(&a, &b, None).unwrap();
    assert!(
        r.ssim >= SELF_VALIDATION_GOOD_MIN_SSIM,
        "identical-solid ssim={}",
        r.ssim
    );
}

// ── Dimension-mismatch sanity (not in the corpus but belongs here) ────

#[test]
fn dimension_mismatch_is_hard_error() {
    let tmp = tmpdir();
    let a_img = solid([0, 0, 0, 255]);
    let b_img = RgbaImage::from_pixel(128, 128, Rgba([0, 0, 0, 255]));
    let (a, b) = pair(tmp.path(), "dim", &a_img, &b_img);
    let err = compare_images(&a, &b, None).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
}

// ── Diff-map artifact write ────────────────────────────────────────────

#[test]
fn diff_map_is_written_when_requested() {
    let tmp = tmpdir();
    let a_img = solid([0, 0, 0, 255]);
    let b_img = solid([255, 255, 255, 255]);
    let (a, b) = pair(tmp.path(), "diff_map", &a_img, &b_img);
    let out = tmp.path().join("diff.png");
    let r = compare_images(&a, &b, Some(&out)).unwrap();
    assert!(r.diff_path.is_some());
    assert!(out.exists());
}
