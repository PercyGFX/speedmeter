//! Small custom controls drawn to match Windows 11.

use super::theme::{self, Palette};
use eframe::egui::{
    self, pos2, vec2, Align, Align2, Color32, CornerRadius, FontId, Id, LayerId, Layout, Order,
    Pos2, Rect, Response, RichText, Sense, Shape, Stroke, StrokeKind, Ui,
};

/// A stroked arrow (stem + chevron), pointing down or up.
pub fn arrow(painter: &egui::Painter, center: Pos2, size: f32, down: bool, color: Color32, width: f32) {
    let (tip, tail) = if down {
        (center.y + size / 2.0, center.y - size / 2.0)
    } else {
        (center.y - size / 2.0, center.y + size / 2.0)
    };
    let head = size * 0.4 * if down { -1.0 } else { 1.0 };
    let stroke = Stroke::new(width, color);
    painter.line_segment([pos2(center.x, tail), pos2(center.x, tip)], stroke);
    painter.add(Shape::line(
        vec![pos2(center.x - size * 0.4, tip + head), pos2(center.x, tip), pos2(center.x + size * 0.4, tip + head)],
        stroke,
    ));
}

fn text_width(ui: &Ui, text: &str, font: &FontId) -> f32 {
    ui.fonts(|f| f.layout_no_wrap(text.to_string(), font.clone(), Color32::WHITE).size().x)
}

/// Segmented selector: a recessed track with the chosen option on a raised tile
/// that slides to a new choice. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, p: &Palette, id: &str, value: &mut T, options: &[(T, &str)]) -> bool {
    let font = theme::body(13.0);
    let chosen_font = theme::strong(13.0);
    let h = 30.0;
    let pad = 3.0;
    // Size each option for its semibold label, so text doesn't shift when chosen.
    let widths: Vec<f32> = options.iter().map(|(_, l)| text_width(ui, l, &chosen_font) + 26.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, h), Sense::hover());
    let painter = ui.painter().clone();
    let (track, tile, edge) = if p.dark {
        (theme::blend(p.content, Color32::BLACK, 0.18), theme::blend(p.selected, p.text, 0.06), Color32::from_rgba_unmultiplied(255, 255, 255, 30))
    } else {
        (theme::blend(p.selected, p.content, 0.35), p.layer, Color32::from_black_alpha(22))
    };
    painter.rect_filled(rect, CornerRadius::same(8), track);
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0_f32, p.line), StrokeKind::Inside);

    let slots: Vec<Rect> = widths
        .iter()
        .scan(rect.left() + pad, |x, w| {
            let r = Rect::from_min_size(pos2(*x, rect.top() + pad), vec2(*w, h - pad * 2.0));
            *x += w;
            Some(r)
        })
        .collect();
    let mut changed = false;
    let mut chosen = options.iter().position(|(v, _)| v == value);
    for (i, (val, _)) in options.iter().enumerate() {
        let resp = ui.interact(slots[i], ui.id().with((id, i)), Sense::click());
        if Some(i) != chosen && resp.hovered() {
            painter.rect_filled(slots[i].shrink(1.0), CornerRadius::same(5), theme::blend(track, p.text, 0.05));
        }
        if resp.clicked() && Some(i) != chosen {
            *value = *val;
            chosen = Some(i);
            changed = true;
        }
    }

    // The raised tile glides to the chosen option.
    if let Some(c) = chosen {
        let ctx = ui.ctx();
        let anim = |part: &str, v: f32| ctx.animate_value_with_time(ui.id().with((id, "tile", part)), v, 0.14);
        let target = slots[c];
        let tile_rect = Rect::from_min_size(pos2(anim("x", target.left()), target.top()), vec2(anim("w", target.width()), target.height()));
        let shadow = egui::Shadow { offset: [0, 1], blur: 3, spread: 0, color: Color32::from_black_alpha(if p.dark { 70 } else { 26 }) };
        painter.add(shadow.as_shape(tile_rect, CornerRadius::same(6)));
        painter.rect_filled(tile_rect, CornerRadius::same(6), tile);
        painter.rect_stroke(tile_rect, CornerRadius::same(6), Stroke::new(1.0_f32, edge), StrokeKind::Inside);
    }
    for (i, (_, label)) in options.iter().enumerate() {
        let (f, color) = if Some(i) == chosen { (chosen_font.clone(), p.text) } else { (font.clone(), p.dim) };
        painter.text(slots[i].center(), Align2::CENTER_CENTER, *label, f, color);
    }
    changed
}

/// Windows 11 toggle switch. The knob slides when clicked.
pub fn toggle(ui: &mut Ui, p: &Palette, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let painter = ui.painter();
    let radius = CornerRadius::same(10);
    let hovered = resp.hovered();
    if *on {
        let fill = if hovered { theme::blend(p.accent, p.text, 0.12) } else { p.accent };
        painter.rect_filled(rect, radius, fill);
    } else {
        if hovered {
            painter.rect_filled(rect, radius, p.hover);
        }
        painter.rect_stroke(rect, radius, Stroke::new(1.0_f32, p.dim), StrokeKind::Inside);
    }
    let knob = if hovered { 6.0 } else { 5.0 };
    let x = egui::lerp((rect.left() + 10.0)..=(rect.right() - 10.0), t);
    painter.circle_filled(pos2(x, rect.center().y), knob, if *on { p.on_accent } else { p.dim });
    resp
}

/// Radio indicator used in the connection list.
pub fn radio_mark(painter: &egui::Painter, p: &Palette, center: Pos2, selected: bool, hovered: bool) {
    if selected {
        painter.circle_filled(center, 10.0, p.accent);
        painter.circle_filled(center, if hovered { 5.0 } else { 4.0 }, p.on_accent);
    } else {
        painter.circle_stroke(center, 9.5, Stroke::new(1.0_f32, p.dim));
        if hovered {
            painter.circle_filled(center, 4.0, p.dim);
        }
    }
}

/// Navigation rail entry: an accent bar marks the current page.
pub fn nav_item(ui: &mut Ui, p: &Palette, glyph: Option<char>, label: &str, selected: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(6), p.hover);
        let bar = Rect::from_center_size(pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, 16.0));
        painter.rect_filled(bar, CornerRadius::same(2), p.accent);
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), theme::blend(p.base, p.hover, 0.6));
    }
    let color = if selected { p.text } else { p.dim };
    let mut x = rect.left() + 14.0;
    if let Some(g) = glyph {
        painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, g.to_string(), theme::icon(16.0), color);
        x += 30.0;
    }
    painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, label, theme::body(14.0), color);
    resp.clicked()
}

/// Flat button. `primary` uses the accent fill.
pub fn button(ui: &mut Ui, p: &Palette, label: &str, primary: bool) -> Response {
    let font = theme::body(14.0);
    let w = text_width(ui, label, &font) + 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
    let hovered = resp.hovered();
    let painter = ui.painter();
    let (fill, fg) = if primary {
        (if hovered { theme::blend(p.accent, p.text, 0.12) } else { p.accent }, p.on_accent)
    } else {
        (if hovered { p.hover } else { p.selected }, p.text)
    };
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    if !primary {
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0_f32, p.line), StrokeKind::Inside);
    }
    painter.text(rect.center(), Align2::CENTER_CENTER, label, font, fg);
    resp
}

/// Small icon-only button (for the ‹ › period arrows).
pub fn icon_button(ui: &mut Ui, p: &Palette, glyph: &str, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    if enabled && resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(6), p.hover);
    }
    let color = if enabled { p.text } else { p.faint };
    painter.text(rect.center(), Align2::CENTER_CENTER, glyph, theme::body(18.0), color);
    enabled && resp.clicked()
}

/// A Windows Settings style row: title and description on the left, the control on
/// the right.
pub fn setting_row(ui: &mut Ui, p: &Palette, title: &str, desc: Option<&str>, control: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(p.layer)
        .stroke(Stroke::new(1.0_f32, p.line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                let text_w = (ui.available_width() - 360.0).max(220.0);
                // Measure the text first so the block can be centered against the control.
                let title_h = ui.fonts(|f| f.row_height(&theme::body(14.0)));
                let desc_h = desc.map_or(0.0, |d| {
                    2.0 + ui.fonts(|f| f.layout(d.to_string(), theme::body(12.5), p.dim, text_w).size().y)
                });
                ui.allocate_ui_with_layout(vec2(text_w, title_h + desc_h), Layout::top_down(Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.label(RichText::new(title).font(theme::body(14.0)).color(p.text));
                    if let Some(d) = desc {
                        ui.add(egui::Label::new(RichText::new(d).font(theme::body(12.5)).color(p.dim)).wrap());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), control);
            });
        });
    ui.add_space(3.0);
}

pub fn section(ui: &mut Ui, p: &Palette, title: &str) {
    ui.add_space(22.0);
    ui.label(RichText::new(title).font(theme::strong(15.0)).color(p.text));
    ui.add_space(4.0);
}

/// Floating info box near the pointer (chart hovers). Each line has its own color.
pub fn tooltip(ctx: &egui::Context, p: &Palette, anchor: Pos2, lines: &[(String, Color32, FontId)]) {
    let painter = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("speedmeter-tip")));
    let galleys: Vec<_> = lines.iter().map(|(t, c, f)| painter.layout_no_wrap(t.clone(), f.clone(), *c)).collect();
    let w = galleys.iter().map(|g| g.size().x).fold(0.0, f32::max) + 24.0;
    let h = galleys.iter().map(|g| g.size().y).sum::<f32>() + 4.0 * (galleys.len().saturating_sub(1)) as f32 + 20.0;
    let screen = ctx.screen_rect();
    let mut pos = anchor + vec2(16.0, -h - 12.0);
    if pos.x + w > screen.right() - 8.0 {
        pos.x = anchor.x - w - 16.0;
    }
    if pos.y < screen.top() + 8.0 {
        pos.y = anchor.y + 20.0;
    }
    let rect = Rect::from_min_size(pos, vec2(w, h));
    let shadow = egui::Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(if p.dark { 110 } else { 40 }) };
    painter.add(shadow.as_shape(rect, CornerRadius::same(8)));
    painter.rect_filled(rect, CornerRadius::same(8), p.tip);
    painter.rect_stroke(rect, CornerRadius::same(8), Stroke::new(1.0_f32, p.line), StrokeKind::Inside);
    let mut y = rect.top() + 10.0;
    for g in galleys {
        let gh = g.size().y;
        painter.galley(pos2(rect.left() + 12.0, y), g, p.text);
        y += gh + 4.0;
    }
}
