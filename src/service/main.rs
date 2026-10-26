//! speedmeter-service.exe: counts how much data each app sends and receives, for
//! SpeedMeter's per-app history and live list.
//!
//! Seeing which process sent which bytes takes Windows event tracing, which only
//! administrators and service accounts may start. So this small program runs as a
//! service under Local Service (not an administrator account, and with all but two
//! of its privileges removed), installed in Program Files where only administrators
//! can change it. It publishes running byte totals per process into a shared table
//! that signed-in users can read but not write, and accepts no commands at all.
//!
//!   speedmeter-service --install     copy to Program Files, register, start (elevated)
//!   speedmeter-service --uninstall   stop and remove (elevated)
//!   speedmeter-service --run <secs> <log>   count in the foreground and write a
//!                                    diagnostic log (elevated; for development)

#![windows_subsystem = "windows"]

#[path = "../appnet.rs"]
mod appnet;
mod counters;
mod etw;
mod scm;

use counters::Publisher;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Counts until `stop` is set: keeps a trace session running (restarting it if it
/// fails) and republishes the totals twice a second.
pub fn run(stop: &AtomicBool, mut log: Option<std::fs::File>) {
    let logging = log.is_some();
    let mut note = |line: &str| {
        if let Some(f) = log.as_mut() {
            let _ = writeln!(f, "[{}] {line}", chrono::Local::now().format("%H:%M:%S%.3f"));
        }
    };
    let mut publisher = match Publisher::create() {
        Ok(p) => p,
        Err(e) => {
            note(&format!("can't create the shared table: {e}"));
            return;
        }
    };
    let shared: &'static etw::Shared = Box::leak(Box::new(etw::Shared::new()));
    let wait = |secs: u64| {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until && !stop.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(250));
        }
    };

    while !stop.load(Ordering::Acquire) {
        let mut session = match etw::Session::start() {
            Ok(s) => s,
            Err((code, text)) => {
                note(&format!("{text} ({code})"));
                publisher.set_status(code, &text, 0);
                wait(30);
                continue;
            }
        };
        note("trace session started");
        publisher.set_status(0, "", 0);
        let consumer = std::thread::spawn(move || etw::consume(shared));
        std::thread::sleep(Duration::from_millis(500));
        session.request_rundown();

        let mut last_note = Instant::now();
        while !stop.load(Ordering::Acquire) && !consumer.is_finished() {
            std::thread::sleep(Duration::from_millis(500));
            let lost = session.events_lost();
            if let Ok(mut c) = shared.counters.lock() {
                c.publish(&mut publisher);
            }
            publisher.set_status(0, "", lost);
            if logging && last_note.elapsed() > Duration::from_secs(5) {
                last_note = Instant::now();
                if let Ok(seen) = shared.seen.lock() {
                    for (p, id, what) in seen.iter() {
                        note(&format!("event {p}{id}: {what}"));
                    }
                }
                note(&format!("events lost: {lost}"));
                if let Ok(c) = shared.counters.lock() {
                    for line in c.summary() {
                        note(&line);
                    }
                }
            }
        }
        session.stop();
        match consumer.join() {
            Ok(Err(e)) => {
                note(&e);
                publisher.set_status(1, &e, 0);
            }
            Err(_) => note("trace reader panicked"),
            Ok(Ok(())) => {}
        }
        drop(session);
        if !stop.load(Ordering::Acquire) {
            note("trace session ended; restarting in 10 s");
            wait(10);
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--install") => std::process::exit(scm::install() as i32),
        Some("--uninstall") => std::process::exit(scm::uninstall() as i32),
        Some("--run") => {
            let secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(15);
            let log = args.get(3).and_then(|p| std::fs::File::create(p).ok());
            let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_secs(secs));
                stop.store(true, Ordering::Release);
            });
            run(stop, log);
        }
        _ => {
            scm::dispatch();
        }
    }
}
