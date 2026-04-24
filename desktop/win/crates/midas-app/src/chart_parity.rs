//! Chart-transition parity harness (plan/chart-transition Slice 0).
//!
//! Reuses the SSIM + pixel-diff machinery already exercised by
//! [`super::screenshot::capture`], but exposes it as a stand-alone
//! compare over two arbitrary paths — without the
//! `.devloop/refs/<stem>.png` reference convention. Built to support
//! scripts that render one fixture through each chart backend (legacy
//! vs new) and diff the resulting PNGs pairwise.
//!
//! **What this module ships today**:
//! 1. [`compare_images`] — pure compare + optional diff-map write.
//! 2. [`pixel_diff_fraction`] — re-export of the same perceptual
//!    threshold used by the screenshot diff path.
//!
//! **Deferred to slice 9a** (when the runtime backend toggle lands):
//! the `render_backend_to_png(backend, fixture) -> PathBuf` helper
//! that would drive both backends in one harness call. Until then,
//! parity is exercised via two sequential runs of the existing
//! `Screenshot` command — one with `ChartBackend::Legacy`, one with
//! `ChartBackend::New` — and a [`compare_images`] call over the
//! resulting pair.

use std::path::Path;

pub use midas_devloop_proto::CompareResult;

/// Compare two PNGs on disk. Mirrors the SSIM + perceptual-pixel-diff
/// pair the screenshot handler computes against a reference, but takes
/// two explicit paths. Optionally writes the similarity map to
/// `diff_out`.
///
/// Errors fall through `std::io::Error` so the caller can surface a
/// clean [`ErrorKind::Internal`] on the wire.
///
/// Both images must have identical dimensions; differing sizes return
/// an `InvalidData` error (parity demands like-for-like comparison).
pub fn compare_images(
    path_a: &Path,
    path_b: &Path,
    diff_out: Option<&Path>,
) -> std::io::Result<CompareResult> {
    let a = image::open(path_a)
        .map_err(|e| std::io::Error::other(format!("read {}: {e}", path_a.display())))?
        .to_rgba8();
    let b = image::open(path_b)
        .map_err(|e| std::io::Error::other(format!("read {}: {e}", path_b.display())))?
        .to_rgba8();

    if a.dimensions() != b.dimensions() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "dimension mismatch: {:?} vs {:?}",
                a.dimensions(),
                b.dimensions()
            ),
        ));
    }

    let similarity = image_compare::rgba_hybrid_compare(&a, &b)
        .map_err(|e| std::io::Error::other(format!("ssim: {e}")))?;

    let ssim = similarity.score;
    let diff_fraction = pixel_diff_fraction(&a, &b);
    let (width, height) = a.dimensions();

    let diff_path = if let Some(out) = diff_out {
        if let Some(parent) = out.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match similarity.image.to_color_map().save(out) {
            Ok(()) => Some(out.to_path_buf()),
            Err(e) => {
                tracing::warn!("parity: could not write diff map to {}: {e}", out.display());
                None
            }
        }
    } else {
        None
    };

    Ok(CompareResult {
        ssim,
        diff_fraction,
        width,
        height,
        diff_path,
    })
}

/// Fraction of pixels whose RGB channels differ by more than the
/// perceptual threshold. Lifted from `screenshot.rs::pixel_diff_fraction`
/// so parity uses the same heuristic; re-implemented here rather than
/// exported to keep the screenshot path private.
pub(crate) fn pixel_diff_fraction(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    // YIQ-proxy: weight R/G/B by luma approximation, threshold on a
    // small delta. Matches the approach in `screenshot::diff_against`.
    const THRESHOLD: i32 = 24;
    let total = (a.width() as u64) * (a.height() as u64);
    if total == 0 {
        return 0.0;
    }
    let mut diffs: u64 = 0;
    for (ap, bp) in a.pixels().zip(b.pixels()) {
        let dr = ap[0] as i32 - bp[0] as i32;
        let dg = ap[1] as i32 - bp[1] as i32;
        let db = ap[2] as i32 - bp[2] as i32;
        // Weighted luma ≈ 0.30 R + 0.59 G + 0.11 B, scaled *100.
        let dy = (dr * 30 + dg * 59 + db * 11).abs() / 100;
        if dy > THRESHOLD {
            diffs += 1;
        }
    }
    diffs as f64 / total as f64
}

/// Slice-0 tolerance thresholds called out in
/// `plan/chart-transition/00-index.md`. Kept as public constants so
/// tests and CI scripts reference them by name, not by magic literal.
/// `#[allow(dead_code)]` because the binary target itself doesn't
/// consume these — the parity-fixture integration test and future
/// slice implementations are the callers.
#[allow(dead_code)]
pub const SLICE_0_MIN_SSIM: f64 = 0.995;
#[allow(dead_code)]
pub const SLICE_0_MAX_DIFF_FRACTION: f64 = 0.002;

/// Self-validation threshold for the "known-good" pairs in the
/// corpus — a well-rendered frame compared against itself (or a
/// byte-identical copy) must score near-perfect.
#[allow(dead_code)]
pub const SELF_VALIDATION_GOOD_MIN_SSIM: f64 = 0.999;

/// Compound parity-pass predicate used by the slice-0 gate AND by
/// the self-validation corpus's "bad" classifier. Returns `true` iff
/// the pair clears BOTH thresholds. SSIM alone is fooled by
/// color-swap-with-same-structure (e.g., bull/bear candle flip); the
/// pixel-diff catches those cases.
#[allow(dead_code)]
pub fn passes_parity_gate(r: &CompareResult) -> bool {
    r.ssim >= SLICE_0_MIN_SSIM && r.diff_fraction <= SLICE_0_MAX_DIFF_FRACTION
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn solid(w: u32, h: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba(color))
    }

    fn write_png(img: &RgbaImage, path: &Path) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        img.save(path).unwrap();
    }

    #[test]
    fn identical_images_score_perfect() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.png");
        let b = tmp.path().join("b.png");
        let img = solid(32, 32, [80, 180, 80, 255]);
        write_png(&img, &a);
        write_png(&img, &b);

        let r = compare_images(&a, &b, None).unwrap();
        assert!(r.ssim >= SELF_VALIDATION_GOOD_MIN_SSIM, "ssim={}", r.ssim);
        assert!(r.diff_fraction <= 1e-6);
        assert_eq!(r.width, 32);
        assert_eq!(r.height, 32);
        assert!(r.diff_path.is_none());
    }

    #[test]
    fn solid_vs_opposite_is_bad() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.png");
        let b = tmp.path().join("b.png");
        write_png(&solid(32, 32, [0, 0, 0, 255]), &a);
        write_png(&solid(32, 32, [255, 255, 255, 255]), &b);

        let r = compare_images(&a, &b, None).unwrap();
        assert!(
            !passes_parity_gate(&r),
            "ssim={} diff_fraction={}",
            r.ssim,
            r.diff_fraction
        );
        assert!(r.diff_fraction > 0.9);
    }

    #[test]
    fn small_shift_lands_inside_slice_0_tolerance() {
        // A one-channel drift of 2 on every pixel is the kind of AA /
        // driver-variance noise the slice-0 threshold is designed to
        // tolerate. Must score ≥ SLICE_0_MIN_SSIM AND diff_fraction
        // must stay ≤ SLICE_0_MAX_DIFF_FRACTION.
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.png");
        let b = tmp.path().join("b.png");
        write_png(&solid(64, 64, [128, 128, 128, 255]), &a);
        write_png(&solid(64, 64, [130, 130, 130, 255]), &b);

        let r = compare_images(&a, &b, None).unwrap();
        assert!(
            r.ssim >= SLICE_0_MIN_SSIM,
            "ssim={} should be ≥ {}",
            r.ssim,
            SLICE_0_MIN_SSIM
        );
        assert!(
            r.diff_fraction <= SLICE_0_MAX_DIFF_FRACTION,
            "diff_fraction={} should be ≤ {}",
            r.diff_fraction,
            SLICE_0_MAX_DIFF_FRACTION
        );
    }

    #[test]
    fn dimension_mismatch_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.png");
        let b = tmp.path().join("b.png");
        write_png(&solid(32, 32, [0, 0, 0, 255]), &a);
        write_png(&solid(64, 64, [0, 0, 0, 255]), &b);

        let err = compare_images(&a, &b, None).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    }

    #[test]
    fn diff_out_writes_map() {
        let tmp = tempfile::tempdir().unwrap();
        let a = tmp.path().join("a.png");
        let b = tmp.path().join("b.png");
        let diff = tmp.path().join("diff.png");
        write_png(&solid(32, 32, [0, 0, 0, 255]), &a);
        write_png(&solid(32, 32, [255, 255, 255, 255]), &b);

        let r = compare_images(&a, &b, Some(&diff)).unwrap();
        assert_eq!(r.diff_path.as_deref(), Some(diff.as_path()));
        assert!(diff.exists());
    }
}
