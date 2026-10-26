//! Charts. Everything shares one idea: a horizontal "wire", with download drawn
//! above it and upload below it on the same scale, colored by the heat tiers.

use super::theme::{self, Palette};
use super::ui;
use crate::config::Heat;
use crate::format::{self, UnitMode};
use crate::heat;
use crate::history::Row;
use crate::shared::Sample;
use chrono::{Datelike, Local, NaiveDate, TimeZone};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Stroke, StrokeKind, Ui,
};
use std::collections::HashMap;

/// The next "round" value at or above `v`: 1, 2, 5, 10, 20, 50... in powers of 1024.
pub fn nice_ceil(v: f64) -> f64 {
    let v = v.max(1.0);
    let mut unit = 1.0;
    while v > unit * 1000.0 {
        unit *= 1024.0;
    }
    for step in [1.0, 2.0, 5.0, 10.0, 20.0, 50.0, 100.0, 200.0, 500.0, 1000.0] {
        if v <= step * unit {
            return step * unit;
        }
    }
    1024.0 * unit
}

fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// Small scale label with a backing so it stays readable over bars.
fn scale_label(painter: &egui::Painter, p: &Palette, pos: Pos2, anchor: Align2, text: String) {
    let g = painter.layout_no_wrap(text, theme::display(12.0), p.faint);
    let r = anchor.anchor_size(pos, g.size()).expand2(vec2(4.0, 1.0));
    painter.rect_filled(r, CornerRadius::same(3), p.content.gamma_multiply(0.85));
    painter.galley(r.min + vec2(4.0, 1.0), g, p.faint);
}

fn plot_split(plot: Rect) -> (f32, f32, f32) {
    let wire = (plot.top() + plot.height() * 0.64).round() + 0.5;
    (wire, wire - plot.top() - 3.0, plot.bottom() - wire - 3.0)
}

pub struct Wave<'a> {
    pub samples: &'a [Sample],
    pub window_s: f64,
    /// Time at the right edge, unix ms. Advances every frame when animating, so
    /// the bars glide left instead of jumping once a second.
    pub now_ms: f64,
    pub heat: &'a Heat,
    pub unit: UnitMode,
    pub animate: bool,
}

fn ago_label(window_s: f64) -> &'static str {
    match window_s as u32 {
        0..=60 => "1 minute ago",
        61..=300 => "5 minutes ago",
        301..=900 => "15 minutes ago",
        _ => "1 hour ago",
    }
}

/// The live chart: one column per second (or per few seconds on long ranges, keeping
/// the peak so spikes never disappear).
pub fn waveform(ui: &mut Ui, p: &Palette, w: &Wave, height: f32) {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let painter = ui.painter_at(rect);
    let plot = Rect::from_min_max(pos2(rect.left(), rect.top() + 2.0), pos2(rect.right(), rect.bottom() - 24.0));
    let (wire_y, down_h, up_h) = plot_split(plot);

    let window_ms = w.window_s * 1000.0;
    let px_per_ms = plot.width() as f64 / window_ms;
    let bucket_ms = ((3.0 / px_per_ms) / 1000.0).ceil().max(1.0) * 1000.0;
    let start = w.now_ms - window_ms - bucket_ms - 1000.0;

    // (start ms, end ms, peak down, peak up)
    let mut cols: Vec<(f64, f64, f32, f32)> = Vec::new();
    for s in w.samples.iter().filter(|s| s.t as f64 > start) {
        let t = s.t as f64;
        if bucket_ms <= 1000.0 {
            cols.push((t - 1000.0, t, s.down, s.up));
            continue;
        }
        let k = ((t - 1.0) / bucket_ms).floor() * bucket_ms;
        match cols.last_mut() {
            Some(c) if c.0 == k => {
                c.2 = c.2.max(s.down);
                c.3 = c.3.max(s.up);
            }
            _ => cols.push((k, k + bucket_ms, s.down, s.up)),
        }
    }

    let visible = |c: &&(f64, f64, f32, f32)| c.1 > w.now_ms - window_ms;
    let max_down = cols.iter().filter(visible).map(|c| c.2).fold(0.0f32, f32::max) as f64;
    let max_up = cols.iter().filter(visible).map(|c| c.3).fold(0.0f32, f32::max) as f64;
    let need = max_down.max(max_up * (down_h / up_h) as f64).max(64.0 * 1024.0);
    let target = nice_ceil(need);
    let top = if w.animate {
        ui.ctx().animate_value_with_time(Id::new("wave-scale"), target as f32, 0.45) as f64
    } else {
        target
    };
    let px_per_byte = down_h as f64 / top;
    let bottom_val = up_h as f64 / px_per_byte;

    painter.line_segment([plot.left_top(), plot.right_top()], Stroke::new(1.0_f32, p.line));
    painter.line_segment([plot.left_bottom(), plot.right_bottom()], Stroke::new(1.0_f32, p.line));

    let x_of = |t: f64| plot.right() - ((w.now_ms - t) * px_per_ms) as f32;
    let pointer = resp.hover_pos();
    let mut hovered = None;
    for c in &cols {
        let x0 = x_of(c.0).max(plot.left());
        let x1 = x_of(c.1).min(plot.right());
        if x1 - x0 < 0.5 {
            continue;
        }
        let colw = x1 - x0;
        let gap = if colw >= 8.0 { 2.0 } else if colw >= 3.0 { 1.0 } else { 0.0 };
        let (bx0, bx1) = (x0 + gap / 2.0, x1 - gap / 2.0);
        let mut hd = (c.2 as f64 * px_per_byte).min(down_h as f64 + 2.0) as f32;
        let mut hu = (c.3 as f64 * px_per_byte).min(up_h as f64 + 2.0) as f32;
        if c.2 > 0.0 {
            hd = hd.max(1.0);
        }
        if c.3 > 0.0 {
            hu = hu.max(1.0);
        }
        let radius = if bx1 - bx0 >= 6.0 { 2 } else { 0 };
        let dc = theme::rgb(heat::speed_color(c.2 as f64, w.heat, p.dark));
        let uc = p.quiet(theme::rgb(heat::speed_color(c.3 as f64, w.heat, p.dark)));
        if pointer.is_some_and(|pp| pp.x >= x0 && pp.x < x1 && pp.y >= plot.top() && pp.y <= plot.bottom()) {
            painter.rect_filled(
                Rect::from_min_max(pos2(x0, plot.top()), pos2(x1, plot.bottom())),
                CornerRadius::ZERO,
                if p.dark { Color32::from_white_alpha(14) } else { Color32::from_black_alpha(12) },
            );
            hovered = Some(*c);
        }
        if hd >= 0.5 {
            painter.rect_filled(Rect::from_min_max(pos2(bx0, wire_y - 1.5 - hd), pos2(bx1, wire_y - 1.5)), CornerRadius::same(radius), dc);
        }
        if hu >= 0.5 {
            painter.rect_filled(Rect::from_min_max(pos2(bx0, wire_y + 1.5), pos2(bx1, wire_y + 1.5 + hu)), CornerRadius::same(radius), uc);
        }
    }
    painter.line_segment([pos2(plot.left(), wire_y), pos2(plot.right(), wire_y)], Stroke::new(1.0_f32, p.faint));

    scale_label(&painter, p, pos2(plot.left() + 2.0, plot.top() + 4.0), Align2::LEFT_TOP, format!("↓ {}", format::speed_short(top, w.unit)));
    scale_label(&painter, p, pos2(plot.left() + 2.0, plot.bottom() - 4.0), Align2::LEFT_BOTTOM, format!("↑ {}", format::speed_short(bottom_val, w.unit)));
    painter.text(pos2(plot.left(), plot.bottom() + 7.0), Align2::LEFT_TOP, ago_label(w.window_s), theme::body(12.0), p.faint);
    painter.text(pos2(plot.right(), plot.bottom() + 7.0), Align2::RIGHT_TOP, "now", theme::body(12.0), p.faint);

    if cols.is_empty() {
        painter.text(plot.center(), Align2::CENTER_CENTER, "Waiting for the first reading", theme::body(14.0), p.dim);
    }

    if let (Some(c), Some(pp)) = (hovered, pointer) {
        let at = |ms: f64| Local.timestamp_millis_opt(ms as i64).single().map(|t| t.format("%H:%M:%S").to_string()).unwrap_or_default();
        let title = if c.1 - c.0 > 1500.0 { format!("Peaks from {} to {}", at(c.0), at(c.1)) } else { at(c.1) };
        let tint = |v: f32| match heat::text_color(v as f64, w.heat, p.dark) {
            Some(rgb) => theme::rgb(rgb),
            None => p.text,
        };
        ui::tooltip(
            ui.ctx(),
            p,
            pp,
            &[
                (title, p.dim, theme::body(12.0)),
                (format!("↓ {}", format::speed(c.2 as f64, w.unit, crate::format::Decimals::Auto)), tint(c.2), theme::display(16.0)),
                (format!("↑ {}", format::speed(c.3 as f64, w.unit, crate::format::Decimals::Auto)), tint(c.3), theme::display(16.0)),
            ],
        );
    }
}

pub struct Bar {
    /// Axis label, e.g. "24" or "Oct".
    pub label: String,
    /// Tooltip heading, e.g. "Friday, October 24".
    pub title: String,
    pub rx: u64,
    pub tx: u64,
}

/// History bars: download above the wire, upload below, colored by how big each
/// total is next to the biggest one in view. Returns the index of a clicked bar.
pub fn mirrored_bars(ui: &mut Ui, p: &Palette, id: &str, bars: &[Bar], height: f32, animate: bool, axis: bool) -> Option<usize> {
    let width = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::click());
    let painter = ui.painter_at(rect.expand(1.0));
    let label_h = if axis { 22.0 } else { 0.0 };
    let plot = Rect::from_min_max(rect.min + vec2(0.0, 2.0), pos2(rect.right(), rect.bottom() - label_h));
    let (wire_y, down_h, up_h) = plot_split(plot);
    let n = bars.len().max(1);
    let slot = plot.width() / n as f32;
    let bw = (slot * 0.62).clamp(2.0, 42.0);
    let radius = (bw / 2.0).min(4.0) as u8;

    let max_rx = bars.iter().map(|b| b.rx).max().unwrap_or(0) as f64;
    let max_tx = bars.iter().map(|b| b.tx).max().unwrap_or(0) as f64;
    let max_total = bars.iter().map(|b| b.rx + b.tx).max().unwrap_or(0).max(1) as f64;
    let top = nice_ceil(max_rx.max(max_tx * (down_h / up_h) as f64).max(1024.0 * 1024.0));
    let ppb = down_h as f64 / top;
    let bottom_val = up_h as f64 / ppb;

    if axis {
        painter.line_segment([plot.left_top(), plot.right_top()], Stroke::new(1.0_f32, p.line));
        painter.line_segment([plot.left_bottom(), plot.right_bottom()], Stroke::new(1.0_f32, p.line));
    }

    let hover_i = resp
        .hover_pos()
        .filter(|pp| pp.y >= plot.top() && pp.y <= rect.bottom())
        .map(|pp| ((pp.x - plot.left()) / slot).floor() as usize)
        .filter(|i| *i < bars.len());
    if let Some(i) = hover_i {
        let x0 = plot.left() + slot * i as f32;
        painter.rect_filled(
            Rect::from_min_max(pos2(x0, plot.top()), pos2(x0 + slot, plot.bottom())),
            CornerRadius::same(4),
            if p.dark { Color32::from_white_alpha(12) } else { Color32::from_black_alpha(10) },
        );
    }

    for (i, b) in bars.iter().enumerate() {
        let cx = plot.left() + slot * (i as f32 + 0.5);
        let mut hd = (b.rx as f64 * ppb) as f32;
        let mut hu = (b.tx as f64 * ppb) as f32;
        if b.rx > 0 {
            hd = hd.max(1.5);
        }
        if b.tx > 0 {
            hu = hu.max(1.5);
        }
        if animate {
            hd = ui.ctx().animate_value_with_time(Id::new(id).with(("d", i)), hd, 0.35);
            hu = ui.ctx().animate_value_with_time(Id::new(id).with(("u", i)), hu, 0.35);
        }
        let share = (b.rx + b.tx) as f64 / max_total;
        let color = theme::rgb(heat::share_color(share, p.dark));
        if hd >= 0.5 {
            painter.rect_filled(
                Rect::from_min_max(pos2(cx - bw / 2.0, wire_y - 1.5 - hd), pos2(cx + bw / 2.0, wire_y - 1.5)),
                CornerRadius::same(radius),
                color,
            );
        }
        if hu >= 0.5 {
            painter.rect_filled(
                Rect::from_min_max(pos2(cx - bw / 2.0, wire_y + 1.5), pos2(cx + bw / 2.0, wire_y + 1.5 + hu)),
                CornerRadius::same(radius),
                p.quiet(color),
            );
        }
    }
    painter.line_segment([pos2(plot.left(), wire_y), pos2(plot.right(), wire_y)], Stroke::new(1.0_f32, p.faint));

    if axis {
        let step = ((34.0 / slot).ceil() as usize).max(1);
        for (i, b) in bars.iter().enumerate().step_by(step) {
            let cx = plot.left() + slot * (i as f32 + 0.5);
            painter.text(pos2(cx, plot.bottom() + 7.0), Align2::CENTER_TOP, &b.label, theme::body(11.5), p.faint);
        }
        scale_label(&painter, p, pos2(plot.left() + 2.0, plot.top() + 4.0), Align2::LEFT_TOP, format!("↓ {}", format::bytes(top as u64)));
        scale_label(&painter, p, pos2(plot.left() + 2.0, plot.bottom() - 4.0), Align2::LEFT_BOTTOM, format!("↑ {}", format::bytes(bottom_val as u64)));
    }

    if let (Some(i), Some(pp)) = (hover_i, resp.hover_pos()) {
        let b = &bars[i];
        let lines = if b.rx + b.tx == 0 {
            vec![(b.title.clone(), p.dim, theme::body(12.0)), ("No traffic recorded".to_string(), p.text, theme::body(14.0))]
        } else {
            vec![
                (b.title.clone(), p.dim, theme::body(12.0)),
                (format!("↓ {} downloaded", format::bytes(b.rx)), p.text, theme::display(15.0)),
                (format!("↑ {} uploaded", format::bytes(b.tx)), p.text, theme::display(15.0)),
                (format!("{} in total", format::bytes(b.rx + b.tx)), p.dim, theme::body(12.5)),
            ]
        };
        ui::tooltip(ui.ctx(), p, pp, &lines);
    }
    hover_i.filter(|_| resp.clicked())
}

/// Month calendar where each day is tinted by how much traffic it had.
/// Returns the clicked day.
pub fn calendar(ui: &mut Ui, p: &Palette, year: i32, month: u32, rows: &[Row], today: NaiveDate) -> Option<NaiveDate> {
    let gap = 4.0;
    let cell = ((ui.available_width() - gap * 6.0) / 7.0).clamp(26.0, 46.0);
    let header_h = 22.0;
    let first = NaiveDate::from_ymd_opt(year, month, 1)?;
    let offset = first.weekday().num_days_from_monday();
    let days = crate::history::days_in_month(year, month);
    let weeks = (offset + days).div_ceil(7);
    let size = vec2(cell * 7.0 + gap * 6.0, header_h + weeks as f32 * (cell + gap) - gap);
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter_at(rect.expand(2.0));

    for (i, name) in ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"].iter().enumerate() {
        let x = rect.left() + i as f32 * (cell + gap) + cell / 2.0;
        painter.text(pos2(x, rect.top() + 8.0), Align2::CENTER_CENTER, *name, theme::body(11.5), p.faint);
    }
    let by_day: HashMap<u32, &Row> = rows
        .iter()
        .filter_map(|r| r.key.get(8..10).and_then(|d| d.parse().ok()).map(|d: u32| (d, r)))
        .collect();
    let max = rows.iter().map(|r| r.total()).max().unwrap_or(0).max(1) as f64;
    let pointer = resp.hover_pos();
    let mut hovered: Option<(NaiveDate, Option<&Row>)> = None;
    let mut clicked = None;

    for d in 1..=days {
        let idx = offset + d - 1;
        let (col, row) = ((idx % 7) as f32, (idx / 7) as f32);
        let r = Rect::from_min_size(pos2(rect.left() + col * (cell + gap), rect.top() + header_h + row * (cell + gap)), vec2(cell, cell));
        let date = NaiveDate::from_ymd_opt(year, month, d)?;
        let data = by_day.get(&d).copied().filter(|r| r.total() > 0);
        let (fill, num) = match data {
            Some(rw) => {
                let c = theme::rgb(heat::share_color(rw.total() as f64 / max, p.dark));
                (c, if luminance(c) > 0.55 { Color32::from_rgb(0x1B, 0x1B, 0x1B) } else { Color32::from_rgb(0xF3, 0xF3, 0xF3) })
            }
            None => (p.layer, if date > today { p.faint.gamma_multiply(0.6) } else { p.faint }),
        };
        painter.rect_filled(r, CornerRadius::same(5), fill);
        if date == today {
            painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.5_f32, p.text), StrokeKind::Inside);
        }
        painter.text(r.left_top() + vec2(6.0, 4.0), Align2::LEFT_TOP, d.to_string(), theme::display(12.0), num);
        if pointer.is_some_and(|pp| r.contains(pp)) {
            painter.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0_f32, p.dim), StrokeKind::Outside);
            hovered = Some((date, data));
            if resp.clicked() && data.is_some() {
                clicked = Some(date);
            }
        }
    }

    if let (Some((date, data)), Some(pp)) = (hovered, pointer) {
        let title = date.format("%A, %B %-d").to_string();
        let lines = match data {
            Some(r) => vec![
                (title, p.dim, theme::body(12.0)),
                (format!("↓ {} downloaded", format::bytes(r.rx)), p.text, theme::display(15.0)),
                (format!("↑ {} uploaded", format::bytes(r.tx)), p.text, theme::display(15.0)),
            ],
            None => vec![(title, p.dim, theme::body(12.0)), ("No traffic recorded".into(), p.text, theme::body(14.0))],
        };
        ui::tooltip(ui.ctx(), p, pp, &lines);
    }
    clicked
}

pub struct TableRow {
    pub label: String,
    pub rx: u64,
    pub tx: u64,
}

/// Plain list of totals, newest first, with an inline bar. Returns a clicked row.
pub fn table(ui: &mut Ui, p: &Palette, first_col: &str, rows: &[TableRow], clickable: bool) -> Option<usize> {
    let width = ui.available_width();
    let cols = [0.0, width * 0.40, width * 0.56, width * 0.72];
    let bar_x = width * 0.76;
    let max = rows.iter().map(|r| r.rx + r.tx).max().unwrap_or(0).max(1) as f64;

    let (head, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
    let painter = ui.painter();
    let hy = head.center().y;
    painter.text(pos2(head.left() + 4.0, hy), Align2::LEFT_CENTER, first_col, theme::body(12.0), p.faint);
    for (i, name) in ["Download", "Upload", "Total"].iter().enumerate() {
        painter.text(pos2(head.left() + cols[i + 1] + 4.0, hy), Align2::RIGHT_CENTER, *name, theme::body(12.0), p.faint);
    }

    let mut clicked = None;
    for (i, r) in rows.iter().enumerate() {
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), if clickable { Sense::click() } else { Sense::hover() });
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(5), p.hover);
        }
        painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0_f32, p.line));
        let y = rect.center().y;
        painter.text(pos2(rect.left() + 4.0, y), Align2::LEFT_CENTER, &r.label, theme::body(13.5), p.text);
        let values = [r.rx, r.tx, r.rx + r.tx];
        for (k, v) in values.iter().enumerate() {
            let color = if k == 2 { p.text } else { p.dim };
            painter.text(pos2(rect.left() + cols[k + 1] + 4.0, y), Align2::RIGHT_CENTER, format::bytes(*v), theme::display(14.0), color);
        }
        let share = (r.rx + r.tx) as f64 / max;
        let full = rect.right() - 6.0 - (rect.left() + bar_x);
        if full > 10.0 && share > 0.0 {
            let w = (full * share as f32).max(2.0);
            let bar = Rect::from_min_size(pos2(rect.left() + bar_x, y - 3.0), vec2(w, 6.0));
            painter.rect_filled(bar, CornerRadius::same(3), theme::rgb(heat::share_color(share, p.dark)));
        }
        if clickable && resp.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}
