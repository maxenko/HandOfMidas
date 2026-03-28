//! Centralized grid system for chart rendering.
//!
//! This module is the **single source of truth** for all grid line
//! computation. It produces horizontal price grid lines with hierarchical
//! styling (dollar, half-dollar, quarter, dime levels). Vertical time
//! grid lines are handled by the `date_labels` module.
//!
//! # Price grid hierarchy
//!
//! Lines are categorized by the price level they represent:
//!
//! | Level | Example prices | Visual weight |
//! |-------|---------------|---------------|
//! | Major ($1+) | $150, $151 | Brightest, 1px |
//! | Half ($0.50) | $150.50 | Medium, 0.7px |
//! | Quarter ($0.25) | $150.25, $150.75 | Subtle, 0.5px |
//! | Dime ($0.10) | $150.10, $150.20 | Faintest, 0.5px |
//!
//! The visible levels adapt to zoom: when zoomed out far enough that
//! dime lines would overlap, only quarter/half/dollar lines are shown,
//! etc.

use crate::camera::Camera2D;

/// Maximum number of price grid lines.
const MAX_PRICE_LINES: usize = 80;

/// A positioned horizontal price grid line with hierarchical weight.
#[derive(Clone, Debug)]
pub struct PriceGridLine {
    /// Screen Y position in logical pixels.
    pub y: f32,
    /// The price value at this line.
    pub price: f64,
    /// Visual weight category (determines color/opacity/thickness).
    pub weight: GridWeight,
}

/// Visual weight for grid lines — determines rendering style.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum GridWeight {
    /// Finest level: $0.10 increments.
    Dime,
    /// $0.25 increments.
    Quarter,
    /// $0.50 increments.
    Half,
    /// $1.00 increments (or the "nice step" for high-priced stocks).
    Dollar,
    /// Major round numbers ($5, $10, $50, $100, etc.).
    Major,
}

/// Compute horizontal price grid lines for the visible price range.
///
/// Returns lines sorted by Y position, each tagged with a hierarchical
/// weight. The caller converts these to GPU instances with appropriate
/// colors and thicknesses.
pub fn compute_price_grid(camera: &Camera2D) -> Vec<PriceGridLine> {
    let price_range = camera.price_high - camera.price_low;
    if price_range <= 0.0 {
        return Vec::new();
    }

    // Choose the finest step that doesn't overcrowd the viewport.
    // Target: at least ~40px between the finest visible grid lines.
    let min_px_between = 40.0;
    let min_price_step = price_range / (camera.viewport_height as f64 / min_px_between);

    // Standard price steps from finest to coarsest.
    let steps: &[f64] = &[
        0.01, 0.02, 0.05, 0.10, 0.25, 0.50, 1.0, 2.0, 5.0, 10.0, 25.0, 50.0, 100.0, 250.0, 500.0,
        1000.0, 2500.0, 5000.0,
    ];

    // Find the finest step that maintains minimum spacing.
    let base_step = steps
        .iter()
        .copied()
        .find(|&s| s >= min_price_step)
        .unwrap_or(5000.0);

    let mut lines = Vec::new();
    let first = (camera.price_low / base_step).ceil() * base_step;
    let mut price = first;

    while price < camera.price_high && lines.len() < MAX_PRICE_LINES {
        let y = camera.snap_to_pixel(camera.price_to_y(price));
        let weight = classify_price(price, base_step);
        lines.push(PriceGridLine { y, price, weight });
        price += base_step;
    }

    lines
}

/// Classify a price level into a grid weight category.
fn classify_price(price: f64, base_step: f64) -> GridWeight {
    // Work with the absolute price to handle negative values.
    let p = price.abs();

    // Check from coarsest to finest.
    // Major: divisible by 5× the "dollar" level.
    let dollar = find_dollar_level(base_step);
    let major = dollar * 5.0;

    if is_close_to_multiple(p, major) {
        return GridWeight::Major;
    }
    if is_close_to_multiple(p, dollar) {
        return GridWeight::Dollar;
    }
    if base_step <= 0.50 && is_close_to_multiple(p, 0.50) {
        return GridWeight::Half;
    }
    if base_step <= 0.25 && is_close_to_multiple(p, 0.25) {
        return GridWeight::Quarter;
    }
    if base_step <= 0.10 && is_close_to_multiple(p, 0.10) {
        return GridWeight::Dime;
    }

    // For larger base steps, classify relative to step multiples.
    let ratio = dollar / base_step;
    if ratio > 1.0 && is_close_to_multiple(p, dollar / 2.0) {
        return GridWeight::Half;
    }

    GridWeight::Dime
}

/// Determine what "one dollar" means for this price scale.
/// For sub-dollar prices, it's literally $1.00.
/// For higher prices, it scales: $10 step → "dollar" = $10, etc.
fn find_dollar_level(base_step: f64) -> f64 {
    if base_step <= 1.0 {
        1.0
    } else if base_step <= 10.0 {
        10.0
    } else if base_step <= 100.0 {
        100.0
    } else if base_step <= 1000.0 {
        1000.0
    } else {
        // Very high-priced assets: use 10^N
        let exp = base_step.log10().ceil();
        10_f64.powf(exp)
    }
}

/// Check if a value is close to a multiple of `step`.
fn is_close_to_multiple(value: f64, step: f64) -> bool {
    if step <= 0.0 {
        return false;
    }
    let remainder = (value / step).round() * step - value;
    remainder.abs() < step * 0.001
}

/// Color and thickness for a grid weight level (linear RGBA).
///
/// Returns `(color_rgba, thickness)`. The caller can use these directly
/// for GPU `GridLineInstance` construction.
pub fn style_for_weight(weight: GridWeight) -> ([f32; 4], f32) {
    match weight {
        GridWeight::Major => ([0.50, 0.50, 0.55, 0.20], 1.0),
        GridWeight::Dollar => ([0.40, 0.40, 0.45, 0.14], 1.0),
        GridWeight::Half => ([0.35, 0.35, 0.40, 0.10], 0.7),
        GridWeight::Quarter => ([0.30, 0.30, 0.35, 0.07], 0.5),
        GridWeight::Dime => ([0.25, 0.25, 0.30, 0.05], 0.5),
    }
}
