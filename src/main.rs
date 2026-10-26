//! SpeedMeter: a network speed meter for the Windows 11 taskbar.
//!
//! One executable, two roles:
//! * the monitor (default / `--background`): lightweight, always running, draws the
//!   taskbar widget and records history;
//! * the dashboard (`--dashboard`): the window with live charts, history and settings.
//!   It runs as its own process so its memory is freed when you close it.

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

fn arg_value(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() {
    std::panic::set_hook(Box::new(|info| {
        sys::log(&format!("panic: {info}"));
    }));

    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);
    let page = shared::page_from_name(&arg_value(&args, "--page").unwrap_or_default());

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
    monitor::run(!has("--background"));
}
