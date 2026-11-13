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

/// Segmented selector: a soft pill-shaped track, with the chosen option on a solid
/// accent pill that slides to a new choice. Returns true when the value changed.
pub fn segmented<T: PartialEq + Copy>(ui: &mut Ui, p: &Palette, id: &str, value: &mut T, options: &[(T, &str)]) -> bool {
    let font = theme::body(13.0);
    let chosen_font = theme::strong(13.0);
    let h = 32.0;
    let pad = 3.0;
    // Size each option for its semibold label, so text doesn't shift when chosen.
    let widths: Vec<f32> = options.iter().map(|(_, l)| text_width(ui, l, &chosen_font) + 28.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + pad * 2.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, h), Sense::hover());
    let painter = ui.painter().clone();
    let round = |r: Rect| CornerRadius::same((r.height() / 2.0) as u8);
    // The track is a tint of whatever it sits on (rows are lighter than the page).
    let track = p.tint(p.content, if p.dark { 0.085 } else { 0.07 });
    painter.rect_filled(rect, round(rect), track);

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
    let mut hovered = None;
    for (i, (val, _)) in options.iter().enumerate() {
        let resp = ui.interact(slots[i], ui.id().with((id, i)), Sense::click());
        if Some(i) != chosen && resp.hovered() {
            hovered = Some(i);
            painter.rect_filled(slots[i], round(slots[i]), p.tint(track, 0.07));
        }
        if resp.clicked() && Some(i) != chosen {
            *value = *val;
            chosen = Some(i);
            changed = true;
        }
    }

    // The accent pill glides to the chosen option.
    if let Some(c) = chosen {
        let ctx = ui.ctx();
        let anim = |part: &str, v: f32| ctx.animate_value_with_time(ui.id().with((id, "pill", part)), v, 0.16);
        let target = slots[c];
        let pill = Rect::from_min_size(pos2(anim("x", target.left()), target.top()), vec2(anim("w", target.width()), target.height()));
        let shadow = egui::Shadow { offset: [0, 1], blur: 4, spread: 0, color: Color32::from_black_alpha(if p.dark { 80 } else { 40 }) };
        painter.add(shadow.as_shape(pill, round(pill)));
        painter.rect_filled(pill, round(pill), p.fill());
    }
    for (i, (_, label)) in options.iter().enumerate() {
        let (f, color) = if Some(i) == chosen {
            (chosen_font.clone(), Color32::WHITE)
        } else {
            (font.clone(), if hovered == Some(i) { p.text } else { p.dim })
        };
        painter.text(slots[i].center() - vec2(0.0, 0.5), Align2::CENTER_CENTER, *label, f, color);
    }
    changed
}

/// Switch: a pill that fills with the accent as a white knob slides across.
pub fn toggle(ui: &mut Ui, p: &Palette, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(42.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_responsive(resp.id, *on);
    let painter = ui.painter();
    let hovered = resp.hovered();
    let off = p.tint(p.layer, if hovered { 0.26 } else { 0.19 });
    let lit = if hovered { theme::blend(p.fill(), Color32::WHITE, 0.10) } else { p.fill() };
    painter.rect_filled(rect, CornerRadius::same(12), theme::blend(off, lit, t));
    let center = pos2(egui::lerp((rect.left() + 12.0)..=(rect.right() - 12.0), t), rect.center().y);
    // A soft shadow lifts the knob off the track.
    painter.circle_filled(center + vec2(0.0, 1.0), 9.5, Color32::from_black_alpha(46));
    let resting = if p.dark { Color32::from_rgb(0xD6, 0xD6, 0xD6) } else { Color32::WHITE };
    painter.circle_filled(center, 9.0, theme::blend(resting, Color32::WHITE, t));
    resp
}

/// Radio indicator used in the connection list.
pub fn radio_mark(painter: &egui::Painter, p: &Palette, center: Pos2, selected: bool, hovered: bool) {
    if selected {
        painter.circle_filled(center, 10.0, p.fill());
        painter.circle_filled(center, if hovered { 4.5 } else { 4.0 }, Color32::WHITE);
    } else {
        painter.circle_stroke(center, 9.5, Stroke::new(1.5_f32, p.tint(p.layer, if hovered { 0.5 } else { 0.32 })));
    }
}

/// The sidebar's update card: what's available, or how far along it is
/// (`progress` 0 to 1). Returns true when clicked.
pub fn update_card(ui: &mut Ui, p: &Palette, glyph: Option<char>, title: &str, detail: &str, progress: Option<f32>, clickable: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 56.0), if clickable { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    let hot = clickable && resp.hovered();
    let radius = CornerRadius::same(8);
    painter.rect_filled(rect, radius, theme::blend(p.base, p.fill(), if hot { 0.26 } else { 0.16 }));
    painter.rect_stroke(rect, radius, Stroke::new(1.0_f32, theme::blend(p.base, p.fill(), if hot { 0.75 } else { 0.5 })), StrokeKind::Inside);
    let mut x = rect.left() + 14.0;
    if let Some(g) = glyph {
        painter.text(pos2(x, rect.center().y), Align2::LEFT_CENTER, g.to_string(), theme::icon(18.0), p.accent);
        x += 30.0;
    }
    let room = rect.right() - 12.0 - x;
    let title = super::charts::fit(painter, title, theme::strong(13.5), p.text, room);
    painter.galley(pos2(x, rect.top() + 10.0), title, p.text);
    let detail = super::charts::fit(painter, detail, theme::body(12.0), p.dim, room);
    painter.galley(pos2(x, rect.top() + 29.0), detail, p.dim);
    if let Some(f) = progress {
        let track = Rect::from_min_max(pos2(rect.left() + 10.0, rect.bottom() - 6.0), pos2(rect.right() - 10.0, rect.bottom() - 3.0));
        painter.rect_filled(track, CornerRadius::same(2), p.tint(p.base, 0.14));
        let done = Rect::from_min_size(track.min, vec2(track.width() * f.clamp(0.0, 1.0), track.height()));
        painter.rect_filled(done, CornerRadius::same(2), p.fill());
    }
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    clickable && resp.clicked()
}

/// Navigation rail entry: an accent bar marks the current page.
pub fn nav_item(ui: &mut Ui, p: &Palette, glyph: Option<char>, label: &str, selected: bool) -> bool {
    theme::ensure_cjk(ui.ctx(), &[label]);
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 38.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, CornerRadius::same(7), p.tint(p.base, 0.085));
        let bar = Rect::from_center_size(pos2(rect.left() + 1.5, rect.center().y), vec2(3.0, 16.0));
        painter.rect_filled(bar, CornerRadius::same(2), p.fill());
    } else if resp.hovered() {
        painter.rect_filled(rect, CornerRadius::same(7), p.tint(p.base, 0.045));
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
    let pressed = resp.is_pointer_button_down_on();
    let (fill, fg) = if primary {
        let c = p.fill();
        (if pressed { theme::blend(c, Color32::BLACK, 0.12) } else if hovered { theme::blend(c, Color32::WHITE, 0.10) } else { c }, Color32::WHITE)
    } else {
        (if pressed { p.tint(p.layer, 0.05) } else if hovered { p.tint(p.layer, 0.14) } else { p.tint(p.layer, 0.09) }, p.text)
    };
    painter.rect_filled(rect, CornerRadius::same(6), fill);
    if !primary {
        painter.rect_stroke(rect, CornerRadius::same(6), Stroke::new(1.0_f32, p.tint(p.layer, 0.16)), StrokeKind::Inside);
    }
    painter.text(rect.center(), Align2::CENTER_CENTER, label, if primary { theme::strong(13.5) } else { font }, fg);
    resp
}

/// Round back or forward button with a drawn chevron (for stepping through periods).
pub fn chevron_button(ui: &mut Ui, p: &Palette, back: bool, enabled: bool) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 32.0), if enabled { Sense::click() } else { Sense::hover() });
    let painter = ui.painter();
    let hot = enabled && resp.hovered();
    let pressed = enabled && resp.is_pointer_button_down_on();
    let fill = if pressed { 0.07 } else if hot { 0.16 } else if enabled { 0.09 } else { 0.04 };
    painter.circle_filled(rect.center(), 15.0, p.tint(p.content, fill));
    let color = if enabled { p.text } else { theme::blend(p.faint, p.content, 0.45) };
    // The chevron's point is nudged so the shape looks centered.
    let dir = if back { -1.0 } else { 1.0 };
    let c = rect.center() + vec2(dir * 1.2, 0.0);
    let (reach, rise) = (3.6 * dir, 5.2);
    painter.add(Shape::line(vec![pos2(c.x - reach, c.y - rise), pos2(c.x + reach * 0.45, c.y), pos2(c.x - reach, c.y + rise)], Stroke::new(1.8_f32, color)));
    enabled && resp.clicked()
}

/// A Windows Settings style row: title and description on the left, the control on
/// the right.
pub fn setting_row(ui: &mut Ui, p: &Palette, title: &str, desc: Option<&str>, control: impl FnOnce(&mut Ui)) {
    // The text gets whatever the control doesn't need. The control's width is only
    // known once it's drawn, so it's remembered from the previous frame.
    let width_id = ui.id().with(("setting-row", title));
    let control_w: Option<f32> = ui.data(|d| d.get_temp(width_id));
    egui::Frame::NONE
        .fill(p.layer)
        .stroke(Stroke::new(1.0_f32, p.line))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.set_min_height(32.0);
                let text_w = (ui.available_width() - control_w.map_or(360.0, |w| w + 28.0)).max(200.0);
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
                let used = ui.with_layout(Layout::right_to_left(Align::Center), control).response.rect.width();
                if control_w.is_none_or(|w| (w - used).abs() > 0.5) {
                    ui.data_mut(|d| d.insert_temp(width_id, used));
                    ui.ctx().request_repaint();
                }
            });
        });
    ui.add_space(4.0);
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
