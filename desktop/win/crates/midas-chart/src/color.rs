//! Small color utilities shared across the chart core and the app shell.

/// Pick black or white for maximum contrast against `bg`.
///
/// Uses the Rec. 709 relative-luminance formula on the sRGB channels;
/// alpha is ignored. Returns `[f32; 4]` RGBA with alpha `1.0`.
pub fn contrast_text_color(bg: [f32; 4]) -> [f32; 4] {
    let luma = 0.2126 * bg[0] + 0.7152 * bg[1] + 0.0722 * bg[2];
    if luma > 0.5 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        [1.0, 1.0, 1.0, 1.0]
    }
}
