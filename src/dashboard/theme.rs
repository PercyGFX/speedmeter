//! Colors, type and egui style for the dashboard.
//!
//! The window takes Windows 11's own neutrals (it sits next to the OS chrome and its
//! title bar is colored to match), so the only saturated color in the app is the
//! heat scale: blue, amber, red. Numbers use Bahnschrift, the DIN-style face that
//! ships with Windows, for an instrument-readout feel. Everything else is Segoe UI.

use crate::heat;
use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Stroke, Style,
    TextStyle, Theme, Visuals,
};
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub dark: bool,
    /// Window background and navigation rail; matches the title bar.
    pub base: Color32,
    /// The content pane.
    pub content: Color32,
    /// Raised rows inside the content pane (settings).
    pub layer: Color32,
    pub hover: Color32,
    pub selected: Color32,
    /// Hairlines and borders.
    pub line: Color32,
    pub edge: Color32,
    pub tip: Color32,
    pub text: Color32,
    pub dim: Color32,
    pub faint: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub calm: Color32,
    pub busy: Color32,
    pub heavy: Color32,
    pub extreme: Color32,
}

pub fn rgb(c: [u8; 3]) -> Color32 {
    Color32::from_rgb(c[0], c[1], c[2])
}

pub fn to_rgb(c: Color32) -> [u8; 3] {
    [c.r(), c.g(), c.b()]
}

/// Blend `c` toward `toward` by `t` (0..1), fully opaque.
pub fn blend(c: Color32, toward: Color32, t: f32) -> Color32 {
    let m = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
    Color32::from_rgb(m(c.r(), toward.r()), m(c.g(), toward.g()), m(c.b(), toward.b()))
}

impl Palette {
    pub fn dark() -> Self {
        let r = &heat::DARK;
        Self {
            dark: true,
            base: Color32::from_rgb(0x20, 0x20, 0x20),
            content: Color32::from_rgb(0x27, 0x27, 0x27),
            layer: Color32::from_rgb(0x2D, 0x2D, 0x2D),
            hover: Color32::from_rgb(0x33, 0x33, 0x33),
            selected: Color32::from_rgb(0x38, 0x38, 0x38),
            line: Color32::from_rgb(0x3A, 0x3A, 0x3A),
            edge: Color32::from_rgb(0x1B, 0x1B, 0x1B),
            tip: Color32::from_rgb(0x2C, 0x2C, 0x2C),
            text: Color32::from_rgb(0xF3, 0xF3, 0xF3),
            dim: Color32::from_rgb(0xAD, 0xAD, 0xAD),
            faint: Color32::from_rgb(0x7A, 0x7A, 0x7A),
            accent: Color32::from_rgb(0x4C, 0xC2, 0xFF),
            on_accent: Color32::from_rgb(0x00, 0x00, 0x00),
            calm: rgb(r.calm),
            busy: rgb(r.busy),
            heavy: rgb(r.heavy),
            extreme: rgb(r.extreme),
        }
    }

    pub fn light() -> Self {
        let r = &heat::LIGHT;
        Self {
            dark: false,
            base: Color32::from_rgb(0xF0, 0xF0, 0xF0),
            content: Color32::from_rgb(0xF9, 0xF9, 0xF9),
            layer: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            hover: Color32::from_rgb(0xEE, 0xEE, 0xEE),
            selected: Color32::from_rgb(0xE6, 0xE6, 0xE6),
            line: Color32::from_rgb(0xE3, 0xE3, 0xE3),
            edge: Color32::from_rgb(0xDD, 0xDD, 0xDD),
            tip: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            text: Color32::from_rgb(0x1B, 0x1B, 0x1B),
            dim: Color32::from_rgb(0x5F, 0x5F, 0x5F),
            faint: Color32::from_rgb(0x8E, 0x8E, 0x8E),
            accent: Color32::from_rgb(0x00, 0x5F, 0xB8),
            on_accent: Color32::from_rgb(0xFF, 0xFF, 0xFF),
            calm: rgb(r.calm),
            busy: rgb(r.busy),
            heavy: rgb(r.heavy),
            extreme: rgb(r.extreme),
        }
    }

    /// A darker (or, on light, paler) version of a heat color, used for upload bars
    /// so the two directions read apart while sharing the scale.
    pub fn quiet(&self, c: Color32) -> Color32 {
        blend(c, self.content, 0.38)
    }
}

pub const DISPLAY: &str = "display";
pub const STRONG: &str = "strong";
pub const ICONS: &str = "icons";

pub fn body(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}

pub fn strong(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(STRONG.into()))
}

/// Bahnschrift, for numbers and readouts.
pub fn display(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(DISPLAY.into()))
}

pub fn icon(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(ICONS.into()))
}

/// Loads Windows' own fonts. Returns whether the Fluent icon font was found.
pub fn install_fonts(ctx: &egui::Context) -> bool {
    let mut fonts = FontDefinitions::default();
    let fallback: Vec<String> = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts");
    let mut load = |key: &str, files: &[&str]| -> bool {
        for f in files {
            if let Ok(bytes) = std::fs::read(dir.join(f)) {
                fonts.font_data.insert(key.to_string(), Arc::new(FontData::from_owned(bytes)));
                return true;
            }
        }
        false
    };
    let segoe = load("segoe", &["segoeui.ttf"]);
    let segoe_sb = load("segoe_sb", &["seguisb.ttf", "segoeuib.ttf"]);
    let bahn = load("bahn", &["bahnschrift.ttf"]);
    let icons = load("fluent", &["SegoeIcons.ttf", "segmdl2.ttf"]);

    let chain = |first: &[(&str, bool)]| -> Vec<String> {
        let mut v: Vec<String> = first.iter().filter(|(_, ok)| *ok).map(|(k, _)| k.to_string()).collect();
        v.extend(fallback.iter().cloned());
        v
    };
    let prop = chain(&[("segoe", segoe)]);
    let strong_fam = chain(&[("segoe_sb", segoe_sb), ("segoe", segoe)]);
    let display_fam = chain(&[("bahn", bahn), ("segoe", segoe)]);
    let icon_fam = chain(&[("fluent", icons)]);
    fonts.families.insert(FontFamily::Proportional, prop);
    fonts.families.insert(FontFamily::Name(STRONG.into()), strong_fam);
    fonts.families.insert(FontFamily::Name(DISPLAY.into()), display_fam);
    fonts.families.insert(FontFamily::Name(ICONS.into()), icon_fam);
    ctx.set_fonts(fonts);
    icons
}

fn visuals(p: &Palette) -> Visuals {
    let mut v = if p.dark { Visuals::dark() } else { Visuals::light() };
    let r = CornerRadius::same(6);
    v.panel_fill = p.base;
    v.window_fill = p.tip;
    v.extreme_bg_color = p.base;
    v.faint_bg_color = p.layer;
    v.code_bg_color = p.layer;
    v.hyperlink_color = p.accent;
    v.selection.bg_fill = p.accent;
    v.selection.stroke = Stroke::new(1.0_f32, p.on_accent);
    v.window_stroke = Stroke::new(1.0_f32, p.line);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(8);
    v.slider_trailing_fill = true;

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.layer;
    w.noninteractive.weak_bg_fill = p.layer;
    w.noninteractive.bg_stroke = Stroke::new(1.0_f32, p.line);
    w.noninteractive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.noninteractive.corner_radius = r;

    w.inactive.bg_fill = p.selected;
    w.inactive.weak_bg_fill = p.selected;
    w.inactive.bg_stroke = Stroke::new(1.0_f32, p.line);
    w.inactive.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.inactive.corner_radius = r;

    w.hovered.bg_fill = blend(p.selected, p.text, 0.06);
    w.hovered.weak_bg_fill = blend(p.selected, p.text, 0.06);
    w.hovered.bg_stroke = Stroke::new(1.0_f32, p.faint);
    w.hovered.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.hovered.corner_radius = r;
    w.hovered.expansion = 0.0;

    w.active.bg_fill = p.selected;
    w.active.weak_bg_fill = p.selected;
    w.active.bg_stroke = Stroke::new(1.0_f32, p.accent);
    w.active.fg_stroke = Stroke::new(1.0_f32, p.text);
    w.active.corner_radius = r;
    w.active.expansion = 0.0;

    w.open = w.active;
    v
}

pub fn apply(ctx: &egui::Context, p: &Palette) {
    let mut style = Style::default();
    style.visuals = visuals(p);
    style.text_styles = [
        (TextStyle::Heading, strong(22.0)),
        (TextStyle::Body, body(14.0)),
        (TextStyle::Button, body(14.0)),
        (TextStyle::Small, body(12.0)),
        (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
    ]
    .into();
    style.spacing.item_spacing = egui::vec2(8.0, 6.0);
    style.spacing.button_padding = egui::vec2(14.0, 6.0);
    style.spacing.interact_size = egui::vec2(40.0, 30.0);
    style.spacing.slider_width = 220.0;
    style.spacing.combo_width = 200.0;
    style.interaction.selectable_labels = false;
    ctx.set_style_of(Theme::Dark, style.clone());
    ctx.set_style_of(Theme::Light, style);
    ctx.set_theme(if p.dark { Theme::Dark } else { Theme::Light });
}
