//! Per-app pieces shared by the Live and History pages: the list of apps with
//! their icons, the live speeds, and the card that turns counting on.

use super::charts::{self, TableRow};
use super::theme;
use super::{ui, App};
use crate::apps::{self, ServiceState};
use crate::format::{self, UnitMode};
use crate::heat;
use crate::history::AppRow;
use crate::shared::{AppsState, Snapshot};
use eframe::egui::{self, pos2, vec2, Align2, CornerRadius, Layout, Rect, RichText, Sense, Stroke, Ui};
use std::collections::HashMap;
use std::sync::atomic::Ordering;

/// Icon size in physical pixels for a 20-point icon.
fn icon_px(ui: &Ui) -> u32 {
    (20.0 * ui.ctx().pixels_per_point()).round() as u32
}

/// Where an app's file lives, for the tooltip.
fn location(key: &str) -> String {
    match apps::special(key) {
        Some((_, note, _)) => note.to_string(),
        None => key.to_string(),
    }
}

/// Table rows for apps from the history.
pub fn rows(app: &mut App, ui: &Ui, list: &[AppRow]) -> Vec<TableRow> {
    let p = app.pal;
    let px = icon_px(ui);
    list.iter()
        .map(|r| {
            let lead = app.icons.lead(ui.ctx(), &r.key, &r.name, px, app.has_icons);
            let detail = if r.file.eq_ignore_ascii_case(&r.name) || r.key.starts_with("svc:") { String::new() } else { r.file.clone() };
            let tip = vec![
                (r.name.clone(), p.text, theme::body(14.0)),
                (location(&r.key), p.dim, theme::body(12.0)),
                (format!("↓ {} downloaded", format::bytes(r.rx)), p.text, theme::display(15.0)),
                (format!("↑ {} uploaded", format::bytes(r.tx)), p.text, theme::display(15.0)),
            ];
            TableRow { label: r.name.clone(), detail, lead, rx: r.rx, tx: r.tx, tip }
        })
        .collect()
}

/// A few seconds of speeds per app, so the live list doesn't flicker with every
/// one-second reading.
#[derive(Default)]
pub struct AppRates {
    last_t: i64,
    /// Last few readings per app (download, upload), newest last.
    recent: HashMap<String, Vec<(f32, f32)>>,
}

const WINDOW: usize = 3;

impl AppRates {
    pub fn update(&mut self, snap: &Snapshot) {
        let Some(t) = snap.latest().map(|s| s.t) else { return };
        if t == self.last_t {
            return;
        }
        self.last_t = t;
        for (key, list) in self.recent.iter_mut() {
            let (d, u) = snap.apps.iter().find(|a| &a.key == key).map_or((0.0, 0.0), |a| (a.down, a.up));
            list.push((d, u));
        }
        for a in &snap.apps {
            self.recent.entry(a.key.clone()).or_insert_with(|| vec![(a.down, a.up)]);
        }
        for list in self.recent.values_mut() {
            if list.len() > WINDOW + 3 {
                list.remove(0);
            }
        }
        // Forget apps that have been quiet for a few seconds.
        self.recent.retain(|_, l| l.iter().rev().take(WINDOW + 2).any(|(d, u)| d + u > 0.0));
    }

    /// (key, download, upload) averaged over the last few seconds, busiest first.
    pub fn current(&self) -> Vec<(String, f64, f64)> {
        let mut out: Vec<(String, f64, f64)> = self
            .recent
            .iter()
            .map(|(k, l)| {
                let tail = &l[l.len().saturating_sub(WINDOW)..];
                let n = tail.len().max(1) as f64;
                let d = tail.iter().map(|x| x.0 as f64).sum::<f64>() / n;
                let u = tail.iter().map(|x| x.1 as f64).sum::<f64>() / n;
                (k.clone(), d, u)
            })
            .filter(|(_, d, u)| d + u > 0.0)
            .collect();
        out.sort_by(|a, b| (b.1 + b.2).total_cmp(&(a.1 + a.2)));
        out
    }
}

/// Apps and their speeds right now, with a share bar.
pub fn live_list(app: &mut App, ui: &mut Ui, max_rows: usize) {
    let p = app.pal;
    let unit: UnitMode = app.cfg.unit;
    let current = app.app_rates.current();
    if current.is_empty() {
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::hover());
        ui.painter().text(rect.left_center() + vec2(4.0, 0.0), Align2::LEFT_CENTER, "No app is using the network right now.", theme::body(13.5), p.dim);
        return;
    }
    let width = ui.available_width();
    let cols = [width * 0.46, width * 0.62];
    let bar_x = width * 0.68;
    let max = current.iter().map(|(_, d, u)| d + u).fold(0.0, f64::max).max(1.0);
    let px = icon_px(ui);

    let (head, _) = ui.allocate_exact_size(vec2(width, 26.0), Sense::hover());
    let painter = ui.painter();
    painter.text(pos2(head.left() + 4.0, head.center().y), Align2::LEFT_CENTER, "App", theme::body(12.0), p.faint);
    painter.text(pos2(head.left() + cols[0], head.center().y), Align2::RIGHT_CENTER, "Download", theme::body(12.0), p.faint);
    painter.text(pos2(head.left() + cols[1], head.center().y), Align2::RIGHT_CENTER, "Upload", theme::body(12.0), p.faint);

    for (key, down, up) in current.iter().take(max_rows) {
        let (name, file) = match app.icons.look(ui.ctx(), key, px) {
            Some(l) => (l.name.clone(), l.file.clone()),
            None => (apps::special(key).map_or_else(|| key.rsplit('\\').next().unwrap_or(key).to_string(), |s| s.0.to_string()), String::new()),
        };
        let lead = app.icons.lead(ui.ctx(), key, &name, px, app.has_icons);
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 34.0), Sense::hover());
        let painter = ui.painter();
        if resp.hovered() {
            painter.rect_filled(rect, CornerRadius::same(5), p.hover);
        }
        painter.line_segment([rect.left_top(), rect.right_top()], Stroke::new(1.0_f32, p.line));
        let y = rect.center().y;
        charts::draw_lead(painter, &p, Rect::from_center_size(pos2(rect.left() + 16.0, y), vec2(20.0, 20.0)), lead);
        let label_w = cols[0] - 90.0 - 36.0;
        let label = charts::fit(painter, &name, theme::body(13.5), p.text, label_w);
        let end = rect.left() + 36.0 + label.size().x;
        painter.galley(pos2(rect.left() + 36.0, y - label.size().y / 2.0), label, p.text);
        let room = rect.left() + 36.0 + label_w - end - 8.0;
        if !file.is_empty() && !file.eq_ignore_ascii_case(&name) && room > 30.0 {
            let detail = painter.layout_no_wrap(file.clone(), theme::body(12.0), p.faint);
            if detail.size().x <= room {
                painter.galley(pos2(end + 8.0, y - detail.size().y / 2.0), detail, p.faint);
            }
        }
        let tint = |v: f64| match heat::text_color(v, &app.cfg.heat, p.dark) {
            Some(rgb) => theme::rgb(rgb),
            None => p.text,
        };
        painter.text(pos2(rect.left() + cols[0], y), Align2::RIGHT_CENTER, format::speed(*down, unit, app.cfg.decimals).to_string(), theme::display(14.0), tint(*down));
        painter.text(pos2(rect.left() + cols[1], y), Align2::RIGHT_CENTER, format::speed(*up, unit, app.cfg.decimals).to_string(), theme::display(14.0), p.dim);
        let share = (down + up) / max;
        let full = rect.right() - 6.0 - (rect.left() + bar_x);
        if full > 10.0 && share > 0.0 {
            let w = (full * share as f32).max(2.0);
            let bar = Rect::from_min_size(pos2(rect.left() + bar_x, y - 3.0), vec2(w, 6.0));
            painter.rect_filled(bar, CornerRadius::same(3), theme::rgb(heat::speed_color(down + up, &app.cfg.heat, p.dark)));
        }
        if let Some(pp) = resp.hover_pos() {
            ui::tooltip(ui.ctx(), &p, pp, &[(name.clone(), p.text, theme::body(14.0)), (location(key), p.dim, theme::body(12.0))]);
        }
    }
    if current.len() > max_rows {
        ui.add_space(4.0);
        ui.label(RichText::new(format!("and {} more", current.len() - max_rows)).font(theme::body(12.5)).color(p.faint));
    }
}

/// Why per-app counting isn't on, with the button that fixes it. Returns false
/// when counting works and nothing was drawn.
pub fn status_card(app: &mut App, ui: &mut Ui, live_state: Option<AppsState>) -> bool {
    app.refresh_service();
    let p = app.pal;
    let busy = app.service.busy.load(Ordering::Acquire);
    let failed = app.service.failed.lock().ok().and_then(|f| f.clone());
    // The meter is receiving per-app numbers: nothing to fix here.
    if live_state == Some(AppsState::Counting) && !busy && failed.is_none() {
        return false;
    }
    let s = &app.service;
    let (title, desc, action): (&str, String, Option<(&str, bool)>) = if busy {
        ("Waiting for approval", "Windows is asking for an administrator's approval to set up per-app counting.".into(), None)
    } else if let Some(e) = failed {
        ("Per-app counting couldn't be set up", e, Some(("Try again", true)))
    } else if s.state == ServiceState::NotInstalled {
        (
            "See which apps use your data",
            "A small background service counts each app's traffic, live and in your history. It runs with limited rights and shares only byte counts. Turning it on asks for administrator approval once.".into(),
            Some(("Turn on", true)),
        )
    } else if s.needs_update() {
        ("Per-app counting needs an update", "This version of SpeedMeter comes with a newer counter. Updating asks for administrator approval.".into(), Some(("Update", true)))
    } else if s.state == ServiceState::Stopped {
        ("Per-app counting is stopped", "The SpeedMeter per-app counter service isn't running.".into(), Some(("Start", true)))
    } else if let Some(e) = &s.error {
        ("Per-app counting has a problem", e.clone(), Some(("Restart", false)))
    } else if live_state == Some(AppsState::Off) && s.state == ServiceState::Running && s.version.is_none() {
        ("Per-app counting is starting", "It begins counting within a few seconds.".into(), None)
    } else {
        return false;
    };
    egui::Frame::NONE
        .fill(p.layer)
        .stroke(Stroke::new(1.0_f32, p.line))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(18, 14))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let text_w = (ui.available_width() - 180.0).max(240.0);
                ui.allocate_ui_with_layout(vec2(text_w, 0.0), Layout::top_down(egui::Align::Min), |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.label(RichText::new(title).font(theme::body(14.5)).color(p.text));
                    ui.add(egui::Label::new(RichText::new(desc).font(theme::body(12.5)).color(p.dim)).wrap());
                });
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| match action {
                    Some((label, primary)) => {
                        if ui::button(ui, &p, label, primary).clicked() {
                            app.change_service(true);
                        }
                    }
                    None => {
                        ui.add(egui::Spinner::new().color(p.accent));
                    }
                });
            });
        });
    true
}
