//! Draws the widget with Direct2D + DirectWrite into a 32-bit premultiplied bitmap,
//! which the widget window shows with UpdateLayeredWindow. Per-pixel alpha means
//! text edges blend cleanly into the translucent Windows 11 taskbar.
//!
//! Two looks share this code:
//! * still (default): a plain meter, compact, redrawn once a second;
//! * animated: numbers roll between readings, the busy row grows on a spring,
//!   colors blend continuously and fine streaks trail behind heavy traffic.

use crate::config::{Config, Layout};
use crate::format::{self, Decimals, UnitMode};
use crate::heat;
use crate::sys::wide;
use windows::core::{w, Result, PCWSTR};
use windows::Foundation::Numerics::Matrix3x2;
use windows::Win32::Foundation::{BOOL, RECT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT, D2D_POINT_2F, D2D_RECT_F,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1DCRenderTarget, ID2D1Factory, ID2D1SolidColorBrush, ID2D1StrokeStyle,
    D2D1_CAP_STYLE_ROUND, D2D1_DASH_STYLE_SOLID, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_FEATURE_LEVEL_DEFAULT, D2D1_LINE_JOIN_ROUND,
    D2D1_RENDER_TARGET_PROPERTIES, D2D1_RENDER_TARGET_TYPE_SOFTWARE,
    D2D1_RENDER_TARGET_USAGE_NONE, D2D1_ROUNDED_RECT, D2D1_STROKE_STYLE_PROPERTIES,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    IDWriteTextLayout, IDWriteTypography, DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_FEATURE,
    DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, DWRITE_FONT_STRETCH, DWRITE_FONT_STRETCH_NORMAL,
    DWRITE_FONT_STRETCH_SEMI_CONDENSED, DWRITE_FONT_STYLE_NORMAL, DWRITE_FONT_WEIGHT,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_ALIGNMENT_TRAILING, DWRITE_TEXT_METRICS,
    DWRITE_TEXT_RANGE, DWRITE_WORD_WRAPPING_NO_WRAP,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HDC, HGDIOBJ,
};

pub use crate::anim::{RowState, MAX_GROW};

pub struct Scene<'a> {
    pub down: RowState,
    pub up: RowState,
    pub cfg: &'a Config,
    /// Dark taskbar (light text) or light taskbar (dark text).
    pub dark: bool,
    pub floating: bool,
    /// Hover highlight strength, 0 to 1.
    pub hover: f32,
    pub scale: f32,
    /// Fixed pixel height (the taskbar's), or None to size to content.
    pub height: Option<i32>,
}

struct Surface {
    dc: HDC,
    bmp: HBITMAP,
    old: HGDIOBJ,
    w: i32,
    h: i32,
}

impl Drop for Surface {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.dc, self.old);
            let _ = DeleteObject(self.bmp);
            let _ = DeleteDC(self.dc);
        }
    }
}

#[derive(Clone, PartialEq)]
struct FontKey {
    family: String,
    size: u32,
    decimals: Decimals,
    unit: UnitMode,
}

struct Fonts {
    key: FontKey,
    value: IDWriteTextFormat,
    unit: IDWriteTextFormat,
    value_w: f32,
    unit_w: f32,
    line_h: f32,
}

struct Geo {
    pad_x: f32,
    pad_y: f32,
    arrow: f32,
    gap1: f32,
    gap2: f32,
    /// Width of one row at rest.
    row_w: f32,
    /// Height of one row at rest.
    row_h: f32,
    /// Width reserved for one row including growth and streak room.
    group_w: f32,
    group_gap: f32,
    content_w: f32,
    content_h: f32,
}

pub struct Renderer {
    d2d: ID2D1Factory,
    dw: IDWriteFactory,
    rt: Option<ID2D1DCRenderTarget>,
    surface: Option<Surface>,
    fonts: Option<Fonts>,
    stroke: Option<ID2D1StrokeStyle>,
    tnum: IDWriteTypography,
}

fn rgba(rgb: [u8; 3], a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r: rgb[0] as f32 / 255.0, g: rgb[1] as f32 / 255.0, b: rgb[2] as f32 / 255.0, a }
}

fn pt(x: f32, y: f32) -> D2D_POINT_2F {
    D2D_POINT_2F { x, y }
}

fn rrect(l: f32, t: f32, r: f32, b: f32, radius: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: D2D_RECT_F { left: l, top: t, right: r, bottom: b }, radiusX: radius, radiusY: radius }
}

fn scale_about(s: f32, cx: f32, cy: f32) -> Matrix3x2 {
    Matrix3x2 { M11: s, M12: 0.0, M21: 0.0, M22: s, M31: cx * (1.0 - s), M32: cy * (1.0 - s) }
}

const IDENTITY: Matrix3x2 = Matrix3x2 { M11: 1.0, M12: 0.0, M21: 0.0, M22: 1.0, M31: 0.0, M32: 0.0 };

/// Cheap deterministic "randomness" so each streak has its own length and pace.
fn hash01(k: u32) -> f32 {
    let x = k.wrapping_mul(0x9E37_79B9).rotate_left(13).wrapping_mul(0x85EB_CA6B);
    (x >> 8) as f32 / (1u32 << 24) as f32
}

impl Renderer {
    pub fn new() -> Result<Self> {
        unsafe {
            let d2d: ID2D1Factory = D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dw: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let tnum = dw.CreateTypography()?;
            tnum.AddFontFeature(DWRITE_FONT_FEATURE { nameTag: DWRITE_FONT_FEATURE_TAG_TABULAR_FIGURES, parameter: 1 })?;
            Ok(Self { d2d, dw, rt: None, surface: None, fonts: None, stroke: None, tnum })
        }
    }

    fn family_exists(&self, name: &str) -> bool {
        unsafe {
            let mut coll: Option<IDWriteFontCollection> = None;
            if self.dw.GetSystemFontCollection(&mut coll, false).is_err() {
                return false;
            }
            let Some(coll) = coll else { return false };
            let mut index = 0u32;
            let mut exists = BOOL(0);
            let name = wide(name);
            coll.FindFamilyName(PCWSTR(name.as_ptr()), &mut index, &mut exists).is_ok() && exists.as_bool()
        }
    }

    fn format(&self, family: &str, weight: DWRITE_FONT_WEIGHT, stretch: DWRITE_FONT_STRETCH, size: f32, trailing: bool) -> Result<IDWriteTextFormat> {
        unsafe {
            let fam = wide(family);
            let f = self.dw.CreateTextFormat(PCWSTR(fam.as_ptr()), None, weight, DWRITE_FONT_STYLE_NORMAL, stretch, size, w!("en-us"))?;
            f.SetTextAlignment(if trailing { DWRITE_TEXT_ALIGNMENT_TRAILING } else { DWRITE_TEXT_ALIGNMENT_LEADING })?;
            f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
            Ok(f)
        }
    }

    fn layout(&self, text: &str, fmt: &IDWriteTextFormat, w: f32, h: f32) -> Result<IDWriteTextLayout> {
        unsafe {
            let s: Vec<u16> = text.encode_utf16().collect();
            let l = self.dw.CreateTextLayout(&s, fmt, w, h)?;
            l.SetTypography(&self.tnum, DWRITE_TEXT_RANGE { startPosition: 0, length: s.len() as u32 })?;
            Ok(l)
        }
    }

    fn text_size(&self, text: &str, fmt: &IDWriteTextFormat) -> Result<(f32, f32)> {
        let l = self.layout(text, fmt, 1000.0, 200.0)?;
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe { l.GetMetrics(&mut m)? };
        Ok((m.widthIncludingTrailingWhitespace, m.height))
    }

    fn ensure_fonts(&mut self, cfg: &Config) -> Result<()> {
        let family = if self.family_exists(&cfg.font) { cfg.font.clone() } else { "Segoe UI".to_string() };
        let key = FontKey { family, size: (cfg.font_size * 100.0) as u32, decimals: cfg.decimals, unit: cfg.unit };
        if self.fonts.as_ref().is_some_and(|f| f.key == key) {
            return Ok(());
        }
        // Bahnschrift has a semi-condensed width that keeps the widget narrow.
        let stretch = if key.family == "Bahnschrift" { DWRITE_FONT_STRETCH_SEMI_CONDENSED } else { DWRITE_FONT_STRETCH_NORMAL };
        let size = cfg.font_size;
        let value = self.format(&key.family, DWRITE_FONT_WEIGHT_SEMI_BOLD, stretch, size, true)?;
        let unit = self.format(&key.family, DWRITE_FONT_WEIGHT_NORMAL, stretch, size * 0.92, false)?;
        let (mut value_w, mut line_h) = (0.0f32, 0.0f32);
        for s in format::widest_values(cfg.decimals) {
            let (w, h) = self.text_size(s, &value)?;
            value_w = value_w.max(w);
            line_h = line_h.max(h);
        }
        let mut unit_w: f32 = 0.0;
        for s in format::units(cfg.unit) {
            unit_w = unit_w.max(self.text_size(s, &unit)?.0);
        }
        self.fonts = Some(Fonts { key, value, unit, value_w: value_w.ceil() + 1.0, unit_w: unit_w.ceil(), line_h });
        Ok(())
    }

    fn geo(&self, cfg: &Config, floating: bool) -> Geo {
        let f = self.fonts.as_ref().expect("fonts");
        let fs = cfg.font_size;
        let arrow = if cfg.show_arrows { fs * 0.62 } else { 0.0 };
        let gap1 = if cfg.show_arrows { fs * 0.36 } else { 0.0 };
        let gap2 = fs * 0.26;
        let row_w = arrow + gap1 + f.value_w + gap2 + f.unit_w;
        let row_h = fs * 1.3;
        let (grow, streaks) = if cfg.animations { (MAX_GROW, fs * 1.6) } else { (1.0, 0.0) };
        let group_w = row_w * grow + streaks;
        let group_gap = fs * 0.9;
        let (content_w, content_h) = match cfg.layout {
            Layout::Stacked => (group_w, row_h * (1.0 + grow)),
            Layout::Inline => (group_w * 2.0 + group_gap, row_h * grow),
        };
        Geo {
            pad_x: if floating { fs * 0.9 } else { fs * 0.55 },
            pad_y: fs * 0.3,
            arrow,
            gap1,
            gap2,
            row_w,
            row_h,
            group_w,
            group_gap,
            content_w,
            content_h,
        }
    }

    /// Pixel size the widget needs. Constant for a given config, so the widget never
    /// jitters while the numbers animate.
    pub fn measure(&mut self, cfg: &Config, floating: bool, scale: f32, height: Option<i32>) -> Result<(i32, i32)> {
        self.ensure_fonts(cfg)?;
        let g = self.geo(cfg, floating);
        let w = ((g.content_w + g.pad_x * 2.0) * scale).ceil() as i32;
        let h = match height {
            Some(h) => h,
            None => ((g.content_h + g.pad_y * 2.0) * scale).ceil() as i32,
        };
        Ok((w.max(8), h.max(8)))
    }

    fn ensure_surface(&mut self, w: i32, h: i32) -> Result<HDC> {
        if let Some(s) = &self.surface {
            if s.w == w && s.h == h {
                return Ok(s.dc);
            }
        }
        self.surface = None;
        unsafe {
            let dc = CreateCompatibleDC(None);
            let bmi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: w,
                    biHeight: -h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let mut bits = std::ptr::null_mut();
            let bmp = CreateDIBSection(dc, &bmi, DIB_RGB_COLORS, &mut bits, None, 0)?;
            let old = SelectObject(dc, bmp);
            self.surface = Some(Surface { dc, bmp, old, w, h });
            Ok(dc)
        }
    }

    fn ensure_target(&mut self) -> Result<ID2D1DCRenderTarget> {
        if let Some(rt) = &self.rt {
            return Ok(rt.clone());
        }
        unsafe {
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
                dpiX: 96.0,
                dpiY: 96.0,
                usage: D2D1_RENDER_TARGET_USAGE_NONE,
                minLevel: D2D1_FEATURE_LEVEL_DEFAULT,
            };
            let rt = self.d2d.CreateDCRenderTarget(&props)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            if self.stroke.is_none() {
                let sp = D2D1_STROKE_STYLE_PROPERTIES {
                    startCap: D2D1_CAP_STYLE_ROUND,
                    endCap: D2D1_CAP_STYLE_ROUND,
                    dashCap: D2D1_CAP_STYLE_ROUND,
                    lineJoin: D2D1_LINE_JOIN_ROUND,
                    miterLimit: 10.0,
                    dashStyle: D2D1_DASH_STYLE_SOLID,
                    dashOffset: 0.0,
                };
                self.stroke = Some(self.d2d.CreateStrokeStyle(&sp, None)?);
            }
            self.rt = Some(rt.clone());
            Ok(rt)
        }
    }

    /// Renders the scene and returns the memory DC holding the pixels.
    pub fn draw(&mut self, scene: &Scene, w: i32, h: i32) -> Result<HDC> {
        self.ensure_fonts(scene.cfg)?;
        let dc = self.ensure_surface(w, h)?;
        let rt = self.ensure_target()?;
        let result = unsafe {
            rt.BindDC(dc, &RECT { left: 0, top: 0, right: w, bottom: h })?;
            rt.SetDpi(96.0 * scene.scale, 96.0 * scene.scale);
            rt.BeginDraw();
            self.paint(&rt, scene, w as f32 / scene.scale, h as f32 / scene.scale);
            rt.EndDraw(None, None)
        };
        if let Err(e) = result {
            // Device loss or similar: rebuild the target next time.
            self.rt = None;
            return Err(e);
        }
        Ok(dc)
    }

    fn paint(&self, rt: &ID2D1DCRenderTarget, s: &Scene, w: f32, h: f32) {
        let cfg = s.cfg;
        let g = self.geo(cfg, s.floating);
        let ink: [u8; 3] = if s.dark { [255, 255, 255] } else { [0, 0, 0] };
        unsafe {
            // Alpha 1/255 everywhere keeps the whole widget clickable while staying invisible.
            rt.Clear(Some(&D2D1_COLOR_F { r: 0.0, g: 0.0, b: 0.0, a: 1.0 / 255.0 }));
            let Ok(brush) = rt.CreateSolidColorBrush(&rgba(ink, 1.0), None) else { return };

            if s.floating {
                let bg = if s.dark { rgba([32, 32, 32], 0.94) } else { rgba([243, 243, 243], 0.96) };
                brush.SetColor(&bg);
                let r = rrect(0.5, 0.5, w - 0.5, h - 0.5, 8.0);
                rt.FillRoundedRectangle(&r, &brush);
                brush.SetColor(&rgba(ink, 0.10 + 0.12 * s.hover));
                rt.DrawRoundedRectangle(&r, &brush, 1.0, None);
            } else if s.hover > 0.0 {
                brush.SetColor(&rgba(ink, (if s.dark { 0.08 } else { 0.06 }) * s.hover));
                rt.FillRoundedRectangle(&rrect(1.0, 4.0, w - 1.0, h - 4.0, 5.0), &brush);
            }

            let left = (w - g.content_w) / 2.0;
            let rows = if cfg.upload_first { [(false, s.up), (true, s.down)] } else { [(true, s.down), (false, s.up)] };
            // Never let grown rows spill outside the widget.
            let room = h - 2.0;
            match cfg.layout {
                Layout::Stacked => {
                    let (mut s0, mut s1) = (rows[0].1.scale.max(0.5), rows[1].1.scale.max(0.5));
                    let total = g.row_h * (s0 + s1);
                    if total > room {
                        let f = room / total;
                        s0 *= f;
                        s1 *= f;
                    }
                    let top = (h - g.row_h * (s0 + s1)) / 2.0;
                    let right = left + g.group_w;
                    self.paint_row(rt, &brush, s, &g, left, right, top + g.row_h * s0 / 2.0, s0, rows[0], ink);
                    self.paint_row(rt, &brush, s, &g, left, right, top + g.row_h * s0 + g.row_h * s1 / 2.0, s1, rows[1], ink);
                }
                Layout::Inline => {
                    for (i, row) in rows.iter().enumerate() {
                        let sc = row.1.scale.max(0.5).min(room / g.row_h);
                        let l = left + i as f32 * (g.group_w + g.group_gap);
                        self.paint_row(rt, &brush, s, &g, l, l + g.group_w, h / 2.0, sc, *row, ink);
                    }
                }
            }
            rt.SetTransform(&IDENTITY);
        }
    }

    /// One row, right-aligned at `right`, grown by `sc` around its right edge so it
    /// expands into the free space on the left.
    #[allow(clippy::too_many_arguments)]
    fn paint_row(
        &self,
        rt: &ID2D1DCRenderTarget,
        brush: &ID2D1SolidColorBrush,
        s: &Scene,
        g: &Geo,
        group_left: f32,
        right: f32,
        cy: f32,
        sc: f32,
        (is_down, row): (bool, RowState),
        ink: [u8; 3],
    ) {
        let cfg = s.cfg;
        let fonts = self.fonts.as_ref().expect("fonts");
        let speed = row.speed;
        let soft = if s.dark { 0.62 } else { 0.60 };
        // `hot` is 0 for a calm row and 1 for a colored one; animated rows fade between.
        let (color, hot) = if cfg.animations {
            (row.rgb(), row.heat)
        } else {
            match heat::text_color(speed, &cfg.heat, s.dark) {
                Some(c) => (c, 1.0),
                None => (ink, 0.0),
            }
        };
        let alpha = 0.96 + 0.04 * hot;
        unsafe {
            rt.SetTransform(&scale_about(sc, right, cy));
            let x = right - g.row_w;

            if cfg.animations {
                let intensity = heat::intensity(speed, &cfg.heat);
                // Free space left of the grown row, in this row's (scaled) units.
                let band = ((right - g.row_w * sc) - group_left) / sc - 2.0;
                if intensity > 0.02 && band > 4.0 {
                    self.paint_streaks(rt, brush, x - 2.0, band, cy, g.row_h, row.phase, intensity, color, is_down);
                }
            }

            if cfg.show_arrows {
                let a = g.arrow;
                let cx = x + a / 2.0;
                let (tip, tail) = if is_down { (cy + a / 2.0, cy - a / 2.0) } else { (cy - a / 2.0, cy + a / 2.0) };
                let head = a * 0.38 * if is_down { -1.0 } else { 1.0 };
                brush.SetColor(&rgba(color, soft + (1.0 - soft) * hot));
                let sw = (cfg.font_size * 0.12).max(1.2);
                let st = self.stroke.as_ref();
                rt.DrawLine(pt(cx, tail), pt(cx, tip), brush, sw, st);
                rt.DrawLine(pt(cx - a * 0.38, tip + head), pt(cx, tip), brush, sw, st);
                rt.DrawLine(pt(cx + a * 0.38, tip + head), pt(cx, tip), brush, sw, st);
            }

            let text = format::speed(speed, cfg.unit, cfg.decimals);
            let top = cy - fonts.line_h / 2.0;
            let vx = x + g.arrow + g.gap1;
            if let Ok(l) = self.layout(&text.value, &fonts.value, fonts.value_w, fonts.line_h) {
                brush.SetColor(&rgba(color, alpha));
                rt.DrawTextLayout(pt(vx, top), &l, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
            if let Ok(l) = self.layout(text.unit, &fonts.unit, fonts.unit_w + 4.0, fonts.line_h) {
                brush.SetColor(&rgba(ink, soft));
                rt.DrawTextLayout(pt(vx + fonts.value_w + g.gap2, top), &l, brush, D2D1_DRAW_TEXT_OPTIONS_NONE);
            }
        }
    }

    /// Fine motion streaks that shoot left out of a busy row, fading as they go.
    /// More traffic means more, longer and faster streaks.
    #[allow(clippy::too_many_arguments)]
    fn paint_streaks(
        &self,
        rt: &ID2D1DCRenderTarget,
        brush: &ID2D1SolidColorBrush,
        x_end: f32,
        band: f32,
        cy: f32,
        row_h: f32,
        phase: f32,
        intensity: f32,
        color: [u8; 3],
        is_down: bool,
    ) {
        let count = 3 + (intensity * 2.0).round() as u32;
        let seed = if is_down { 11 } else { 47 };
        let st = self.stroke.as_ref();
        for k in 0..count {
            let r1 = hash01(seed + k * 7);
            let r2 = hash01(seed + k * 13 + 3);
            let lane = (r1 - 0.5) * row_h * 0.62;
            let len = band * (0.4 + 0.45 * intensity) * (0.6 + 0.4 * r2);
            let t = (phase * (0.85 + 0.5 * r2) + r1).fract();
            // Each streak is born at the arrow and races left, fading out.
            let head = x_end - t * (band + len);
            let tail = (head + len).min(x_end);
            let head = head.max(x_end - band);
            if tail - head < 0.5 {
                continue;
            }
            let fade = (1.0 - t).powf(1.2) * (0.45 + 0.5 * intensity);
            // Draw as a few segments so the streak tapers from bright to clear.
            const SEGMENTS: usize = 4;
            for sgm in 0..SEGMENTS {
                let a = head + (tail - head) * sgm as f32 / SEGMENTS as f32;
                let b = head + (tail - head) * (sgm + 1) as f32 / SEGMENTS as f32;
                let alpha = fade * (sgm + 1) as f32 / SEGMENTS as f32;
                unsafe {
                    brush.SetColor(&rgba(color, alpha));
                    rt.DrawLine(pt(a, cy + lane), pt(b, cy + lane), brush, 1.0, st);
                }
            }
        }
    }
}
