//! The window/taskbar icon. The drawing lives in `icon_art.rs`, shared with the `.ico` resource
//! that `build.rs` embeds in the executable and the MSIX logos.

use eframe::egui::IconData;

use crate::icon_art::render;

const SIZE: u32 = 256;

pub fn app_icon() -> IconData {
    IconData {
        rgba: render(SIZE),
        width: SIZE,
        height: SIZE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icon_art::render_on;

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

    #[test]
    fn wide_canvas_centers_the_icon() {
        let (width, height, icon) = (40, 20, 20);
        let wide = render_on(width, height, icon);
        let square = render(icon);
        assert_eq!(wide.len(), (width * height * 4) as usize);
        for y in 0..height {
            let row = &wide[(y * width * 4) as usize..((y + 1) * width * 4) as usize];
            // 10 transparent pixels each side, the square icon's row in the middle.
            assert!(row[..40].iter().all(|&b| b == 0));
            assert!(row[120..].iter().all(|&b| b == 0));
            assert_eq!(
                &row[40..120],
                &square[(y * icon * 4) as usize..((y + 1) * icon * 4) as usize]
            );
        }
    }
}
