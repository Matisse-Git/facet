//! The app icon, drawn procedurally: a white faceted gem on an accent-blue rounded square.
//!
//! No dependencies, so the same drawing is shared by the window icon (`icon.rs`), the `.ico`
//! resource embedded by `build.rs`, and the MSIX logos written by `facet-bench package`
//! (both include this file with `#[path]`).

/// Render the icon as straight-alpha RGBA, `size`×`size`, with 4×4 supersampling.
pub fn render(size: u32) -> Vec<u8> {
    render_on(size, size, size)
}

/// Render the icon `icon`×`icon` centered on a transparent `width`×`height` canvas (the
/// MSIX wide tile). Straight-alpha RGBA.
pub fn render_on(width: u32, height: u32, icon: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let (left, top) = ((width - icon) / 2, (height - icon) / 2);
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for py in 0..height {
        for px in 0..width {
            if px < left || px >= left + icon || py < top || py >= top + icon {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            let (ix, iy) = (px - left, py - top);
            let mut sum = [0.0f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (ix as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / icon as f32;
                    let y = (iy as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / icon as f32;
                    let sample = shade(x, y);
                    // Accumulate premultiplied so edges do not fringe.
                    sum[0] += sample[0] * sample[3];
                    sum[1] += sample[1] * sample[3];
                    sum[2] += sample[2] * sample[3];
                    sum[3] += sample[3];
                }
            }
            let count = (SAMPLES * SAMPLES) as f32;
            let alpha = sum[3] / count;
            let channel = |value: f32| {
                if alpha > 0.0 {
                    ((value / count / alpha).clamp(0.0, 1.0) * 255.0).round() as u8
                } else {
                    0
                }
            };
            rgba.extend_from_slice(&[
                channel(sum[0]),
                channel(sum[1]),
                channel(sum[2]),
                (alpha * 255.0).round() as u8,
            ]);
        }
    }
    rgba
}

/// Color at normalized coordinates (0..1, y down) as straight RGBA in 0..1.
fn shade(x: f32, y: f32) -> [f32; 4] {
    // Rounded-square background.
    let inset = 0.06;
    let corner = 0.22;
    let half = 0.5 - inset;
    let (dx, dy) = (
        (x - 0.5).abs() - (half - corner),
        (y - 0.5).abs() - (half - corner),
    );
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0) - corner;
    if outside > 0.0 {
        return [0.0, 0.0, 0.0, 0.0];
    }
    // Vertical gradient: bright accent blue to a deeper blue.
    let top = [0.16, 0.56, 1.0];
    let bottom = [0.02, 0.36, 0.86];
    let mut color = [
        top[0] + (bottom[0] - top[0]) * y,
        top[1] + (bottom[1] - top[1]) * y,
        top[2] + (bottom[2] - top[2]) * y,
    ];

    // The gem: a kite with a flat crown. Vertices in normalized space.
    let crown_y = 0.40;
    let top_y = 0.27;
    let bottom_y = 0.76;
    let (left, right) = (0.22, 0.78);
    let (crown_left, crown_right) = (0.34, 0.66);

    let inside_gem = if (top_y..=crown_y).contains(&y) {
        // Trapezoid crown.
        let t = (y - top_y) / (crown_y - top_y);
        let l = crown_left + (left - crown_left) * t;
        let r = crown_right + (right - crown_right) * t;
        (l..=r).contains(&x)
    } else if y > crown_y && y <= bottom_y {
        // Triangular pavilion converging to the bottom point.
        let t = (y - crown_y) / (bottom_y - crown_y);
        let l = left + (0.5 - left) * t;
        let r = right + (0.5 - right) * t;
        (l..=r).contains(&x)
    } else {
        false
    };

    if inside_gem {
        // Facets: three vertical bands, each a slightly different white.
        let band = if y <= crown_y {
            ((x - left) / (right - left) * 3.0).floor()
        } else {
            let t = (y - crown_y) / (bottom_y - crown_y);
            let l = left + (0.5 - left) * t;
            let r = right + (0.5 - right) * t;
            ((x - l) / (r - l).max(1e-4) * 3.0).floor()
        };
        let brightness = match band as i32 {
            0 => 0.86,
            1 => 1.0,
            _ => 0.92,
        } - if y <= crown_y { 0.0 } else { 0.05 };
        color = [brightness, brightness, (brightness + 0.04).min(1.0)];
    }
    [color[0], color[1], color[2], 1.0]
}
