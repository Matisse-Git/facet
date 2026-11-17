//! The window/taskbar icon, drawn procedurally: a white faceted gem on an accent-blue rounded
//! square. (The `.ico` resource for the executable itself arrives with the installer in M4.)

use eframe::egui::IconData;

const SIZE: u32 = 256;

pub fn app_icon() -> IconData {
    IconData {
        rgba: render(SIZE),
        width: SIZE,
        height: SIZE,
    }
}

/// Render the icon as straight-alpha RGBA, `size`×`size`, with 4×4 supersampling.
fn render(size: u32) -> Vec<u8> {
    const SAMPLES: u32 = 4;
    let mut rgba = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut sum = [0.0f32; 4];
            for sy in 0..SAMPLES {
                for sx in 0..SAMPLES {
                    let x = (px as f32 + (sx as f32 + 0.5) / SAMPLES as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SAMPLES as f32) / size as f32;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_has_expected_shape() {
        let size = 64;
        let rgba = render(size);
        assert_eq!(rgba.len(), (size * size * 4) as usize);
        let pixel = |x: u32, y: u32| {
            let i = ((y * size + x) * 4) as usize;
            [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
        };
        // Corners are transparent (rounded square), the center is the opaque white gem,
        // and the area beside the gem is opaque blue.
        assert_eq!(pixel(0, 0)[3], 0);
        assert_eq!(pixel(size - 1, size - 1)[3], 0);
        let center = pixel(size / 2, size / 2);
        assert_eq!(center[3], 255);
        assert!(center[0] > 200 && center[1] > 200 && center[2] > 200);
        let side = pixel(size / 8, size / 2);
        assert_eq!(side[3], 255);
        assert!(
            side[2] > side[0] + 80,
            "background should be blue: {side:?}"
        );
    }
}
