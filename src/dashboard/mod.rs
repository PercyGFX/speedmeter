//! The dashboard window (`speedmeter --dashboard`). Runs as its own process so its
//! memory is freed when closed. Live data comes from the monitor through shared
//! memory; history comes straight from the SQLite file.

mod charts;
mod history_view;
mod live;
mod settings;
mod theme;
mod ui;

use crate::anim::Track;
use crate::config::{self, Config, ThemePref};
use crate::history::{self, Row};
use crate::net::{self, Adapter};
use crate::shared::{Reader, Snapshot, PAGE_HISTORY, PAGE_LIVE, PAGE_SETTINGS};
use crate::{format, monitor, sys, update};
use eframe::egui::{self, vec2, CornerRadius, Margin, RichText, Stroke};
use history_view::HistoryState;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};
use theme::Palette;
use windows::core::w;
use windows::Win32::Foundation::{GetLastError, BOOL, COLORREF, ERROR_ALREADY_EXISTS, HWND};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_CAPTION_COLOR, DWMWA_TEXT_COLOR, DWMWA_USE_IMMERSIVE_DARK_MODE,
};
use windows::Win32::System::Threading::CreateMutexW;

pub const TITLE: &str = "SpeedMeter";
const ICON_RGBA: &[u8] = include_bytes!("../../assets/icon_64.rgba");

#[derive(Clone, Debug, Default)]
pub enum UpdateState {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(update::Release),
    Downloading {
        version: String,
        got: u64,
        total: u64,
    },
    Installing(String),
    Failed(String),
}

pub struct App {
    pub page: u32,
    pub pal: Palette,
    pub cfg: Config,
    saved: Config,
    cfg_mtime: Option<SystemTime>,
    reader: Option<Reader>,
    reader_retry: Instant,
    pub snap: Option<Snapshot>,
    db: Option<rusqlite::Connection>,
    db_tried: bool,
    pub live_range: u32,
    pub hist: HistoryState,
    pub adapters: Vec<Adapter>,
    adapters_at: Option<Instant>,
    pub first_day: Option<chrono::NaiveDate>,
    totals_cache: Option<(Instant, (u64, u64), (u64, u64))>,
    last24_cache: Option<(Instant, Vec<Row>)>,
    pub confirm_clear: bool,
    pub show_all_adapters: bool,
    pub down: Track,
    pub up: Track,
    /// Height of the content viewport, for sizing the live chart.
    pub view_h: f32,
    last_frame: Instant,
    motion_ok: bool,
    slow_checked: Instant,
    hwnd: Option<isize>,
    styled: Option<Palette>,
    has_icons: bool,
    logo: egui::TextureHandle,
    pub update: Arc<Mutex<UpdateState>>,
    ctx: egui::Context,
}

fn palette_for(cfg: &Config) -> Palette {
    let dark = match cfg.theme {
        ThemePref::Dark => true,
        ThemePref::Light => false,
        ThemePref::System => sys::apps_are_dark(),
    };
    if dark {
        Palette::dark()
    } else {
        Palette::light()
    }
}

fn window_hwnd(cc: &eframe::CreationContext) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(h) => Some(h.hwnd.get()),
        _ => None,
    }
}

fn colorref(c: egui::Color32) -> COLORREF {
    COLORREF(c.r() as u32 | (c.g() as u32) << 8 | (c.b() as u32) << 16)
}

/// Colors the native title bar like the navigation rail, so the window reads as one
/// surface while keeping Windows' own caption buttons and snap layouts.
fn style_title_bar(hwnd: isize, p: &Palette) {
    let h = HWND(hwnd as *mut _);
    let dark = BOOL::from(p.dark);
    let caption = colorref(p.base);
    let text = colorref(p.dim);
    unsafe {
        let _ = DwmSetWindowAttribute(h, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(h, DWMWA_CAPTION_COLOR, &caption as *const _ as *const _, 4);
        let _ = DwmSetWindowAttribute(h, DWMWA_TEXT_COLOR, &text as *const _ as *const _, 4);
    }
}

impl App {
    fn new(cc: &eframe::CreationContext, page: u32) -> Self {
        let has_icons = theme::install_fonts(&cc.egui_ctx);
        let cfg = config::load();
        let pal = palette_for(&cfg);
        theme::apply(&cc.egui_ctx, &pal);
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([64, 64], ICON_RGBA),
            egui::TextureOptions::LINEAR,
        );
        let mut app = Self {
            page,
            pal,
            saved: cfg.clone(),
            cfg,
            cfg_mtime: config::modified(),
            reader: Reader::open(),
            reader_retry: Instant::now(),
            snap: None,
            db: None,
            db_tried: false,
            live_range: 60,
            hist: HistoryState::new(),
            adapters: Vec::new(),
            adapters_at: None,
            first_day: None,
            totals_cache: None,
            last24_cache: None,
            confirm_clear: false,
            show_all_adapters: false,
            down: Track::default(),
            up: Track::default(),
            view_h: 700.0,
            last_frame: Instant::now(),
            motion_ok: sys::windows_animations_enabled(),
            slow_checked: Instant::now(),
            hwnd: window_hwnd(cc),
            styled: None,
            has_icons,
            logo,
            update: Arc::new(Mutex::new(UpdateState::Idle)),
            ctx: cc.egui_ctx.clone(),
        };
        app.first_day = app.db().and_then(history::first_day);
        app.check_updates(false);
        app
    }

    pub fn animating(&self) -> bool {
        self.cfg.animations && self.motion_ok
    }

    pub fn db(&mut self) -> Option<&rusqlite::Connection> {
        if self.db.is_none() && !self.db_tried {
            self.db_tried = true;
            self.db = history::open(&config::db_path()).ok();
        }
        self.db.as_ref()
    }

    pub fn reader_retry_now(&mut self) {
        self.reader = None;
        self.reader_retry = Instant::now() - Duration::from_secs(10);
    }

    /// (today, this month) as (download, upload). Live from the monitor when it runs.
    pub fn totals(&mut self) -> ((u64, u64), (u64, u64)) {
        if let Some(s) = self.snap.as_ref().filter(|s| s.is_fresh()) {
            return ((s.today_rx, s.today_tx), (s.month_rx, s.month_tx));
        }
        if let Some((at, t, m)) = self.totals_cache {
            if at.elapsed() < Duration::from_secs(30) {
                return (t, m);
            }
        }
        let today = history::today();
        let (t, m) = match self.db() {
            Some(db) => (history::day_totals(db, &history::day_key(today)), history::month_totals(db, &history::month_key(today))),
            None => ((0, 0), (0, 0)),
        };
        self.totals_cache = Some((Instant::now(), t, m));
        (t, m)
    }

    pub fn last_24h(&mut self) -> Vec<Row> {
        if let Some((at, rows)) = &self.last24_cache {
            if at.elapsed() < Duration::from_secs(60) {
                return rows.clone();
            }
        }
        let rows = self.db().map(|db| history::recent_hours(db, 24)).unwrap_or_default();
        self.last24_cache = Some((Instant::now(), rows.clone()));
        rows
    }

    pub fn refresh_adapters(&mut self) {
        if self.adapters_at.is_none_or(|t| t.elapsed() > Duration::from_secs(5)) {
            self.adapters = net::list_adapters();
            self.adapters_at = Some(Instant::now());
        }
    }

    pub fn clear_history(&mut self) {
        if let Some(db) = self.db() {
            if let Err(e) = history::clear(db) {
                sys::log(&format!("clearing history failed: {e}"));
            }
        }
        monitor::notify_reload(true);
        self.hist.invalidate();
        self.first_day = None;
        self.totals_cache = None;
        self.last24_cache = None;
    }

    pub fn check_updates(&self, manual: bool) {
        let state = self.update.clone();
        {
            let Ok(mut s) = state.lock() else { return };
            if matches!(*s, UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installing(_)) {
                return;
            }
            if manual {
                *s = UpdateState::Checking;
            }
        }
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let next = match update::check() {
                Ok(Some(r)) => UpdateState::Available(r),
                Ok(None) => UpdateState::UpToDate,
                Err(e) if manual => UpdateState::Failed(e),
                Err(_) => UpdateState::Idle,
            };
            if let Ok(mut s) = state.lock() {
                *s = next;
            }
            ctx.request_repaint();
        });
    }

    pub fn start_update(&self, rel: update::Release) {
        let state = self.update.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let set = |s: UpdateState| {
                if let Ok(mut g) = state.lock() {
                    *g = s;
                }
                ctx.request_repaint();
            };
            set(UpdateState::Downloading { version: rel.version.clone(), got: 0, total: rel.size });
            let result = update::download(&rel, |got, total| {
                set(UpdateState::Downloading { version: rel.version.clone(), got, total });
            });
            match result.and_then(|path| update::install(&path, "--open")) {
                Ok(()) => set(UpdateState::Installing(rel.version.clone())),
                Err(e) => set(UpdateState::Failed(e)),
            }
        });
    }

    fn poll(&mut self, ctx: &egui::Context) {
        if self.reader.is_none() && self.reader_retry.elapsed() > Duration::from_secs(2) {
            self.reader = Reader::open();
            self.reader_retry = Instant::now();
        }
        if let Some(r) = &self.reader {
            if let Some(s) = r.snapshot() {
                self.snap = Some(s);
            }
            let page = r.take_page_request();
            if page != 0 {
                self.page = page;
                ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
            }
        }
        if self.slow_checked.elapsed() > Duration::from_secs(1) {
            self.slow_checked = Instant::now();
            // Pick up changes made from the taskbar menu.
            let m = config::modified();
            if m != self.cfg_mtime && self.cfg == self.saved {
                self.cfg = config::load();
                self.saved = self.cfg.clone();
                self.cfg_mtime = m;
            }
            self.motion_ok = sys::windows_animations_enabled();
        }
        let pal = palette_for(&self.cfg);
        if pal != self.pal {
            self.pal = pal;
            theme::apply(ctx, &pal);
        }
        if self.styled != Some(self.pal) {
            if let Some(h) = self.hwnd {
                style_title_bar(h, &self.pal);
            }
            self.styled = Some(self.pal);
        }
    }

    fn save_if_needed(&mut self, ctx: &egui::Context) {
        // Wait for a slider drag to finish so the meter isn't flooded with reloads.
        if self.cfg == self.saved || ctx.input(|i| i.pointer.any_down()) {
            return;
        }
        let autostart_changed = self.cfg.start_with_windows != self.saved.start_with_windows;
        match config::save(&self.cfg) {
            Ok(()) => {
                self.cfg_mtime = config::modified();
                self.saved = self.cfg.clone();
                if autostart_changed {
                    sys::set_autostart(self.cfg.start_with_windows);
                }
                monitor::notify_reload(false);
            }
            Err(e) => sys::log(&format!("saving settings failed: {e}")),
        }
    }

    fn rail(&mut self, ui: &mut egui::Ui) {
        let p = self.pal;
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(22.0, 22.0)));
            ui.add_space(2.0);
            ui.label(RichText::new("SpeedMeter").font(theme::display(19.0)).color(p.text));
        });
        ui.add_space(20.0);
        let icons = self.has_icons;
        for (page, glyph, label) in [
            (PAGE_LIVE, '\u{E9D9}', "Live"),
            (PAGE_HISTORY, '\u{E81C}', "History"),
            (PAGE_SETTINGS, '\u{E713}', "Settings"),
        ] {
            if ui::nav_item(ui, &p, icons.then_some(glyph), label, self.page == page) {
                self.page = page;
            }
            ui.add_space(2.0);
        }

        ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
            let running = self.snap.as_ref().is_some_and(|s| s.is_fresh());
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), egui::Sense::hover());
                ui.painter().circle_filled(r.center(), 3.5, if running { p.busy } else { p.heavy });
                let text = match &self.snap {
                    Some(s) if running => format!("Measuring {}", s.iface_name),
                    _ => "Not measuring".to_string(),
                };
                ui.add(egui::Label::new(RichText::new(text).font(theme::body(12.5)).color(p.dim)).truncate());
            });
            ui.add_space(8.0);
            let state = self.update.lock().map(|s| s.clone()).unwrap_or_default();
            match state {
                UpdateState::Available(r) => {
                    if ui::button(ui, &p, &format!("Update to {}", r.version), true).clicked() {
                        self.start_update(r);
                    }
                }
                UpdateState::Downloading { got, total, .. } => {
                    let pct = if total > 0 { got * 100 / total } else { 0 };
                    ui.label(RichText::new(format!("Downloading update, {pct}%")).font(theme::body(12.5)).color(p.dim));
                }
                UpdateState::Installing(_) => {
                    ui.label(RichText::new("Installing update").font(theme::body(12.5)).color(p.dim));
                }
                _ => {}
            }
        });
    }
}

/// The Updates rows on the Settings page.
pub fn update_rows(app: &mut App, ui: &mut egui::Ui) {
    let p = app.pal;
    let state = app.update.lock().map(|s| s.clone()).unwrap_or_default();
    let current = update::current();
    let (title, desc) = match &state {
        UpdateState::Idle => (format!("SpeedMeter {current}"), "New versions are published on GitHub.".to_string()),
        UpdateState::Checking => ("Checking for updates".to_string(), String::new()),
        UpdateState::UpToDate => (format!("You're up to date ({current})"), String::new()),
        UpdateState::Available(r) => (format!("Version {} is available", r.version), format!("You have {current}.")),
        UpdateState::Downloading { version, got, total } => {
            (format!("Downloading {version}"), format!("{} of {}", format::bytes(*got), format::bytes(*total)))
        }
        UpdateState::Installing(v) => (format!("Installing {v}"), "SpeedMeter closes and reopens in a moment.".to_string()),
        UpdateState::Failed(e) => ("Couldn't update".to_string(), e.clone()),
    };
    let desc = (!desc.is_empty()).then_some(desc);
    ui::setting_row(ui, &p, &title, desc.as_deref(), |ui| match &state {
        UpdateState::Available(r) => {
            if ui::button(ui, &p, "Update now", true).clicked() {
                app.start_update(r.clone());
            }
        }
        UpdateState::Checking | UpdateState::Downloading { .. } | UpdateState::Installing(_) => {
            ui.add(egui::Spinner::new().color(p.accent));
        }
        _ => {
            if ui::button(ui, &p, "Check for updates", false).clicked() {
                app.check_updates(true);
            }
        }
    });
    ui::setting_row(
        ui,
        &p,
        "Install updates automatically",
        Some("Downloads new versions in the background and installs them quietly. Applies to the installed app."),
        |ui| {
            ui::toggle(ui, &p, &mut app.cfg.auto_update);
        },
    );
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().clamp(0.0, 0.1);
        self.last_frame = now;

        let latest = self.snap.as_ref().filter(|s| s.is_fresh()).and_then(|s| s.latest());
        let (down, up) = latest.map(|s| (s.down as f64, s.up as f64)).unwrap_or((0.0, 0.0));
        let ink = theme::to_rgb(self.pal.text);
        let mut moving = false;
        if self.animating() {
            self.down.target = down;
            self.up.target = up;
            moving |= self.down.step(dt, &self.cfg.heat, self.pal.dark, ink);
            moving |= self.up.step(dt, &self.cfg.heat, self.pal.dark, ink);
        } else {
            self.down.snap(down, &self.cfg.heat, self.pal.dark, ink);
            self.up.snap(up, &self.cfg.heat, self.pal.dark, ink);
        }

        let p = self.pal;
        egui::SidePanel::left("rail")
            .exact_width(212.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(p.base).inner_margin(Margin { left: 10, right: 12, top: 14, bottom: 14 }))
            .show(ctx, |ui| self.rail(ui));
        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(p.content)
                    .stroke(Stroke::new(1.0_f32, p.edge))
                    .corner_radius(CornerRadius { nw: 8, ne: 0, sw: 0, se: 0 })
                    .inner_margin(Margin { left: 32, right: 32, top: 24, bottom: 12 }),
            )
            .show(ctx, |ui| {
                self.view_h = ui.available_height();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| match self.page {
                    PAGE_HISTORY => history_view::show(self, ui),
                    PAGE_SETTINGS => settings::show(self, ui),
                    _ => live::show(self, ui),
                });
            });
        self.save_if_needed(ctx);

        let (focused, minimized) = ctx.input(|i| (i.viewport().focused.unwrap_or(true), i.viewport().minimized.unwrap_or(false)));
        if minimized {
            ctx.request_repaint_after(Duration::from_secs(1));
        } else if self.animating() && self.page == PAGE_LIVE {
            // The live chart glides continuously.
            if focused {
                ctx.request_repaint();
            } else {
                ctx.request_repaint_after(Duration::from_millis(33));
            }
        } else if moving {
            ctx.request_repaint();
        } else {
            ctx.request_repaint_after(Duration::from_millis(400));
        }
    }
}

/// Opens the dashboard, or focuses the one that's already open.
pub fn run(page: u32) {
    let _single = unsafe { CreateMutexW(None, true, w!("Local\\SpeedMeter.Dashboard")) };
    if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
        if let Some(h) = sys::find_own_window(TITLE) {
            if let Some(r) = Reader::open() {
                r.request_page(page);
            }
            sys::bring_to_front(h);
        }
        return;
    }
    let icon = egui::IconData { rgba: ICON_RGBA.to_vec(), width: 64, height: 64 };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1120.0, 780.0])
            .with_min_inner_size([880.0, 600.0])
            .with_icon(icon),
        centered: true,
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(TITLE, options, Box::new(move |cc| Ok(Box::new(App::new(cc, page))))) {
        sys::log(&format!("dashboard failed to start: {e}"));
    }
}
