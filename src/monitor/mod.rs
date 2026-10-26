//! The always-running background part: samples the network once a second, draws the
//! taskbar widget, records history, and publishes live data for the dashboard.
//!
//! Everything runs on one UI thread. Database writes go to a separate thread so a
//! slow disk can never freeze the widget (which shares an input queue with the taskbar).

mod layout;
mod render;
mod taskbar;
mod tray;
mod widget;

use crate::config::{self, Config, Connection, Placement};
use crate::format::{self, UnitMode};
use crate::history::{self, MinuteRecord, WriteMsg};
use crate::net::{self, Reading, Sampler};
use crate::shared::{self, Sample, PAGE_HISTORY, PAGE_LIVE, PAGE_SETTINGS};
use crate::sys::{self, wide};
use chrono::Local;
use crate::anim::Track;
use render::{Renderer, Scene};

/// The taskbar's text color: white on a dark taskbar, black on a light one.
fn ink(dark: bool) -> [u8; 3] {
    if dark {
        [255, 255, 255]
    } else {
        [0, 0, 0]
    }
}
use std::cell::RefCell;
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc;
use std::time::Duration;
use tray::Tray;
use widget::Widget;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GetLastError, ERROR_ALREADY_EXISTS, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::Controls::{InitCommonControlsEx, ICC_WIN95_CLASSES, INITCOMMONCONTROLSEX};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Shell::{
    SHQueryUserNotificationState, QUNS_BUSY, QUNS_PRESENTATION_MODE, QUNS_RUNNING_D3D_FULL_SCREEN,
};
use windows::Win32::UI::WindowsAndMessaging::*;

pub const MAIN_CLASS: PCWSTR = w!("SpeedMeter.Monitor");
pub const WM_APP_TRAY: u32 = WM_APP + 1;
/// Sent by the dashboard after it saves settings. wParam 1 = history changed too.
pub const WM_APP_RELOAD: u32 = WM_APP + 2;
pub const WM_APP_QUIT: u32 = WM_APP + 3;
pub const WM_APP_WIDGET: u32 = WM_APP + 4;
const TIMER_TICK: usize = 1;
const TIMER_ANIM: usize = 2;
const NIN_SELECT: u32 = WM_USER;
const NIN_KEYSELECT: u32 = WM_USER + 1;

pub static MAIN_HWND: AtomicIsize = AtomicIsize::new(0);
static TASKBAR_CREATED: AtomicU32 = AtomicU32::new(0);

thread_local! {
    static STATE: RefCell<Option<Monitor>> = const { RefCell::new(None) };
}

/// Runs `f` on the monitor state unless it is already borrowed further up the stack
/// (a re-entrant window message), in which case the message is simply skipped.
fn with<R>(f: impl FnOnce(&mut Monitor) -> R) -> Option<R> {
    STATE.with(|s| s.try_borrow_mut().ok().and_then(|mut g| g.as_mut().map(f)))
}

pub fn main_window() -> Option<HWND> {
    unsafe { FindWindowW(MAIN_CLASS, None).ok().filter(|h| !h.is_invalid()) }
}

pub fn is_running() -> bool {
    main_window().is_some()
}

/// Tells a running monitor to re-read config.json (and history totals if `history`).
pub fn notify_reload(history: bool) {
    if let Some(h) = main_window() {
        unsafe {
            let _ = PostMessageW(h, WM_APP_RELOAD, WPARAM(history as usize), LPARAM(0));
        }
    }
}

pub fn request_quit() {
    if let Some(h) = main_window() {
        unsafe {
            let _ = PostMessageW(h, WM_APP_QUIT, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn start_in_background() {
    let _ = std::process::Command::new(sys::exe_path()).arg("--background").spawn();
}

struct Totals {
    day: String,
    month: String,
    today: (u64, u64),
    this_month: (u64, u64),
}


#[derive(Clone, Copy)]
struct Frame {
    floating: bool,
    scale: f32,
    w: i32,
    h: i32,
}

/// Synthetic traffic for trying out the visuals (set SPEEDMETER_DEMO=1). It sweeps
/// from a trickle to 80 MB/s and back every 24 seconds and is never recorded.
fn demo_reading(tick: u64) -> Reading {
    let t = (tick % 24) as f64;
    let down = if t < 14.0 {
        4.0e3 * (80.0e6f64 / 4.0e3).powf(t / 13.0)
    } else if t < 18.0 {
        80.0e6 * (1.0 + 0.08 * (t * 2.7).sin())
    } else {
        20.0e3 * (1.0 + 0.5 * (t * 1.3).sin())
    };
    let up = down * 0.06 * (1.0 + 0.3 * (t * 0.9).cos());
    Reading { down, up, rx: down as u64, tx: up as u64 }
}

struct Monitor {
    hwnd: HWND,
    cfg: Config,
    sampler: Sampler,
    writer: mpsc::Sender<WriteMsg>,
    live: Option<shared::Writer>,
    renderer: Option<Renderer>,
    widget: Option<Widget>,
    floating_needs_position: bool,
    layout: layout::Shared,
    tray: Tray,
    hover: bool,
    hover_t: f32,
    dark: bool,
    down: Track,
    up: Track,
    frame: Option<Frame>,
    anim_running: bool,
    last_frame: std::time::Instant,
    motion_ok: bool,
    demo: bool,
    last: Reading,
    bucket: MinuteRecord,
    totals: Totals,
    ticks: u64,
    label: (String, String),
    menu_adapters: Vec<(String, String)>,
    logged: [bool; 3],
}

// Menu command ids
const ID_OPEN: u32 = 1;
const ID_HISTORY: u32 = 2;
const ID_SETTINGS: u32 = 3;
const ID_EXIT: u32 = 9;
const ID_NET_AUTO: u32 = 100;
const ID_NET_ALL: u32 = 101;
const ID_NET_REFRESH: u32 = 102;
const ID_NET_BASE: u32 = 110;
const ID_POS_RIGHT: u32 = 200;
const ID_POS_LEFT: u32 = 201;
const ID_POS_FLOAT: u32 = 202;
const ID_UNIT_BYTES: u32 = 300;
const ID_UNIT_BITS: u32 = 301;
const ID_HEAT: u32 = 400;
const ID_AUTOSTART: u32 = 401;
const ID_ANIM: u32 = 402;

fn new_bucket(now: chrono::DateTime<Local>) -> MinuteRecord {
    MinuteRecord {
        ts: now.timestamp() / 60 * 60,
        day: now.format("%Y-%m-%d").to_string(),
        rx: 0,
        tx: 0,
        peak_rx: 0.0,
        peak_tx: 0.0,
    }
}

fn fullscreen_app_running() -> bool {
    match unsafe { SHQueryUserNotificationState() } {
        Ok(s) => s == QUNS_BUSY || s == QUNS_RUNNING_D3D_FULL_SCREEN || s == QUNS_PRESENTATION_MODE,
        Err(_) => false,
    }
}

fn work_area() -> RECT {
    let mut r = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut RECT as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    r
}

fn clamp_to_screen(x: i32, y: i32, w: i32, h: i32) -> (i32, i32) {
    unsafe {
        let vx = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let vy = GetSystemMetrics(SM_YVIRTUALSCREEN);
        let vw = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let vh = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        (x.clamp(vx, (vx + vw - w).max(vx)), y.clamp(vy, (vy + vh - h).max(vy)))
    }
}

impl Monitor {
    fn new(hwnd: HWND, cfg: Config) -> Self {
        let renderer = match Renderer::new() {
            Ok(r) => Some(r),
            Err(e) => {
                sys::log(&format!("renderer init failed: {e}"));
                None
            }
        };
        let now = Local::now();
        let writer = history::spawn_writer(config::db_path());
        let _ = writer.send(WriteMsg::Prune { keep_days: cfg.keep_minutes_days });
        let mut m = Self {
            hwnd,
            sampler: Sampler::new(),
            writer,
            live: shared::Writer::create(),
            renderer,
            widget: None,
            floating_needs_position: true,
            layout: layout::spawn(),
            tray: Tray::new(hwnd),
            hover: false,
            hover_t: 0.0,
            dark: sys::taskbar_is_dark(),
            down: Track::default(),
            up: Track::default(),
            frame: None,
            anim_running: false,
            last_frame: std::time::Instant::now(),
            motion_ok: sys::windows_animations_enabled(),
            demo: std::env::var_os("SPEEDMETER_DEMO").is_some(),
            last: Reading::default(),
            bucket: new_bucket(now),
            totals: Totals { day: String::new(), month: String::new(), today: (0, 0), this_month: (0, 0) },
            ticks: 0,
            label: (String::new(), String::new()),
            menu_adapters: Vec::new(),
            logged: [false; 3],
            cfg,
        };
        m.load_totals();
        m.tray.set_visible(m.cfg.show_tray_icon);
        sys::set_menu_theme(sys::apps_are_dark());
        m
    }

    /// Today's and this month's totals = what's on disk + the minute still in memory.
    fn load_totals(&mut self) {
        let now = Local::now();
        let day = now.format("%Y-%m-%d").to_string();
        let month = now.format("%Y-%m").to_string();
        let (mut today, mut this_month) = ((0, 0), (0, 0));
        if let Ok(c) = history::open(&config::db_path()) {
            today = history::day_totals(&c, &day);
            this_month = history::month_totals(&c, &month);
        }
        let pending = (self.bucket.rx, self.bucket.tx);
        self.totals = Totals {
            day,
            month,
            today: (today.0 + pending.0, today.1 + pending.1),
            this_month: (this_month.0 + pending.0, this_month.1 + pending.1),
        };
    }

    fn flush_bucket(&mut self) {
        if self.bucket.rx + self.bucket.tx > 0 {
            let _ = self.writer.send(WriteMsg::Minute(self.bucket.clone()));
        }
        self.bucket.rx = 0;
        self.bucket.tx = 0;
        self.bucket.peak_rx = 0.0;
        self.bucket.peak_tx = 0.0;
    }

    fn wait_for_writer(&self) {
        let (tx, rx) = mpsc::channel();
        if self.writer.send(WriteMsg::Flush(tx)).is_ok() {
            let _ = rx.recv_timeout(Duration::from_secs(3));
        }
    }

    fn account(&mut self, r: &Reading) {
        let now = Local::now();
        let minute = now.timestamp() / 60 * 60;
        if self.bucket.ts != minute {
            self.flush_bucket();
            self.bucket = new_bucket(now);
        }
        let day = &self.bucket.day;
        if *day != self.totals.day {
            if day[..7] != self.totals.month {
                self.totals.month = day[..7].to_string();
                self.totals.this_month = (0, 0);
            }
            self.totals.day = day.clone();
            self.totals.today = (0, 0);
        }
        let b = &mut self.bucket;
        b.rx += r.rx;
        b.tx += r.tx;
        b.peak_rx = b.peak_rx.max(r.down);
        b.peak_tx = b.peak_tx.max(r.up);
        self.totals.today.0 += r.rx;
        self.totals.today.1 += r.tx;
        self.totals.this_month.0 += r.rx;
        self.totals.this_month.1 += r.tx;
    }

    fn tick(&mut self) {
        self.ticks += 1;
        let real = self.sampler.sample(&self.cfg.connection);
        let r = if self.demo { demo_reading(self.ticks) } else { real };
        self.last = r;
        if !self.demo {
            self.account(&r);
        }
        let active = self.sampler.active();
        self.label = net::selection_label(&self.cfg.connection, &active);

        if let Some(live) = self.live.as_mut() {
            let t = &self.totals;
            live.publish(
                Sample { t: chrono::Utc::now().timestamp_millis(), down: r.down as f32, up: r.up as f32 },
                [t.today.0, t.today.1, t.this_month.0, t.this_month.1],
                &self.label.0,
                &self.label.1,
            );
        }

        if self.ticks % 5 == 0 {
            self.dark = sys::taskbar_is_dark();
            self.motion_ok = sys::windows_animations_enabled();
        }
        if self.ticks % (6 * 3600) == 0 {
            let _ = self.writer.send(WriteMsg::Prune { keep_days: self.cfg.keep_minutes_days });
        }
        if self.animating() {
            self.down.target = r.down;
            self.up.target = r.up;
            self.layout_widget();
            self.start_animation();
        } else {
            self.down.snap(r.down, &self.cfg.heat, self.dark, ink(self.dark));
            self.up.snap(r.up, &self.cfg.heat, self.dark, ink(self.dark));
            self.render();
        }

        let down = format::speed(r.down, self.cfg.unit, self.cfg.decimals);
        let up = format::speed(r.up, self.cfg.unit, self.cfg.decimals);
        self.tray.set_tip(&format!("SpeedMeter: {}\nDownload {down}\nUpload {up}", self.label.0));
    }

    fn tooltip_text(&self) -> String {
        let (name, desc) = &self.label;
        let t = &self.totals;
        let head = if desc.is_empty() || desc == name { name.clone() } else { format!("{name} ({desc})") };
        format!(
            "{head}\nToday: {} down, {} up\nThis month: {} down, {} up\nDouble-click to open SpeedMeter",
            format::bytes(t.today.0),
            format::bytes(t.today.1),
            format::bytes(t.this_month.0),
            format::bytes(t.this_month.1),
        )
    }

    fn animating(&self) -> bool {
        self.cfg.animations && self.motion_ok
    }

    fn start_animation(&mut self) {
        if !self.anim_running {
            self.anim_running = true;
            self.last_frame = std::time::Instant::now();
            unsafe { SetTimer(self.hwnd, TIMER_ANIM, 15, None) };
        }
    }

    fn stop_animation(&mut self) {
        if self.anim_running {
            self.anim_running = false;
            unsafe {
                let _ = KillTimer(self.hwnd, TIMER_ANIM);
            }
        }
    }

    /// One animation frame (~60 per second while anything moves).
    fn animate(&mut self) {
        let now = std::time::Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().clamp(0.0, 0.05);
        self.last_frame = now;
        let heat = self.cfg.heat.clone();
        let mut moving = self.down.step(dt, &heat, self.dark, ink(self.dark));
        moving |= self.up.step(dt, &heat, self.dark, ink(self.dark));
        let goal = if self.hover { 1.0 } else { 0.0 };
        if (self.hover_t - goal).abs() > 0.002 {
            self.hover_t += (goal - self.hover_t) * (1.0 - (-dt / 0.08).exp());
            moving = true;
        } else {
            self.hover_t = goal;
        }
        self.paint();
        if !moving {
            self.stop_animation();
        }
    }

    /// Full update: find the taskbar, (re)create and place the widget, then draw.
    fn render(&mut self) {
        self.layout_widget();
        self.paint();
    }

    fn layout_widget(&mut self) {
        let floating = self.cfg.placement == Placement::Floating;
        let tb = if floating { None } else { taskbar::find() };

        let stale = match &self.widget {
            None => true,
            Some(w) => {
                !w.alive() || w.floating != floating || (!floating && Some(w.parent) != tb.as_ref().map(|t| t.hwnd))
            }
        };
        if stale {
            self.widget = None;
            self.widget = if floating {
                self.floating_needs_position = true;
                Widget::create(true, None, self.hwnd)
            } else {
                tb.as_ref().and_then(|t| Widget::create(false, Some(t.hwnd), self.hwnd))
            };
        }
        let tip = self.tooltip_text();
        let (Some(widget), Some(renderer)) = (self.widget.as_mut(), self.renderer.as_mut()) else { return };

        let (scale, height) = if floating {
            let dpi = unsafe { GetDpiForWindow(widget.hwnd) }.max(96);
            (dpi as f32 / 96.0, None)
        } else {
            match &tb {
                Some(t) => (t.scale(), Some(t.height())),
                None => return,
            }
        };
        let (w, h) = match renderer.measure(&self.cfg, floating, scale, height) {
            Ok(s) => s,
            Err(e) => {
                if !self.logged[0] {
                    sys::log(&format!("measure failed: {e}"));
                    self.logged[0] = true;
                }
                return;
            }
        };
        if floating {
            if self.floating_needs_position {
                let (x, y) = self.cfg.floating_pos.unwrap_or_else(|| {
                    let wa = work_area();
                    (wa.right - w - 24, wa.bottom - h - 24)
                });
                let (x, y) = clamp_to_screen(x, y, w, h);
                widget.place_floating(x, y, w, h);
                self.floating_needs_position = false;
            } else {
                widget.resize_floating(w, h);
            }
            widget.set_visible(!fullscreen_app_running());
        } else if let Some(t) = &tb {
            let layout = self.layout.lock().ok().and_then(|g| g.clone());
            widget.place_docked(t.slot(self.cfg.placement, w, self.cfg.offset, layout.as_ref()));
        }
        widget.set_tooltip(&tip);
        widget.set_dark(sys::apps_are_dark());
        self.frame = Some(Frame { floating, scale, w, h });
    }

    /// Draws the current animation state into the widget (no layout work).
    fn paint(&mut self) {
        if !self.animating() {
            self.hover_t = if self.hover { 1.0 } else { 0.0 };
        }
        let (Some(widget), Some(renderer), Some(f)) = (self.widget.as_ref(), self.renderer.as_mut(), self.frame) else {
            return;
        };
        let scene = Scene {
            down: self.down.row,
            up: self.up.row,
            cfg: &self.cfg,
            dark: self.dark,
            floating: f.floating,
            hover: self.hover_t,
            scale: f.scale,
        };
        match renderer.draw(&scene, f.w, f.h) {
            Ok(dc) => {
                if let Err(e) = widget.present(dc, f.w, f.h) {
                    if !self.logged[1] {
                        sys::log(&format!("UpdateLayeredWindow failed: {e}"));
                        self.logged[1] = true;
                    }
                }
            }
            Err(e) => {
                if !self.logged[2] {
                    sys::log(&format!("draw failed: {e}"));
                    self.logged[2] = true;
                }
            }
        }
    }

    fn open(&mut self, page: u32) {
        if let Some(h) = sys::find_own_window(crate::dashboard::TITLE) {
            if let Some(l) = self.live.as_mut() {
                l.request_page(page);
            }
            sys::bring_to_front(h);
        } else {
            let _ = std::process::Command::new(sys::exe_path())
                .args(["--dashboard", "--page", shared::page_name(page)])
                .spawn();
        }
    }

    fn set_config(&mut self, new: Config) {
        let old = std::mem::replace(&mut self.cfg, new);
        if let Err(e) = config::save(&self.cfg) {
            sys::log(&format!("saving config failed: {e}"));
        }
        self.apply(&old);
    }

    fn reload(&mut self, history_changed: bool) {
        let new = config::load();
        let old = std::mem::replace(&mut self.cfg, new);
        self.apply(&old);
        if history_changed {
            self.wait_for_writer();
            self.load_totals();
        }
    }

    fn apply(&mut self, old: &Config) {
        if old.placement != self.cfg.placement {
            self.widget = None;
        }
        if !self.animating() {
            self.stop_animation();
            self.down.snap(self.down.target, &self.cfg.heat, self.dark, ink(self.dark));
            self.up.snap(self.up.target, &self.cfg.heat, self.dark, ink(self.dark));
        }
        if old.connection != self.cfg.connection {
            self.sampler.force_refresh();
        }
        if old.floating_pos != self.cfg.floating_pos {
            self.floating_needs_position = true;
        }
        if old.keep_minutes_days != self.cfg.keep_minutes_days {
            let _ = self.writer.send(WriteMsg::Prune { keep_days: self.cfg.keep_minutes_days });
        }
        self.tray.set_visible(self.cfg.show_tray_icon);
        sys::set_autostart(self.cfg.start_with_windows);
        self.render();
    }

    fn build_menu(&mut self) -> windows::core::Result<HMENU> {
        unsafe {
            let add = |menu: HMENU, id: u32, text: &str, checked: bool, radio: bool| -> windows::core::Result<()> {
                let s = wide(text);
                AppendMenuW(menu, MF_STRING, id as usize, PCWSTR(s.as_ptr()))?;
                if checked {
                    if radio {
                        let _ = CheckMenuRadioItem(menu, id, id, id, MF_BYCOMMAND.0);
                    } else {
                        let _ = CheckMenuItem(menu, id, (MF_BYCOMMAND | MF_CHECKED).0);
                    }
                }
                Ok(())
            };
            let sub = |menu: HMENU, child: HMENU, text: &str| -> windows::core::Result<()> {
                let s = wide(text);
                AppendMenuW(menu, MF_POPUP, child.0 as usize, PCWSTR(s.as_ptr()))
            };

            let menu = CreatePopupMenu()?;
            add(menu, ID_OPEN, "Open SpeedMeter", false, false)?;
            add(menu, ID_HISTORY, "Traffic history", false, false)?;
            add(menu, ID_SETTINGS, "Settings", false, false)?;
            let _ = SetMenuDefaultItem(menu, ID_OPEN, 0);
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;

            let net_menu = CreatePopupMenu()?;
            let auto_label = match (&self.cfg.connection, self.sampler.active().first()) {
                (Connection::Auto, Some(a)) => format!("Automatic (now {})", a.name),
                _ => "Automatic".to_string(),
            };
            add(net_menu, ID_NET_AUTO, &auto_label, self.cfg.connection == Connection::Auto, true)?;
            add(net_menu, ID_NET_ALL, "All networks combined", self.cfg.connection == Connection::All, true)?;
            AppendMenuW(net_menu, MF_SEPARATOR, 0, None)?;
            self.menu_adapters.clear();
            for (i, a) in self.sampler.adapters().iter().enumerate() {
                let selected = matches!(&self.cfg.connection, Connection::Adapter { id, .. } if *id == a.id);
                let state = if a.up { "" } else { ", disconnected" };
                add(net_menu, ID_NET_BASE + i as u32, &format!("{} ({}{state})", a.name, a.description), selected, true)?;
                self.menu_adapters.push((a.id.clone(), a.name.clone()));
            }
            AppendMenuW(net_menu, MF_SEPARATOR, 0, None)?;
            add(net_menu, ID_NET_REFRESH, "Refresh list", false, false)?;
            sub(menu, net_menu, "Network")?;

            let pos = CreatePopupMenu()?;
            add(pos, ID_POS_RIGHT, "Next to the tray icons", self.cfg.placement == Placement::TaskbarRight, true)?;
            add(pos, ID_POS_LEFT, "Left side of the taskbar", self.cfg.placement == Placement::TaskbarLeft, true)?;
            add(pos, ID_POS_FLOAT, "Floating window", self.cfg.placement == Placement::Floating, true)?;
            sub(menu, pos, "Widget position")?;

            let units = CreatePopupMenu()?;
            add(units, ID_UNIT_BYTES, "Bytes (KB/s, MB/s)", self.cfg.unit == UnitMode::Bytes, true)?;
            add(units, ID_UNIT_BITS, "Bits (Kbps, Mbps)", self.cfg.unit == UnitMode::Bits, true)?;
            sub(menu, units, "Speed units")?;

            add(menu, ID_HEAT, "Color speeds when busy", self.cfg.heat.enabled, false)?;
            add(menu, ID_ANIM, "Animations", self.cfg.animations, false)?;
            add(menu, ID_AUTOSTART, "Start with Windows", self.cfg.start_with_windows, false)?;
            AppendMenuW(menu, MF_SEPARATOR, 0, None)?;
            add(menu, ID_EXIT, "Exit SpeedMeter", false, false)?;
            Ok(menu)
        }
    }

    fn command(&mut self, id: u32) {
        let mut c = self.cfg.clone();
        match id {
            ID_OPEN => return self.open(PAGE_LIVE),
            ID_HISTORY => return self.open(PAGE_HISTORY),
            ID_SETTINGS => return self.open(PAGE_SETTINGS),
            ID_EXIT => {
                unsafe {
                    let _ = PostMessageW(self.hwnd, WM_APP_QUIT, WPARAM(0), LPARAM(0));
                }
                return;
            }
            ID_NET_AUTO => c.connection = Connection::Auto,
            ID_NET_ALL => c.connection = Connection::All,
            ID_NET_REFRESH => {
                self.sampler.force_refresh();
                return;
            }
            ID_POS_RIGHT => c.placement = Placement::TaskbarRight,
            ID_POS_LEFT => c.placement = Placement::TaskbarLeft,
            ID_POS_FLOAT => c.placement = Placement::Floating,
            ID_UNIT_BYTES => c.unit = UnitMode::Bytes,
            ID_UNIT_BITS => c.unit = UnitMode::Bits,
            ID_HEAT => c.heat.enabled = !c.heat.enabled,
            ID_ANIM => c.animations = !c.animations,
            ID_AUTOSTART => c.start_with_windows = !c.start_with_windows,
            n if n >= ID_NET_BASE => {
                let Some((id, name)) = self.menu_adapters.get((n - ID_NET_BASE) as usize).cloned() else { return };
                c.connection = Connection::Adapter { id, name };
            }
            _ => return,
        }
        if c != self.cfg {
            self.set_config(c);
        }
    }

    fn widget_moved(&mut self) {
        if let Some(w) = &self.widget {
            if w.floating {
                let r = w.screen_rect();
                let mut c = self.cfg.clone();
                c.floating_pos = Some((r.left, r.top));
                self.cfg = c;
                let _ = config::save(&self.cfg);
            }
        }
    }

    fn shutdown(&mut self) {
        self.flush_bucket();
        self.wait_for_writer();
        self.widget = None;
        self.tray.set_visible(false);
    }
}

fn show_menu(hwnd: HWND) {
    let Some(Ok(menu)) = with(|m| m.build_menu()) else { return };
    unsafe {
        let mut p = POINT::default();
        let _ = GetCursorPos(&mut p);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(menu, TPM_RETURNCMD | TPM_RIGHTBUTTON | TPM_BOTTOMALIGN, p.x, p.y, 0, hwnd, None);
        let _ = PostMessageW(hwnd, WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        if cmd.0 > 0 {
            with(|m| m.command(cmd.0 as u32));
        }
    }
}

unsafe extern "system" fn main_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER if wp.0 == TIMER_TICK => {
            with(|m| m.tick());
            LRESULT(0)
        }
        WM_TIMER if wp.0 == TIMER_ANIM => {
            with(|m| m.animate());
            LRESULT(0)
        }
        WM_APP_TRAY => {
            match (lp.0 & 0xFFFF) as u32 {
                NIN_SELECT | NIN_KEYSELECT => {
                    with(|m| m.open(PAGE_LIVE));
                }
                WM_CONTEXTMENU => show_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_WIDGET => {
            match wp.0 {
                widget::EV_HOVER_ON | widget::EV_HOVER_OFF => {
                    with(|m| {
                        m.hover = wp.0 == widget::EV_HOVER_ON;
                        if m.animating() {
                            m.start_animation();
                        } else {
                            m.paint();
                        }
                    });
                }
                widget::EV_DOUBLE_CLICK => {
                    with(|m| m.open(PAGE_LIVE));
                }
                widget::EV_RIGHT_CLICK => show_menu(hwnd),
                widget::EV_MOVED => {
                    with(|m| m.widget_moved());
                }
                _ => {}
            }
            LRESULT(0)
        }
        WM_APP_RELOAD => {
            with(|m| m.reload(wp.0 == 1));
            LRESULT(0)
        }
        WM_APP_QUIT | WM_CLOSE => {
            with(|m| m.shutdown());
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_QUERYENDSESSION => LRESULT(1),
        WM_ENDSESSION => {
            if wp.0 != 0 {
                with(|m| m.shutdown());
            }
            LRESULT(0)
        }
        WM_SETTINGCHANGE | WM_DISPLAYCHANGE | WM_DPICHANGED => {
            with(|m| {
                m.dark = sys::taskbar_is_dark();
                sys::set_menu_theme(sys::apps_are_dark());
                m.floating_needs_position = true;
                m.render();
            });
            DefWindowProcW(hwnd, msg, wp, lp)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        m if m != 0 && m == TASKBAR_CREATED.load(Ordering::Relaxed) => {
            with(|m| {
                m.widget = None;
                m.tray.recreate(m.cfg.show_tray_icon);
                m.render();
            });
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Runs the monitor until the user exits. Returns immediately if one is already running.
pub fn run(open_dashboard: bool) {
    unsafe {
        let _mutex = CreateMutexW(None, true, w!("Local\\SpeedMeter.Monitor"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        let icc = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_WIN95_CLASSES,
        };
        let _ = InitCommonControlsEx(&icc);

        let hinst = GetModuleHandleW(None).unwrap_or_default();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(main_proc),
            hInstance: hinst.into(),
            lpszClassName: MAIN_CLASS,
            ..Default::default()
        };
        RegisterClassExW(&wc);
        widget::register_class();
        TASKBAR_CREATED.store(RegisterWindowMessageW(w!("TaskbarCreated")), Ordering::Relaxed);

        // A real (hidden) top-level window, not message-only, so it receives the
        // TaskbarCreated and WM_SETTINGCHANGE broadcasts.
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_TOOLWINDOW,
            MAIN_CLASS,
            w!("SpeedMeter monitor"),
            WS_POPUP,
            0,
            0,
            0,
            0,
            None,
            None,
            hinst,
            None,
        ) else {
            sys::log("could not create the monitor window");
            return;
        };
        MAIN_HWND.store(hwnd.0 as isize, Ordering::Relaxed);

        let first_run = !config::exists();
        let cfg = config::load();
        if first_run {
            let _ = config::save(&cfg);
        }
        sys::set_autostart(cfg.start_with_windows);

        STATE.with(|s| *s.borrow_mut() = Some(Monitor::new(hwnd, cfg)));
        let main = hwnd.0 as isize;
        crate::update::spawn_auto_updater(move || {
            let _ = PostMessageW(HWND(main as *mut _), WM_APP_QUIT, WPARAM(0), LPARAM(0));
        });
        SetTimer(hwnd, TIMER_TICK, 1000, None);
        with(|m| m.tick());

        if open_dashboard {
            with(|m| m.open(PAGE_LIVE));
        }

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        STATE.with(|s| *s.borrow_mut() = None);
    }
}
