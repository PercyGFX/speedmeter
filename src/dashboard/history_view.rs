//! History page: totals by hour, day, month or year. Clicking a bar or a row
//! drills into it (year, then month, then day).

use super::apps_ui;
use super::charts::{self, Bar, TableRow};
use super::theme;
use super::{ui, App};
use crate::format;
use crate::history::{self, AppRow, Row};
use chrono::{Datelike, Duration, NaiveDate};
use eframe::egui::{self, vec2, Layout, RichText, Ui};
use std::time::Instant;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scale {
    Day,
    Month,
    Year,
    All,
}

/// What the list under the chart shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListMode {
    Apps,
    Periods,
}

pub struct HistoryState {
    pub scale: Scale,
    pub anchor: NaiveDate,
    loaded: Option<Loaded>,
    list: ListMode,
    /// Day view: the hour picked on the chart; the app list shows only that hour.
    hour: Option<u32>,
    all_apps: bool,
    apps: Option<LoadedApps>,
    /// The period on screen, to reset the hour and "show all" when it changes.
    period: Option<(Scale, NaiveDate)>,
}

struct LoadedApps {
    key: (Scale, NaiveDate, Option<u32>),
    at: Instant,
    rows: Vec<AppRow>,
}

/// How many apps show before "Show all".
const TOP_APPS: usize = 12;

struct Loaded {
    key: (Scale, NaiveDate),
    at: Instant,
    rows: Vec<Row>,
    /// Day view only: this day is older than the minute-level data we keep.
    detail_missing: Option<(u64, u64)>,
}

impl HistoryState {
    pub fn new() -> Self {
        Self {
            scale: Scale::Month,
            anchor: history::today(),
            loaded: None,
            list: ListMode::Apps,
            hour: None,
            all_apps: false,
            apps: None,
            period: None,
        }
    }

    /// Forces a reload (after an import or clearing history).
    pub fn invalidate(&mut self) {
        self.loaded = None;
        self.apps = None;
    }
}

/// The last day of the period that starts at `start`.
fn period_last_day(scale: Scale, start: NaiveDate) -> NaiveDate {
    match scale {
        Scale::Day => start,
        Scale::Month | Scale::Year => shift(scale, start, 1).pred_opt().unwrap_or(start),
        Scale::All => history::today(),
    }
}

fn load_apps(c: &rusqlite::Connection, scale: Scale, start: NaiveDate, hour: Option<u32>) -> Vec<AppRow> {
    if let (Scale::Day, Some(h)) = (scale, hour) {
        let from = history::hour_ts(start, h);
        return history::apps_in_hours(c, from, from + 3600);
    }
    let (from, to) = match scale {
        Scale::All => ("0000-00-00".to_string(), "9999-99-99".to_string()),
        _ => (history::day_key(start), history::day_key(period_last_day(scale, start))),
    };
    history::apps_between(c, &from, &to)
}

/// The app list for the period on screen.
fn apps_body(app: &mut App, ui: &mut Ui, scale: Scale, anchor: NaiveDate) {
    let p = app.pal;
    let start = period_start(scale, anchor);
    let hour = if scale == Scale::Day { app.hist.hour } else { None };
    let key = (scale, start, hour);
    let stale = app.hist.apps.as_ref().is_none_or(|l| l.key != key || l.at.elapsed().as_secs() >= 30);
    if stale {
        if let Some(db) = app.db() {
            let rows = load_apps(db, scale, start, hour);
            app.hist.apps = Some(LoadedApps { key, at: Instant::now(), rows });
            app.first_app_day = app.db().and_then(history::first_app_day);
        }
    }
    let list = app.hist.apps.as_ref().map(|l| l.rows.clone()).unwrap_or_default();
    let live_state = app.snap.as_ref().filter(|s| s.is_fresh()).map(|s| s.apps_state);
    let note = |ui: &mut Ui, text: &str| {
        ui.add(egui::Label::new(RichText::new(text).font(theme::body(12.5)).color(p.faint)).wrap());
    };

    if list.is_empty() {
        if apps_ui::status_card(app, ui, live_state) {
            return;
        }
        let what = match scale {
            Scale::Day => "day",
            Scale::Month => "month",
            Scale::Year => "year",
            Scale::All => "time",
        };
        let msg = match app.first_app_day {
            _ if hour.is_some() => "No per-app detail for this hour.".to_string(),
            Some(f) if period_last_day(scale, start) < f => {
                format!("Per-app counting started on {}, after this {what}.", f.format("%B %-d, %Y"))
            }
            None => "No app traffic yet. Apps appear here about a minute after they use the network.".to_string(),
            _ => format!("No app traffic recorded for {}.", title(scale, anchor)),
        };
        let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 40.0), egui::Sense::hover());
        ui.painter().text(rect.left_center() + vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, msg, theme::body(13.5), p.dim);
        return;
    }

    let shown = if app.hist.all_apps { list.len() } else { list.len().min(TOP_APPS) };
    let rows = apps_ui::rows(app, ui, &list[..shown]);
    charts::table(ui, &p, "App", &rows, false);
    if list.len() > TOP_APPS {
        ui.add_space(6.0);
        let label = if app.hist.all_apps { "Show fewer".to_string() } else { format!("Show all {} apps", list.len()) };
        let link = ui.add(egui::Label::new(RichText::new(label).font(theme::body(13.0)).color(p.accent)).sense(egui::Sense::click()));
        if link.clicked() {
            app.hist.all_apps = !app.hist.all_apps;
        }
    }
    ui.add_space(10.0);
    if let Some(f) = app.first_app_day.filter(|f| *f > start && scale != Scale::Day) {
        note(ui, &format!("Per-app counting started on {}. Traffic before that isn't split by app.", f.format("%B %-d, %Y")));
    }
    if list.iter().any(|r| r.key == crate::apps::OTHER) {
        note(ui, "Other is traffic no single app accounts for, such as packet headers and other users' apps.");
    }
    if scale == Scale::Day && hour.is_none() {
        note(ui, "Click an hour in the chart to see its apps.");
    }
    ui.add_space(6.0);
    apps_ui::status_card(app, ui, live_state);
}

fn add_months(d: NaiveDate, n: i32) -> NaiveDate {
    let m = d.year() * 12 + d.month0() as i32 + n;
    NaiveDate::from_ymd_opt(m.div_euclid(12), m.rem_euclid(12) as u32 + 1, 1).unwrap_or(d)
}

fn period_start(scale: Scale, d: NaiveDate) -> NaiveDate {
    match scale {
        Scale::Day => d,
        Scale::Month => NaiveDate::from_ymd_opt(d.year(), d.month(), 1).unwrap_or(d),
        Scale::Year => NaiveDate::from_ymd_opt(d.year(), 1, 1).unwrap_or(d),
        Scale::All => d,
    }
}

fn shift(scale: Scale, d: NaiveDate, dir: i32) -> NaiveDate {
    match scale {
        Scale::Day => d + Duration::days(dir as i64),
        Scale::Month => add_months(d, dir),
        Scale::Year => NaiveDate::from_ymd_opt(d.year() + dir, 1, 1).unwrap_or(d),
        Scale::All => d,
    }
}

fn title(scale: Scale, d: NaiveDate) -> String {
    match scale {
        Scale::Day => d.format("%A, %B %-d, %Y").to_string(),
        Scale::Month => d.format("%B %Y").to_string(),
        Scale::Year => d.format("%Y").to_string(),
        Scale::All => "All time".to_string(),
    }
}

fn load(c: &rusqlite::Connection, scale: Scale, anchor: NaiveDate) -> (Vec<Row>, Option<(u64, u64)>) {
    match scale {
        Scale::Day => {
            let rows = history::hours_of_day(c, anchor);
            let total: u64 = rows.iter().map(|r| r.total()).sum();
            let day = history::day_totals(c, &history::day_key(anchor));
            let missing = (total == 0 && day.0 + day.1 > 0).then_some(day);
            (rows, missing)
        }
        Scale::Month => {
            let found = history::days_of_month(c, &history::month_key(anchor));
            let days = history::days_in_month(anchor.year(), anchor.month());
            let rows = (1..=days)
                .map(|d| {
                    let key = format!("{}-{:02}", history::month_key(anchor), d);
                    found.iter().find(|r| r.key == key).cloned().unwrap_or(Row { key, ..Default::default() })
                })
                .collect();
            (rows, None)
        }
        Scale::Year => {
            let found = history::months_of_year(c, anchor.year());
            let rows = (1..=12)
                .map(|m| {
                    let key = format!("{}-{:02}", anchor.year(), m);
                    found.iter().find(|r| r.key == key).cloned().unwrap_or(Row { key, ..Default::default() })
                })
                .collect();
            (rows, None)
        }
        Scale::All => (history::years(c), None),
    }
}

/// The date a row stands for, used for drill-down.
fn row_date(scale: Scale, anchor: NaiveDate, i: usize, r: &Row) -> Option<NaiveDate> {
    match scale {
        Scale::Day => None,
        Scale::Month => NaiveDate::parse_from_str(&r.key, "%Y-%m-%d").ok(),
        Scale::Year => NaiveDate::from_ymd_opt(anchor.year(), i as u32 + 1, 1),
        Scale::All => r.key.parse().ok().and_then(|y| NaiveDate::from_ymd_opt(y, 1, 1)),
    }
}

fn drill(state: &mut HistoryState, date: NaiveDate) {
    state.scale = match state.scale {
        Scale::All => Scale::Year,
        Scale::Year => Scale::Month,
        _ => Scale::Day,
    };
    state.anchor = date;
}

fn stat(ui: &mut Ui, p: &theme::Palette, value: &str, unit: &str, caption: &str) {
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 1.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 5.0;
            ui.label(RichText::new(value).font(theme::display(26.0)).color(p.text));
            if !unit.is_empty() {
                ui.label(RichText::new(unit).font(theme::display(15.0)).color(p.dim));
            }
        });
        ui.label(RichText::new(caption).font(theme::body(12.5)).color(p.dim));
    });
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let today = history::today();

    // Header: period navigation on the left, scale on the right.
    let first_day = app.first_day;
    ui.horizontal(|ui| {
        let st = &mut app.hist;
        let can_back = st.scale != Scale::All
            && first_day.is_none_or(|f| period_start(st.scale, st.anchor) > period_start(st.scale, f));
        let can_fwd = st.scale != Scale::All && shift(st.scale, period_start(st.scale, st.anchor), 1) <= today;
        if st.scale != Scale::All {
            if ui::icon_button(ui, &p, "‹", can_back) {
                st.anchor = shift(st.scale, period_start(st.scale, st.anchor), -1);
            }
        }
        ui.label(RichText::new(title(st.scale, st.anchor)).font(theme::display(24.0)).color(p.text));
        if st.scale != Scale::All {
            if ui::icon_button(ui, &p, "›", can_fwd) {
                st.anchor = shift(st.scale, period_start(st.scale, st.anchor), 1).min(today);
            }
        }
        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
            let mut scale = st.scale;
            if ui::segmented(ui, &p, "scale", &mut scale, &[(Scale::Day, "Day"), (Scale::Month, "Month"), (Scale::Year, "Year"), (Scale::All, "All time")]) {
                st.scale = scale;
                if scale != Scale::All && st.anchor > today {
                    st.anchor = today;
                }
            }
        });
    });

    // Load (cached; refreshed every 30 s so today's numbers keep up).
    let key = (app.hist.scale, period_start(app.hist.scale, app.hist.anchor));
    if app.hist.period != Some(key) {
        app.hist.period = Some(key);
        app.hist.hour = None;
        app.hist.all_apps = false;
    }
    let stale = app.hist.loaded.as_ref().is_none_or(|l| l.key != key || l.at.elapsed().as_secs() >= 30);
    if stale {
        if let Some(db) = app.db() {
            let (rows, detail_missing) = load(db, key.0, key.1);
            app.hist.loaded = Some(Loaded { key, at: Instant::now(), rows, detail_missing });
        }
    }
    let scale = app.hist.scale;
    let anchor = app.hist.anchor;
    let Some(loaded) = app.hist.loaded.as_ref() else {
        ui.add_space(20.0);
        ui.label(RichText::new("The history database couldn't be opened.").color(p.dim));
        return;
    };
    let rows = loaded.rows.clone();
    let detail_missing = loaded.detail_missing;

    // Summary line.
    ui.add_space(14.0);
    let rx: u64 = rows.iter().map(|r| r.rx).sum();
    let tx: u64 = rows.iter().map(|r| r.tx).sum();
    let busiest = rows.iter().enumerate().filter(|(_, r)| r.total() > 0).max_by_key(|(_, r)| r.total());
    let label_for = |i: usize, r: &Row| -> String {
        match scale {
            Scale::Day => format!("{}:00", r.key),
            Scale::Month => row_date(scale, anchor, i, r).map(|d| d.format("%b %-d").to_string()).unwrap_or_default(),
            Scale::Year => row_date(scale, anchor, i, r).map(|d| d.format("%B").to_string()).unwrap_or_default(),
            Scale::All => r.key.clone(),
        }
    };
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 44.0;
        let (rxv, rxu) = format::bytes_parts(detail_missing.map_or(rx, |d| d.0));
        let (txv, txu) = format::bytes_parts(detail_missing.map_or(tx, |d| d.1));
        stat(ui, &p, &rxv, rxu, "Downloaded");
        stat(ui, &p, &txv, txu, "Uploaded");
        match scale {
            Scale::Day => {
                let peak = rows.iter().map(|r| r.peak_rx).fold(0.0, f64::max);
                let s = format::speed(peak, app.cfg.unit, app.cfg.decimals);
                stat(ui, &p, &s.value, s.unit, "Fastest download");
            }
            Scale::Month | Scale::Year => {
                // Average over the days (or months) that were recorded, so a fresh
                // install isn't diluted by days before SpeedMeter was running.
                let count = rows.iter().filter(|r| r.total() > 0).count().max(1) as u64;
                let (v, u) = format::bytes_parts((rx + tx) / count);
                stat(ui, &p, &v, u, if scale == Scale::Month { "Daily average" } else { "Monthly average" });
            }
            Scale::All => {
                if let Some(f) = first_day {
                    stat(ui, &p, &f.format("%b %Y").to_string(), "", "Recording since");
                }
            }
        }
        if let Some((i, r)) = busiest {
            let (v, u) = format::bytes_parts(r.total());
            let what = match scale {
                Scale::Day => "Busiest hour",
                Scale::Month => "Busiest day",
                Scale::Year => "Busiest month",
                Scale::All => "Busiest year",
            };
            stat(ui, &p, &format!("{v} {u}"), "", &format!("{what}: {}", label_for(i, r)));
        }
    });

    // Chart.
    ui.add_space(18.0);
    let bars: Vec<Bar> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let (label, title) = match scale {
                Scale::Day => {
                    let h: u32 = r.key.parse().unwrap_or(0);
                    (h.to_string(), format!("{:02}:00 to {:02}:00", h, (h + 1) % 24))
                }
                Scale::Month => {
                    let d = row_date(scale, anchor, i, r);
                    (d.map(|d| d.day().to_string()).unwrap_or_default(), d.map(|d| d.format("%A, %B %-d").to_string()).unwrap_or_default())
                }
                Scale::Year => {
                    let d = row_date(scale, anchor, i, r);
                    (d.map(|d| d.format("%b").to_string()).unwrap_or_default(), d.map(|d| d.format("%B %Y").to_string()).unwrap_or_default())
                }
                Scale::All => (r.key.clone(), r.key.clone()),
            };
            Bar { label, title, rx: r.rx, tx: r.tx }
        })
        .collect();

    let nothing = rx + tx == 0;
    if let Some((drx, dtx)) = detail_missing {
        empty_state(
            ui,
            &p,
            &format!(
                "Hourly detail is kept for {} days, so only this day's total is left: {} down, {} up.",
                app.cfg.keep_minutes_days,
                format::bytes(drx),
                format::bytes(dtx)
            ),
        );
    } else if nothing {
        let msg = match scale {
            Scale::All => "No history yet. SpeedMeter records traffic for as long as it runs.".to_string(),
            _ => format!("No traffic recorded for {}.", title(scale, anchor)),
        };
        empty_state(ui, &p, &msg);
    } else {
        let selected = if scale == Scale::Day { app.hist.hour.map(|h| h as usize) } else { None };
        if let Some(i) = charts::mirrored_bars(ui, &p, "history", &bars, 250.0, app.animating(), true, selected) {
            if scale == Scale::Day {
                // An hour: show which apps used it (click again for the whole day).
                if rows[i].total() > 0 || app.hist.hour == Some(i as u32) {
                    app.hist.hour = if app.hist.hour == Some(i as u32) { None } else { Some(i as u32) };
                    app.hist.list = ListMode::Apps;
                }
            } else if let Some(d) = row_date(scale, anchor, i, &rows[i]).filter(|_| rows[i].total() > 0) {
                drill(&mut app.hist, d);
            }
        }
    }

    if nothing && detail_missing.is_none() {
        return;
    }

    // Under the chart: which apps used the data, or the list of periods.
    ui.add_space(22.0);
    let periods = match scale {
        Scale::Day => "Hours",
        Scale::Month => "Days",
        Scale::Year => "Months",
        Scale::All => "Years",
    };
    let mut mode = app.hist.list;
    ui.horizontal(|ui| {
        ui::segmented(ui, &p, "list", &mut mode, &[(ListMode::Apps, "Apps"), (ListMode::Periods, periods)]);
        if let (ListMode::Apps, Scale::Day, Some(h)) = (mode, scale, app.hist.hour) {
            ui.add_space(14.0);
            ui.label(RichText::new(format!("{:02}:00 to {:02}:00", h, (h + 1) % 24)).font(theme::body(14.0)).color(p.text));
            ui.add_space(6.0);
            let link = ui.add(egui::Label::new(RichText::new("Show whole day").font(theme::body(13.0)).color(p.accent)).sense(egui::Sense::click()));
            if link.clicked() {
                app.hist.hour = None;
            }
        }
    });
    app.hist.list = mode;
    ui.add_space(10.0);
    let table_rows: Vec<(usize, TableRow)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.total() > 0)
        .rev()
        .map(|(i, r)| {
            let label = match scale {
                Scale::Month => row_date(scale, anchor, i, r).map(|d| d.format("%a, %b %-d").to_string()).unwrap_or_default(),
                _ => label_for(i, r),
            };
            (i, TableRow { label, rx: r.rx, tx: r.tx, ..Default::default() })
        })
        .collect();
    let first_col = match scale {
        Scale::Day => "Hour",
        Scale::Month => "Day",
        Scale::Year => "Month",
        Scale::All => "Year",
    };
    let only_rows: Vec<TableRow> = table_rows.iter().map(|(_, r)| r.clone()).collect();
    let mut clicked_row = None;
    let mut clicked_day = None;
    let wide = ui.available_width() >= 760.0;
    let mut list = |app: &mut App, ui: &mut Ui| match mode {
        ListMode::Apps => apps_body(app, ui, scale, anchor),
        ListMode::Periods => clicked_row = charts::table(ui, &p, first_col, &only_rows, scale != Scale::Day),
    };
    if scale == Scale::Month && wide {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let cal_w = 300.0;
            let table_w = ui.available_width() - cal_w - 32.0;
            let column = Layout::top_down(egui::Align::Min);
            ui.allocate_ui_with_layout(vec2(table_w, 0.0), column, |ui| list(app, ui));
            ui.add_space(32.0);
            ui.allocate_ui_with_layout(vec2(cal_w, 0.0), column, |ui| {
                clicked_day = charts::calendar(ui, &p, anchor.year(), anchor.month(), &rows, today);
            });
        });
    } else {
        if scale == Scale::Month {
            ui.allocate_ui_with_layout(vec2(340.0, 0.0), Layout::top_down(egui::Align::Min), |ui| {
                clicked_day = charts::calendar(ui, &p, anchor.year(), anchor.month(), &rows, today);
            });
            ui.add_space(18.0);
        }
        list(app, ui);
    }
    if let Some(k) = clicked_row {
        let (i, _) = &table_rows[k];
        if let Some(d) = row_date(scale, anchor, *i, &rows[*i]) {
            drill(&mut app.hist, d);
        }
    }
    if let Some(d) = clicked_day {
        drill(&mut app.hist, d);
    }
}

fn empty_state(ui: &mut Ui, p: &theme::Palette, msg: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 160.0), egui::Sense::hover());
    let painter = ui.painter();
    painter.line_segment([rect.left_center(), rect.right_center()], egui::Stroke::new(1.0_f32, p.line));
    let g = painter.layout(msg.to_string(), theme::body(14.0), p.dim, (rect.width() - 80.0).min(560.0));
    let r = egui::Align2::CENTER_CENTER.anchor_size(rect.center(), g.size()).expand2(vec2(14.0, 8.0));
    painter.rect_filled(r, egui::CornerRadius::same(6), p.content);
    painter.galley(r.min + vec2(14.0, 8.0), g, p.dim);
}
