//! Facet's design system: color palettes (light/dark), type scale, spacing grid, radii, motion.
//!
//! Every view takes its colors from [`Palette`] and its metrics from the constants here, so the
//! whole app stays on one 8pt grid with one accent color. Nothing else should hard-code a color.

use std::sync::Arc;

use eframe::egui::{
    self, epaint::text::VariationCoords, Color32, FontData, FontDefinitions, FontFamily, FontId,
    Stroke, TextStyle, Theme,
};

use crate::settings::{CodeFont, ThemeMode};

/// Height of the custom title bar, in logical points.
pub const TITLE_BAR_HEIGHT: f32 = 48.0;
/// Height of a standard list row / control, in logical points.
pub const ROW_HEIGHT: f32 = 32.0;

/// The 8pt spacing grid (with a 4pt half-step).
pub mod space {
    pub const XS: f32 = 4.0;
    pub const S: f32 = 8.0;
    pub const M: f32 = 16.0;
    pub const L: f32 = 24.0;
    pub const XL: f32 = 32.0;
}

/// Corner radii.
pub mod radius {
    pub const S: u8 = 4;
    pub const M: u8 = 6;
    pub const L: u8 = 10;
    /// The content layer's top-left corner and cards (settings sections, dialog fields).
    pub const LAYER: u8 = 8;
}

/// Motion: short, ease-out, never bouncy.
pub mod motion {
    /// Hover/press feedback and small reveals.
    pub const FAST: f32 = 0.12;
    /// Panels, overlays, toasts.
    pub const NORMAL: f32 = 0.15;

    /// Cubic ease-out.
    pub fn ease_out(t: f32) -> f32 {
        let inv = 1.0 - t.clamp(0.0, 1.0);
        1.0 - inv * inv * inv
    }
}

/// Glyphs from Segoe Fluent Icons (ships with Windows 11).
pub mod icon {
    pub const ADD: char = '\u{E710}';
    pub const CLOSE: char = '\u{E711}';
    pub const SEARCH: char = '\u{E721}';
    pub const FOLDER: char = '\u{E8B7}';
    pub const FOLDER_OPEN: char = '\u{E838}';
    pub const REFRESH: char = '\u{E72C}';
    pub const COPY: char = '\u{E8C8}';
    pub const DELETE: char = '\u{E74D}';
    pub const ERROR: char = '\u{E783}';
    pub const INFO: char = '\u{E946}';
    pub const CHEVRON_DOWN: char = '\u{E70D}';
    pub const CHEVRON_UP: char = '\u{E70E}';
    /// GripperBarHorizontal: a drag handle.
    pub const GRIPPER: char = '\u{E76F}';
    pub const CHEVRON_RIGHT: char = '\u{E76C}';
    pub const REMOVE: char = '\u{E738}';
    pub const UNDO: char = '\u{E7A7}';
    pub const CHECK: char = '\u{E73E}';
    pub const DOCUMENT: char = '\u{E8A5}';
    pub const LOCK: char = '\u{E72E}';
    pub const UNLOCK: char = '\u{E785}';
    pub const SETTINGS: char = '\u{E713}';
    /// OpenInNewWindow.
    pub const OPEN_IN: char = '\u{E8A7}';
    /// Sync (two arrows in a circle).
    pub const SYNC: char = '\u{E895}';
    /// BranchFork2.
    pub const BRANCH: char = '\u{F003}';
    /// Page: the Changes view.
    pub const CHANGES: char = '\u{E8A5}';
    /// Recent (clock): the History view.
    pub const HISTORY: char = '\u{E81C}';
    /// Download: Clone.
    pub const DOWNLOAD: char = '\u{E896}';
    /// NewFolder: New repository.
    pub const NEW_FOLDER: char = '\u{E8F4}';
}

const FAMILY_SEMIBOLD: &str = "facet-semibold";
const FAMILY_DISPLAY: &str = "facet-display";
const FAMILY_ICONS: &str = "facet-icons";

/// Body text.
pub fn body() -> FontId {
    FontId::new(14.0, FontFamily::Proportional)
}

/// Secondary text: captions, metadata, section labels.
pub fn small() -> FontId {
    FontId::new(12.0, FontFamily::Proportional)
}

/// Emphasized body text (active tab, row titles).
pub fn strong() -> FontId {
    FontId::new(14.0, FontFamily::Name(FAMILY_SEMIBOLD.into()))
}

/// Emphasized secondary text (section headers).
pub fn small_strong() -> FontId {
    FontId::new(12.0, FontFamily::Name(FAMILY_SEMIBOLD.into()))
}

/// Large display text (repository name, welcome title).
pub fn title() -> FontId {
    FontId::new(28.0, FontFamily::Name(FAMILY_DISPLAY.into()))
}

/// Dialog and card titles.
pub fn heading() -> FontId {
    FontId::new(20.0, FontFamily::Name(FAMILY_DISPLAY.into()))
}

/// Code, object ids, command lines.
pub fn mono() -> FontId {
    FontId::new(13.0, FontFamily::Monospace)
}

/// An icon glyph at `size` points.
pub fn icon_font(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_ICONS.into()))
}

/// All colors used by the UI, for one appearance.
#[derive(Debug)]
pub struct Palette {
    pub dark: bool,
    /// Solid surface for content areas (lists, diffs, detail views). Chrome areas (title bar,
    /// sidebar) are left transparent so the Mica backdrop shows through.
    pub surface: Color32,
    /// The quieter second surface inside content (file list column, settings cards).
    pub surface_alt: Color32,
    /// Surface for things that float above content: palette, toasts, tooltips.
    pub raised: Color32,
    /// 1px dividers and outlines.
    pub hairline: Color32,
    pub text: Color32,
    pub text_secondary: Color32,
    pub text_tertiary: Color32,
    /// The one accent color.
    pub accent: Color32,
    /// Text/icons drawn on top of `accent`.
    pub on_accent: Color32,
    /// Selected rows, focused fields.
    pub accent_soft: Color32,
    /// Hovered row/button fill.
    pub hover: Color32,
    /// Pressed row/button fill; also the active tab.
    pub pressed: Color32,
    pub danger: Color32,
    pub success: Color32,
    pub warning: Color32,
    /// Skeleton placeholder base and shimmer highlight.
    pub skeleton: Color32,
    pub skeleton_highlight: Color32,
    /// Dim layer behind modal-ish overlays (command palette).
    pub scrim: Color32,
    pub shadow: Color32,
    /// Diff: row tint of added / removed lines, and the stronger tint of the words that
    /// changed inside them.
    pub diff_added: Color32,
    pub diff_added_strong: Color32,
    pub diff_removed: Color32,
    pub diff_removed_strong: Color32,
    /// Diff: hunk header row.
    pub diff_hunk: Color32,
}

/// Straight-alpha color to egui's premultiplied `Color32`, usable in `const`.
const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    const fn mul(c: u8, a: u8) -> u8 {
        ((c as u16 * a as u16 + 127) / 255) as u8
    }
    Color32::from_rgba_premultiplied(mul(r, a), mul(g, a), mul(b, a), a)
}

pub const DARK: Palette = Palette {
    dark: true,
    surface: Color32::from_rgb(0x1C, 0x1C, 0x1C),
    surface_alt: Color32::from_rgb(0x24, 0x24, 0x24),
    raised: Color32::from_rgb(0x2B, 0x2B, 0x2B),
    hairline: Color32::from_rgb(0x32, 0x32, 0x32),
    text: Color32::from_rgb(0xF2, 0xF2, 0xF2),
    text_secondary: Color32::from_rgb(0xC4, 0xC4, 0xC4),
    text_tertiary: Color32::from_rgb(0xA3, 0xA3, 0xA3),
    accent: Color32::from_rgb(0x60, 0xCD, 0xFF),
    on_accent: Color32::from_rgb(0x00, 0x26, 0x3F),
    accent_soft: rgba(0x60, 0xCD, 0xFF, 36),
    hover: rgba(255, 255, 255, 16),
    pressed: rgba(255, 255, 255, 26),
    danger: Color32::from_rgb(0xFF, 0x9C, 0x8F),
    success: Color32::from_rgb(0x6C, 0xCB, 0x8A),
    warning: Color32::from_rgb(0xF2, 0xC6, 0x61),
    skeleton: rgba(255, 255, 255, 14),
    skeleton_highlight: rgba(255, 255, 255, 30),
    scrim: rgba(0, 0, 0, 110),
    shadow: rgba(0, 0, 0, 128),
    diff_added: rgba(0x2E, 0xA0, 0x43, 33),
    diff_added_strong: rgba(0x2E, 0xA0, 0x43, 97),
    diff_removed: rgba(0xF8, 0x51, 0x49, 31),
    diff_removed_strong: rgba(0xF8, 0x51, 0x49, 92),
    diff_hunk: rgba(0x60, 0xCD, 0xFF, 24),
};

pub const LIGHT: Palette = Palette {
    dark: false,
    surface: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    surface_alt: Color32::from_rgb(0xF9, 0xF9, 0xF9),
    raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    hairline: Color32::from_rgb(0xE5, 0xE5, 0xE5),
    text: Color32::from_rgb(0x1A, 0x1A, 0x1A),
    text_secondary: Color32::from_rgb(0x5C, 0x5C, 0x5C),
    text_tertiary: Color32::from_rgb(0x6B, 0x6B, 0x6B),
    accent: Color32::from_rgb(0x00, 0x67, 0xC0),
    on_accent: Color32::WHITE,
    accent_soft: rgba(0x00, 0x67, 0xC0, 26),
    hover: rgba(0, 0, 0, 20),
    pressed: rgba(0, 0, 0, 30),
    danger: Color32::from_rgb(0xC4, 0x2B, 0x1C),
    success: Color32::from_rgb(0x0F, 0x7B, 0x3F),
    warning: Color32::from_rgb(0x8A, 0x5A, 0x00),
    skeleton: rgba(0, 0, 0, 14),
    skeleton_highlight: rgba(0, 0, 0, 28),
    scrim: rgba(0, 0, 0, 56),
    shadow: rgba(0, 0, 0, 40),
    diff_added: rgba(0x2E, 0xA0, 0x43, 26),
    diff_added_strong: rgba(0x2E, 0xA0, 0x43, 72),
    diff_removed: rgba(0xE5, 0x3E, 0x2E, 22),
    diff_removed_strong: rgba(0xE5, 0x3E, 0x2E, 62),
    diff_hunk: rgba(0x00, 0x67, 0xC0, 18),
};

impl Palette {
    /// The palette for the appearance currently in effect.
    pub fn of(ctx: &egui::Context) -> &'static Self {
        match ctx.theme() {
            Theme::Dark => &DARK,
            Theme::Light => &LIGHT,
        }
    }

    /// 1px stroke in the hairline color.
    pub fn hairline_stroke(&self) -> Stroke {
        Stroke::new(1.0, self.hairline)
    }
}

/// Multiply a color's opacity (for fades).
pub fn fade(color: Color32, opacity: f32) -> Color32 {
    color.gamma_multiply(opacity.clamp(0.0, 1.0))
}

/// Tell egui which appearance to use.
pub fn set_mode(ctx: &egui::Context, mode: ThemeMode) {
    ctx.set_theme(match mode {
        ThemeMode::System => egui::ThemePreference::System,
        ThemeMode::Light => egui::ThemePreference::Light,
        ThemeMode::Dark => egui::ThemePreference::Dark,
    });
}

/// Install fonts and both styles. Call once at startup.
pub fn install(ctx: &egui::Context, mode: ThemeMode, code_font: CodeFont) {
    ctx.set_fonts(font_definitions(code_font));
    ctx.style_mut_of(Theme::Dark, |style| configure(style, &DARK));
    ctx.style_mut_of(Theme::Light, |style| configure(style, &LIGHT));
    set_mode(ctx, mode);
}

fn configure(style: &mut egui::Style, palette: &Palette) {
    style.text_styles = [
        (TextStyle::Body, body()),
        (TextStyle::Button, body()),
        (TextStyle::Small, small()),
        (TextStyle::Heading, title()),
        (TextStyle::Monospace, mono()),
    ]
    .into();

    style.spacing.item_spacing = egui::vec2(space::S, space::S);
    style.spacing.button_padding = egui::vec2(space::M - space::XS, 6.0);
    style.spacing.interact_size.y = ROW_HEIGHT - space::XS;
    style.spacing.scroll = egui::style::ScrollStyle::floating();
    style.interaction.selectable_labels = false;
    style.animation_time = motion::FAST;

    let visuals = &mut style.visuals;
    visuals.panel_fill = Color32::TRANSPARENT;
    visuals.window_fill = palette.raised;
    visuals.window_stroke = palette.hairline_stroke();
    visuals.window_corner_radius = radius::L.into();
    visuals.menu_corner_radius = radius::M.into();
    visuals.extreme_bg_color = Color32::TRANSPARENT;
    visuals.faint_bg_color = palette.hover;
    visuals.hyperlink_color = palette.accent;
    visuals.selection.bg_fill = palette.accent_soft;
    visuals.selection.stroke = Stroke::new(1.0, palette.accent);
    visuals.text_cursor.stroke = Stroke::new(1.5, palette.accent);
    visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 28,
        spread: 0,
        color: palette.shadow,
    };
    visuals.popup_shadow = egui::Shadow {
        offset: [0, 4],
        blur: 16,
        spread: 0,
        color: palette.shadow,
    };

    let widgets = &mut visuals.widgets;
    widgets.noninteractive.fg_stroke = Stroke::new(1.0, palette.text);
    widgets.noninteractive.bg_stroke = palette.hairline_stroke();
    for (state, fill) in [
        (&mut widgets.inactive, Color32::TRANSPARENT),
        (&mut widgets.hovered, palette.hover),
        (&mut widgets.active, palette.pressed),
        (&mut widgets.open, palette.pressed),
    ] {
        state.bg_fill = fill;
        state.weak_bg_fill = fill;
        state.bg_stroke = Stroke::NONE;
        state.fg_stroke = Stroke::new(1.0, palette.text);
        state.corner_radius = radius::M.into();
        state.expansion = 0.0;
    }
}

/// Segoe UI Variable for text, Segoe Fluent Icons for glyphs, `code_font` (falling back to the
/// other code fonts) for code, with egui's bundled fonts kept as a last-resort fallback if a
/// system font is missing. Reads the font files: call it off the UI thread after startup.
pub fn font_definitions(code_font: CodeFont) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let dir = std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| r"C:\Windows".into())
        .join("Fonts");
    let load = |file: &str| std::fs::read(dir.join(file)).ok().map(FontData::from_owned);

    let mut text_chain = Vec::new();
    let mut semibold_chain = Vec::new();
    let mut display_chain = Vec::new();
    let mut icon_chain = Vec::new();
    let mut mono_chain = Vec::new();

    if let Some(segoe) = load("SegUIVar.ttf") {
        // One variable font, three instances along its weight and optical-size axes.
        let instance = |weight: f32, optical_size: f32| {
            let mut data = segoe.clone();
            data.tweak.coords = VariationCoords::new([(b"wght", weight), (b"opsz", optical_size)]);
            Arc::new(data)
        };
        fonts
            .font_data
            .insert("segoe-text".into(), instance(400.0, 10.5));
        fonts
            .font_data
            .insert("segoe-semibold".into(), instance(600.0, 10.5));
        fonts
            .font_data
            .insert("segoe-display".into(), instance(600.0, 36.0));
        text_chain.push("segoe-text".to_owned());
        semibold_chain.push("segoe-semibold".to_owned());
        display_chain.push("segoe-display".to_owned());
    } else {
        // Windows 10 or a stripped image: the static Segoe UI files.
        if let Some(regular) = load("segoeui.ttf") {
            fonts
                .font_data
                .insert("segoe-text".into(), Arc::new(regular));
            text_chain.push("segoe-text".to_owned());
        }
        if let Some(semibold) = load("seguisb.ttf") {
            fonts
                .font_data
                .insert("segoe-semibold".into(), Arc::new(semibold));
            semibold_chain.push("segoe-semibold".to_owned());
            display_chain.push("segoe-semibold".to_owned());
        }
    }

    for file in ["SegoeIcons.ttf", "segmdl2.ttf"] {
        if let Some(icons) = load(file) {
            fonts.font_data.insert(file.into(), Arc::new(icons));
            icon_chain.push(file.to_owned());
        }
    }
    // The chosen font, then Consolas (always installed). Only two are loaded: font data stays
    // resident for the life of the process.
    let mut code_files = vec![code_font.file()];
    if code_font != CodeFont::Consolas {
        code_files.push(CodeFont::Consolas.file());
    }
    for file in code_files {
        if let Some(mut mono) = load(file) {
            // Cascadia is a variable font and reads heavy at small sizes on dark surfaces;
            // SemiLight keeps code and ids crisp. (Ignored by the static Consolas fallback.)
            mono.tweak.coords = VariationCoords::new([(b"wght", 350.0)]);
            fonts.font_data.insert(file.into(), Arc::new(mono));
            mono_chain.push(file.to_owned());
        }
    }

    let default_proportional = fonts
        .families
        .get(&FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let default_mono = fonts
        .families
        .get(&FontFamily::Monospace)
        .cloned()
        .unwrap_or_default();

    let with_fallbacks = |primary: Vec<String>, fallback: &[String]| {
        let mut chain = primary;
        chain.extend(fallback.iter().cloned());
        chain
    };

    fonts.families.insert(
        FontFamily::Proportional,
        with_fallbacks(text_chain.clone(), &default_proportional),
    );
    fonts.families.insert(
        FontFamily::Name(FAMILY_SEMIBOLD.into()),
        with_fallbacks(
            with_fallbacks(semibold_chain, &text_chain),
            &default_proportional,
        ),
    );
    fonts.families.insert(
        FontFamily::Name(FAMILY_DISPLAY.into()),
        with_fallbacks(
            with_fallbacks(display_chain, &text_chain),
            &default_proportional,
        ),
    );
    fonts.families.insert(
        FontFamily::Name(FAMILY_ICONS.into()),
        with_fallbacks(icon_chain, &default_proportional),
    );
    fonts.families.insert(
        FontFamily::Monospace,
        with_fallbacks(with_fallbacks(mono_chain, &text_chain), &default_mono),
    );
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_is_monotonic_and_bounded() {
        assert_eq!(motion::ease_out(0.0), 0.0);
        assert_eq!(motion::ease_out(1.0), 1.0);
        assert_eq!(motion::ease_out(2.0), 1.0);
        let mut last = 0.0;
        for step in 1..=20 {
            let value = motion::ease_out(step as f32 / 20.0);
            assert!(value >= last);
            last = value;
        }
        // Ease-out front-loads the movement.
        assert!(motion::ease_out(0.5) > 0.5);
    }

    #[test]
    fn premultiplication_matches_egui() {
        let ours = rgba(10, 132, 255, 64);
        let theirs = Color32::from_rgba_unmultiplied(10, 132, 255, 64);
        for (a, b) in ours.to_array().into_iter().zip(theirs.to_array()) {
            assert!(a.abs_diff(b) <= 1, "{ours:?} vs {theirs:?}");
        }
        assert_eq!(rgba(1, 2, 3, 255), Color32::from_rgb(1, 2, 3));
        assert_eq!(rgba(200, 200, 200, 0), Color32::TRANSPARENT);
    }

    #[test]
    fn text_contrast_is_readable_on_content_surfaces() {
        // WCAG relative luminance contrast of primary text against the content surface.
        fn luminance(c: Color32) -> f32 {
            let channel = |v: u8| {
                let v = f32::from(v) / 255.0;
                if v <= 0.03928 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * channel(c.r()) + 0.7152 * channel(c.g()) + 0.0722 * channel(c.b())
        }
        for palette in [&DARK, &LIGHT] {
            let (a, b) = (luminance(palette.text), luminance(palette.surface));
            let ratio = (a.max(b) + 0.05) / (a.min(b) + 0.05);
            assert!(ratio >= 7.0, "contrast {ratio} too low for {palette:?}");
        }
    }
}
