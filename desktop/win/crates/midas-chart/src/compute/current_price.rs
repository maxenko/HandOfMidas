//! Current-price indicator: a thin dotted horizontal line at the live
//! close price plus a flat right-edge price badge (no triangular
//! pointer — that geometry belongs to user-placed levels / brackets).
//!
//! Visual contract:
//! * Line: ~1 px tall dots, ~2 px wide, spaced ~6 px apart along the
//!   chart-area X. Very dim alpha so the indicator never competes
//!   with grid lines, candles, or annotations.
//! * Badge: rect (no point), bull/bear-tinted, sits flush against the
//!   priceline border at the current price's Y. Color follows the
//!   last candle's direction (close ≥ open → bull, else bear).
//!
//! Sans-IO. Returns plain data; the caller appends to the
//! corresponding `ChartScene` slots.
//!
//! No clipping is needed at the producer side: the line dots clip
//! horizontally to the priceline border by construction (loop bound
//! is `priceline_x`), and a `y` outside the price area is just
//! invisible — the dotted-line dots and the badge land off-screen but
//! cause no rendering glitch.

use midas_core::CandleData;
use midas_gpu_types::{BadgeInstance, GridLineInstance};

use crate::camera::Camera2D;
use crate::compute::PRICELINE_WIDTH;
use crate::widget::compute::{LabelAnchor, WidgetLabel};

/// Dot length along X in logical pixels.
const DOT_WIDTH_PX: f32 = 2.0;
/// Distance from one dot's start to the next dot's start. Dot ON for
/// `DOT_WIDTH_PX`, OFF for `DOT_PERIOD_PX - DOT_WIDTH_PX`.
const DOT_PERIOD_PX: f32 = 6.0;
/// Dot thickness in logical pixels. Sub-pixel; the renderer expands to
/// at least one physical pixel.
const DOT_HEIGHT_PX: f32 = 1.0;
/// Right-edge inset before the priceline border so dots don't overlap
/// the badge.
const RIGHT_INSET_PX: f32 = 4.0;
/// Badge dimensions, mirroring the level price-badge layout in
/// `levels.rs::to_decorators` minus the triangle.
const BADGE_HEIGHT_PX: f32 = 18.0;
/// Font size for the price text inside the badge.
const BADGE_FONT_SIZE_PX: f32 = 11.0;
/// Inset from the right edge of the chart for the badge text.
const BADGE_TEXT_RIGHT_INSET_PX: f32 = 6.0;

/// Output of [`compute_current_price_indicator`]. The caller appends
/// each field to the matching `ChartScene` slot.
pub struct CurrentPriceIndicator {
    /// Dotted-line dots; append to `scene.grid_instances`.
    pub line_dots: Vec<GridLineInstance>,
    /// Right-edge badge; append to `scene.badges`.
    pub badge: BadgeInstance,
    /// Price text; append to `scene.labels` (the indicator sits at the
    /// annotation z-layer so it's drawn above grid + axis text but
    /// behind the crosshair).
    pub price_text: WidgetLabel,
}

/// Build the current-price indicator from chart data + camera.
///
/// Returns `None` when there is no current price to indicate:
/// * empty `data`,
/// * non-finite close,
/// * viewport degenerate (width < `PRICELINE_WIDTH`).
pub fn compute_current_price_indicator(
    data: &dyn CandleData,
    camera: &Camera2D,
    viewport_width: f32,
    bull_color: [f32; 4],
    bear_color: [f32; 4],
) -> Option<CurrentPriceIndicator> {
    if data.is_empty() {
        return None;
    }
    let last = data.len() - 1;
    let close = data.close(last);
    let open = data.open(last);
    if !close.is_finite() {
        return None;
    }
    let priceline_x = viewport_width - PRICELINE_WIDTH;
    if priceline_x <= 0.0 {
        return None;
    }

    let y = camera.snap_to_pixel(camera.price_to_y(close as f64));

    // Pick bull/bear from the last candle's direction. Doji (open ==
    // close) goes bull — matches the candle pipeline's convention.
    let direction_color = if close >= open {
        bull_color
    } else {
        bear_color
    };
    let line_color = [
        direction_color[0],
        direction_color[1],
        direction_color[2],
        // Very dim — the line is a hint, not a focal element.
        0.30,
    ];

    let line_dots = build_dots(priceline_x, y, line_color);
    let badge = build_badge(viewport_width, y, direction_color);
    let price_text = build_text(viewport_width, y, close, direction_color);

    Some(CurrentPriceIndicator {
        line_dots,
        badge,
        price_text,
    })
}

fn build_dots(priceline_x: f32, y: f32, color: [f32; 4]) -> Vec<GridLineInstance> {
    let right = priceline_x - RIGHT_INSET_PX;
    if right <= 0.0 {
        return Vec::new();
    }
    let dot_count = (right / DOT_PERIOD_PX).ceil() as usize;
    let mut out = Vec::with_capacity(dot_count);
    let mut x = 0.0_f32;
    while x + DOT_WIDTH_PX <= right {
        out.push(GridLineInstance {
            rect: [
                x,
                y - DOT_HEIGHT_PX * 0.5,
                x + DOT_WIDTH_PX,
                y + DOT_HEIGHT_PX * 0.5,
            ],
            color,
        });
        x += DOT_PERIOD_PX;
    }
    out
}

fn build_badge(viewport_width: f32, y: f32, fill: [f32; 4]) -> BadgeInstance {
    // Body sits inside the right-side priceline gutter, flush against
    // the border line. No triangular nose — `shape_id = 0` (Rect).
    let body_left = viewport_width - PRICELINE_WIDTH;
    let body_right = viewport_width;
    let half_h = BADGE_HEIGHT_PX * 0.5;
    BadgeInstance {
        rect: [body_left, y - half_h, body_right, y + half_h],
        fill: [fill[0], fill[1], fill[2], 1.0],
        border: [0.0; 4],
        shape_id: 0, // BadgeShape::Rect — see midas_gpu_types::BadgeInstance docs.
        shape_param: 0.0,
        border_thickness: 0.0,
        _pad: 0.0,
    }
}

fn build_text(viewport_width: f32, y: f32, price: f32, fill: [f32; 4]) -> WidgetLabel {
    WidgetLabel {
        text: format!("{price:.2}"),
        screen_x: viewport_width - BADGE_TEXT_RIGHT_INSET_PX,
        screen_y: y,
        bg_color: [0.0; 4],
        text_color: crate::color::contrast_text_color(fill),
        font_size: BADGE_FONT_SIZE_PX,
        anchor: LabelAnchor::Right,
    }
}

#[cfg(test)]
mod tests {
    use std::ops::Range;

    use super::*;

    fn camera(price_low: f64, price_high: f64, vh: u32) -> Camera2D {
        Camera2D {
            time_start: 0.0,
            time_end: 100.0,
            price_low,
            price_high,
            viewport_width: 1000,
            viewport_height: vh,
            dpi_scale: 1.0,
        }
    }

    struct StubData {
        opens: Vec<f32>,
        closes: Vec<f32>,
    }
    impl CandleData for StubData {
        fn len(&self) -> usize {
            self.closes.len()
        }
        fn timestamp(&self, idx: usize) -> i64 {
            idx as i64
        }
        fn open(&self, idx: usize) -> f32 {
            self.opens[idx]
        }
        fn high(&self, idx: usize) -> f32 {
            self.closes[idx].max(self.opens[idx])
        }
        fn low(&self, idx: usize) -> f32 {
            self.closes[idx].min(self.opens[idx])
        }
        fn close(&self, idx: usize) -> f32 {
            self.closes[idx]
        }
        fn volume(&self, _idx: usize) -> u32 {
            100
        }
        fn price_range(&self, range: Range<usize>) -> (f32, f32) {
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for i in range {
                lo = lo.min(self.low(i));
                hi = hi.max(self.high(i));
            }
            (lo, hi)
        }
        fn find_index_by_time(&self, _ts: i64) -> usize {
            0
        }
    }

    #[test]
    fn empty_data_returns_none() {
        let data = StubData {
            opens: vec![],
            closes: vec![],
        };
        let cam = camera(100.0, 110.0, 600);
        let bull = [0.0, 1.0, 0.0, 1.0];
        let bear = [1.0, 0.0, 0.0, 1.0];
        assert!(compute_current_price_indicator(&data, &cam, 1000.0, bull, bear).is_none());
    }

    #[test]
    fn nan_close_returns_none() {
        let data = StubData {
            opens: vec![100.0],
            closes: vec![f32::NAN],
        };
        let cam = camera(99.0, 101.0, 600);
        let bull = [0.0, 1.0, 0.0, 1.0];
        let bear = [1.0, 0.0, 0.0, 1.0];
        assert!(compute_current_price_indicator(&data, &cam, 1000.0, bull, bear).is_none());
    }

    #[test]
    fn bull_close_picks_bull_color() {
        let data = StubData {
            opens: vec![100.0],
            closes: vec![100.5],
        };
        let cam = camera(99.0, 101.0, 600);
        let bull = [0.1, 0.8, 0.2, 1.0];
        let bear = [0.9, 0.1, 0.1, 1.0];
        let ind =
            compute_current_price_indicator(&data, &cam, 1000.0, bull, bear).expect("indicator");
        // Badge fill alpha is forced to 1.0; RGB matches bull.
        assert!((ind.badge.fill[0] - bull[0]).abs() < 1e-6);
        assert!((ind.badge.fill[1] - bull[1]).abs() < 1e-6);
        assert!((ind.badge.fill[2] - bull[2]).abs() < 1e-6);
        // Line color shares RGB but uses dim alpha.
        assert!(ind.line_dots[0].color[3] < 0.5);
    }

    #[test]
    fn bear_close_picks_bear_color() {
        let data = StubData {
            opens: vec![100.0],
            closes: vec![99.5],
        };
        let cam = camera(99.0, 101.0, 600);
        let bull = [0.1, 0.8, 0.2, 1.0];
        let bear = [0.9, 0.1, 0.1, 1.0];
        let ind =
            compute_current_price_indicator(&data, &cam, 1000.0, bull, bear).expect("indicator");
        assert!((ind.badge.fill[0] - bear[0]).abs() < 1e-6);
        assert!((ind.badge.fill[1] - bear[1]).abs() < 1e-6);
        assert!((ind.badge.fill[2] - bear[2]).abs() < 1e-6);
    }

    #[test]
    fn rect_shape_no_pointer() {
        let data = StubData {
            opens: vec![100.0],
            closes: vec![100.5],
        };
        let cam = camera(99.0, 101.0, 600);
        let bull = [0.1, 0.8, 0.2, 1.0];
        let bear = [0.9, 0.1, 0.1, 1.0];
        let ind =
            compute_current_price_indicator(&data, &cam, 1000.0, bull, bear).expect("indicator");
        // shape_id 0 == BadgeShape::Rect — no triangular nose.
        assert_eq!(ind.badge.shape_id, 0);
        assert_eq!(ind.badge.shape_param, 0.0);
    }

    #[test]
    fn dots_stop_before_priceline() {
        let data = StubData {
            opens: vec![100.0],
            closes: vec![100.5],
        };
        let cam = camera(99.0, 101.0, 600);
        let bull = [0.1, 0.8, 0.2, 1.0];
        let bear = [0.9, 0.1, 0.1, 1.0];
        let ind =
            compute_current_price_indicator(&data, &cam, 1000.0, bull, bear).expect("indicator");
        let priceline_x = 1000.0 - PRICELINE_WIDTH;
        for dot in &ind.line_dots {
            assert!(
                dot.rect[2] <= priceline_x - RIGHT_INSET_PX + 0.01,
                "dot at {:?} crossed priceline ({})",
                dot.rect,
                priceline_x
            );
        }
        assert!(!ind.line_dots.is_empty());
    }

    #[test]
    fn narrow_viewport_returns_none() {
        let data = StubData {
            opens: vec![100.0],
            closes: vec![100.5],
        };
        let cam = camera(99.0, 101.0, 600);
        let bull = [0.1, 0.8, 0.2, 1.0];
        let bear = [0.9, 0.1, 0.1, 1.0];
        // viewport_width <= PRICELINE_WIDTH → no price area to draw on.
        assert!(
            compute_current_price_indicator(&data, &cam, PRICELINE_WIDTH, bull, bear).is_none()
        );
    }
}
