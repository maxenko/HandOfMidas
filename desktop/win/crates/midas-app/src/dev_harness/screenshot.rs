//! PNG encoding + perceptual-diff for screenshots captured via
//! `iced::window::screenshot`.
//!
//! The plan's token-budget discipline drives the policy:
//!
//! 1. Every capture writes a PNG to `out_path`.
//! 2. If a reference PNG exists at the conventional path
//!    (`.devloop/refs/<stem>.png`), compute SSIM and write a diff image
//!    to `.devloop/diffs/<stem>.png`.
//! 3. The response body carries `delta` and `diff_path` fields so the
//!    Claude driver can decide whether to load the PNG into context.
//!
//! Never loads images into context itself — just writes files and
//! returns numeric metadata.

use std::path::{Path, PathBuf};

use iced::window::Screenshot;

pub const REFS_DIR: &str = ".devloop/refs";
pub const DIFFS_DIR: &str = ".devloop/diffs";

/// Outcome of a screenshot capture + optional diff.
pub struct CaptureResult {
    pub out_path: PathBuf,
    /// Mirror copy in the user's `Pictures\Screenshots` folder. Always
    /// written when the folder is resolvable; `None` if `dirs::picture_dir()`
    /// returns nothing (non-Windows host or unusual env).
    pub mirror_path: Option<PathBuf>,
    pub reference_path: Option<PathBuf>,
    pub diff_path: Option<PathBuf>,
    /// SSIM score in `[0, 1]` (1 = identical), or `None` if no
    /// reference was available to compare against.
    pub ssim: Option<f64>,
    /// Fraction of pixels that differ noticeably (YIQ-ish proxy) — a
    /// secondary signal that can be more intuitive than SSIM for large
    /// edits. `None` if no reference.
    pub diff_fraction: Option<f64>,
    pub width: u32,
    pub height: u32,
    pub scale_factor: f32,
}

/// Encode `screenshot` to PNG at `out_path`, optionally diff against
/// the conventional reference path, and return a summary.
pub fn capture(screenshot: &Screenshot, out_path: &Path) -> std::io::Result<CaptureResult> {
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let width = screenshot.size.width;
    let height = screenshot.size.height;

    let rgba = screenshot.rgba.to_vec();
    let buf: image::RgbaImage =
        image::RgbaImage::from_raw(width, height, rgba).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "screenshot rgba buffer size mismatch",
            )
        })?;

    buf.save_with_format(out_path, image::ImageFormat::Png)
        .map_err(|e| std::io::Error::other(format!("png encode: {e}")))?;

    let mirror_path = mirror_to_pictures(out_path);

    let reference_path = reference_for(out_path);
    let (ssim, diff_fraction, diff_path) = match reference_path.as_ref() {
        Some(ref_path) if ref_path.exists() => diff_against(&buf, ref_path, out_path),
        _ => (None, None, None),
    };

    Ok(CaptureResult {
        out_path: out_path.to_path_buf(),
        mirror_path,
        reference_path,
        diff_path,
        ssim,
        diff_fraction,
        width,
        height,
        scale_factor: screenshot.scale_factor,
    })
}

/// Copy the freshly-written PNG into the user's
/// `Pictures\Screenshots\` folder with a timestamp suffix so parallel
/// runs don't overwrite each other. Returns the written path, or `None`
/// if the folder can't be resolved or the copy fails (non-fatal —
/// the primary `out_path` was already written).
fn mirror_to_pictures(out_path: &Path) -> Option<PathBuf> {
    let pictures = dirs::picture_dir()?;
    let dest_dir = pictures.join("Screenshots");
    if let Err(e) = std::fs::create_dir_all(&dest_dir) {
        tracing::warn!("devloop: could not create {}: {e}", dest_dir.display());
        return None;
    }

    let stem = out_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("shot");
    let stamp = chrono::Local::now().format("%Y%m%dT%H%M%S");
    let dest = dest_dir.join(format!("{stem}-{stamp}.png"));

    match std::fs::copy(out_path, &dest) {
        Ok(_) => {
            tracing::debug!("devloop: mirrored screenshot to {}", dest.display());
            Some(dest)
        }
        Err(e) => {
            tracing::warn!(
                "devloop: could not mirror screenshot to {}: {e}",
                dest.display()
            );
            None
        }
    }
}

/// Convention: a screenshot at `.devloop/shots/foo.png` uses reference
/// `.devloop/refs/foo.png`. Mirrors whatever stem the caller picked.
fn reference_for(out_path: &Path) -> Option<PathBuf> {
    let stem = out_path.file_stem()?.to_str()?;
    Some(PathBuf::from(REFS_DIR).join(format!("{stem}.png")))
}

fn diff_against(
    actual: &image::RgbaImage,
    reference_path: &Path,
    out_path: &Path,
) -> (Option<f64>, Option<f64>, Option<PathBuf>) {
    let reference = match image::open(reference_path) {
        Ok(img) => img.to_rgba8(),
        Err(e) => {
            tracing::warn!(
                "devloop: could not read reference {}: {e}",
                reference_path.display()
            );
            return (None, None, None);
        }
    };

    if reference.dimensions() != actual.dimensions() {
        tracing::warn!(
            "devloop: reference size {:?} != actual {:?}, skipping diff",
            reference.dimensions(),
            actual.dimensions(),
        );
        return (None, None, None);
    }

    // SSIM via image-compare: hybrid structural + colour similarity.
    // Compare on RGB luminance by collapsing to grayscale — image-compare
    // provides `rgba_hybrid_compare` which weights alpha carefully.
    let similarity = match image_compare::rgba_hybrid_compare(&reference, actual) {
        Ok(sim) => sim,
        Err(e) => {
            tracing::warn!("devloop: ssim compare failed: {e}");
            return (None, None, None);
        }
    };

    let ssim = similarity.score;
    let diff_fraction = pixel_diff_fraction(&reference, actual);

    let stem = out_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("shot");
    let diff_path = PathBuf::from(DIFFS_DIR).join(format!("{stem}.png"));
    if let Some(parent) = diff_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // image-compare returns an `image: GrayImage`-like struct for the
    // similarity map; save it so humans can see where regressions live.
    if let Err(e) = similarity.image.to_color_map().save(&diff_path) {
        tracing::warn!(
            "devloop: could not write diff image {}: {e}",
            diff_path.display()
        );
        return (Some(ssim), Some(diff_fraction), None);
    }

    (Some(ssim), Some(diff_fraction), Some(diff_path))
}

/// Fraction of pixels whose max channel delta exceeds 8/255. Cheap
/// secondary signal — intuitive for humans even when SSIM is close to 1.
fn pixel_diff_fraction(a: &image::RgbaImage, b: &image::RgbaImage) -> f64 {
    const THRESHOLD: i32 = 8;
    let mut differing = 0u64;
    let total = (a.width() as u64) * (a.height() as u64);
    for (px_a, px_b) in a.pixels().zip(b.pixels()) {
        let mut max_delta = 0;
        for c in 0..3 {
            let d = (px_a.0[c] as i32 - px_b.0[c] as i32).abs();
            if d > max_delta {
                max_delta = d;
            }
        }
        if max_delta > THRESHOLD {
            differing += 1;
        }
    }
    if total == 0 {
        0.0
    } else {
        differing as f64 / total as f64
    }
}
