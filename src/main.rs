//! SpeedMeter: a network speed meter for the Windows 11 taskbar.
//!
//! One executable, two roles:
//! * the monitor (default / `--background`): lightweight, always running, draws the
//!   taskbar meter and records history;
//! * the dashboard (`--dashboard`): the window with live charts, history and settings.
//!   It runs as its own process so its memory is freed when you close it.
//!
//! Other flags: `--open` (start if needed and show the window, used after an update),
//! `--quit` (close every running SpeedMeter cleanly, used by the installer),
//! `--page live|history|settings`.

#![windows_subsystem = "windows"]

mod anim;
mod config;
mod dashboard;
mod format;
mod heat;
mod history;
mod monitor;
mod net;
mod shared;
mod sys;
mod update;

use std::time::Duration;

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

/// Asks the monitor and the dashboard to close and waits (up to 6 s) until they have.
/// The monitor saves the minute in progress before it exits.
fn quit_all() {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_CLOSE};
    monitor::request_quit();
    if let Some(h) = sys::find_own_window(dashboard::TITLE) {
        unsafe {
            let _ = PostMessageW(h, WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    for _ in 0..60 {
        if !monitor::is_running() && sys::find_own_window(dashboard::TITLE).is_none() {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        sys::log(&format!("panic: {info}"));
    }));

    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let page = shared::page_from_name(&arg_value(&args, "--page").unwrap_or_default());

    if has("--quit") {
        quit_all();
        return;
    }
    if has("--dashboard") {
        dashboard::run(page);
        return;
    }
    if monitor::is_running() {
        // Launched again by hand (Start menu, Explorer): show the window instead.
        if !has("--background") {
            dashboard::run(page);
        }
        return;
    }
    // No flags, or --open: start measuring and show the window.
    monitor::run(!has("--background"));
}
