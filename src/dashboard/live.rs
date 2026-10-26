//! Live page: the two readouts, the mirrored waveform, and today's totals.

use super::charts::{self, Bar, Wave};
use super::theme::{self, Palette};
use super::{ui, App};
use crate::format;
use crate::heat;
use eframe::egui::{self, pos2, vec2, Layout, RichText, Sense, Ui};

pub const RANGES: [(u32, &str); 4] = [(60, "1 min"), (300, "5 min"), (900, "15 min"), (3600, "1 hour")];

/// Big readout. The number rolls and, with animations on, grows with the traffic.
fn readout(ui: &mut Ui, p: &Palette, down: bool, row: &crate::anim::RowState, animate: bool, cfg: &crate::config::Config, caption: &str) {
    let base = 46.0;
    let size = if animate { base * row.scale } else { base };
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 74.0), Sense::hover());
    let painter = ui.painter();
    let (color, hot) = if animate {
        (theme::rgb(row.rgb()), row.heat)
    } else {
        match heat::text_color(row.speed, &cfg.heat, p.dark) {
            Some(c) => (theme::rgb(c), 1.0),
            None => (p.text, 0.0),
        }
    };
    let baseline = rect.top() + 56.0;
    let arrow_size = size * 0.42;
    let arrow_color = if hot > 0.0 { theme::blend(p.dim, color, hot) } else { p.dim };
    ui::arrow(painter, pos2(rect.left() + arrow_size / 2.0 + 1.0, baseline - size * 0.36), arrow_size, down, arrow_color, (size * 0.06).max(2.0));

    let s = format::speed(row.speed, cfg.unit, cfg.decimals);
    let x = rect.left() + arrow_size + size * 0.28;
    let value = painter.layout_no_wrap(s.value.clone(), theme::display(size), color);
    let value_w = value.size().x;
    // Bahnschrift's descent is about a fifth of the size; align both runs on the baseline.
    painter.galley(pos2(x, baseline - value.size().y + size * 0.2), value, color);
    let unit_size = (size * 0.42).max(16.0);
    let unit = painter.layout_no_wrap(s.unit.to_string(), theme::display(unit_size), p.dim);
    painter.galley(pos2(x + value_w + 8.0, baseline - unit.size().y + unit_size * 0.2), unit, p.dim);

    ui.add_space(2.0);
    ui.label(RichText::new(caption).font(theme::body(13.0)).color(p.dim));
}

fn totals_block(ui: &mut Ui, p: &Palette, title: &str, rx: u64, tx: u64) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(RichText::new(title).font(theme::body(13.0)).color(p.dim));
        let (v, u) = format::bytes_parts(rx + tx);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(RichText::new(v).font(theme::display(30.0)).color(p.text));
            ui.add_space(1.0);
            ui.label(RichText::new(u).font(theme::display(17.0)).color(p.dim));
        });
        ui.label(
            RichText::new(format!("{} down, {} up", format::bytes(rx), format::bytes(tx)))
                .font(theme::body(12.5))
                .color(p.faint),
        );
    });
}

fn legend(ui: &mut Ui, p: &Palette, cfg: &crate::config::Config) {
    if !cfg.heat.enabled {
        return;
    }
    let h = &cfg.heat;
    let short = |v: f64| format::speed_short(v, cfg.unit);
    let items = [
        (p.calm, format!("Under {}", short(h.busy))),
        (p.busy, format!("{} to {}", short(h.busy), short(h.heavy))),
        (p.heavy, format!("{} to {}", short(h.heavy), short(h.extreme))),
        (p.extreme, format!("Over {}", short(h.extreme))),
    ];
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        for (c, label) in items {
            let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
            ui.painter().rect_filled(r, 3, c);
            ui.label(RichText::new(label).font(theme::body(12.0)).color(p.faint));
            ui.add_space(10.0);
        }
    });
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let animate = app.animating();
    let running = app.snap.as_ref().is_some_and(|s| s.is_fresh());

    if !running {
        notice_not_running(app, ui);
        ui.add_space(16.0);
    }

    // Connection name, quiet, top right.
    ui.horizontal(|ui| {
        let (name, desc) = match &app.snap {
            Some(s) if running => (s.iface_name.clone(), s.iface_desc.clone()),
            _ => ("Not measuring".into(), String::new()),
        };
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            let resp = ui.add(egui::Label::new(RichText::new(&name).font(theme::body(13.0)).color(p.dim)).sense(Sense::click()));
            if !desc.is_empty() {
                let resp = resp.on_hover_text(format!("{desc}\nClick to choose a different network"));
                if resp.clicked() {
                    app.page = crate::shared::PAGE_SETTINGS;
                }
            }
        });
    });

    let peak = |f: fn(&crate::shared::Sample) -> f32| -> f64 {
        app.snap.as_ref().map(|s| s.samples.iter().map(f).fold(0.0f32, f32::max) as f64).unwrap_or(0.0)
    };
    let (peak_down, peak_up) = (peak(|s| s.down), peak(|s| s.up));
    ui.columns(2, |cols| {
        readout(
            &mut cols[0],
            &p,
            true,
            &app.down.row,
            animate,
            &app.cfg,
            &format!("Download, peak {} in the last hour", format::speed(peak_down, app.cfg.unit, app.cfg.decimals)),
        );
        readout(
            &mut cols[1],
            &p,
            false,
            &app.up.row,
            animate,
            &app.cfg,
            &format!("Upload, peak {} in the last hour", format::speed(peak_up, app.cfg.unit, app.cfg.decimals)),
        );
    });

    ui.add_space(18.0);
    ui.horizontal(|ui| {
        legend(ui, &p, &app.cfg);
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            let mut range = app.live_range;
            if ui::segmented(ui, &p, "range", &mut range, &RANGES) {
                app.live_range = range;
            }
        });
    });
    ui.add_space(8.0);

    let now_ms = if animate {
        chrono::Utc::now().timestamp_millis() as f64 - 1000.0
    } else {
        app.snap.as_ref().and_then(|s| s.latest()).map(|s| s.t as f64).unwrap_or(0.0)
    };
    let empty = Vec::new();
    let samples = app.snap.as_ref().map(|s| &s.samples).unwrap_or(&empty);
    let height = (ui.available_height() - 150.0).clamp(220.0, 440.0);
    charts::waveform(
        ui,
        &p,
        &Wave {
            samples,
            window_s: app.live_range as f64,
            now_ms,
            heat: &app.cfg.heat,
            unit: app.cfg.unit,
            animate,
        },
        height,
    );

    ui.add_space(18.0);
    let (today, month) = app.totals();
    let last24 = app.last_24h();
    ui.columns(3, |cols| {
        totals_block(&mut cols[0], &p, "Today", today.0, today.1);
        totals_block(&mut cols[1], &p, "This month", month.0, month.1);
        let ui = &mut cols[2];
        ui.label(RichText::new("Last 24 hours").font(theme::body(13.0)).color(p.dim));
        let bars: Vec<Bar> = last24
            .iter()
            .map(|r| {
                let start: i64 = r.key.parse().unwrap_or(0);
                let t = chrono::TimeZone::timestamp_opt(&chrono::Local, start, 0).single();
                let title = t.map(|t| format!("{} to {}", t.format("%H:00"), (t + chrono::Duration::hours(1)).format("%H:00"))).unwrap_or_default();
                Bar { label: String::new(), title, rx: r.rx, tx: r.tx }
            })
            .collect();
        charts::mirrored_bars(ui, &p, "last24", &bars, 64.0, false, false);
    });
}

fn notice_not_running(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    egui::Frame::NONE
        .fill(p.layer)
        .stroke(egui::Stroke::new(1.0, p.line))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(18, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                ui.painter().circle_filled(r.center(), 4.0, p.heavy);
                ui.vertical(|ui| {
                    ui.label(RichText::new("SpeedMeter isn't measuring right now").font(theme::body(14.0)).color(p.text));
                    ui.label(
                        RichText::new("Start it to see live speeds in the taskbar and keep recording history.")
                            .font(theme::body(12.5))
                            .color(p.dim),
                    );
                });
                ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::button(ui, &p, "Start SpeedMeter", true).clicked() {
                        crate::monitor::start_in_background();
                        app.reader_retry_now();
                    }
                });
            });
        });
}
