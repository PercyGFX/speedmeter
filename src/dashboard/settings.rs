//! Settings page. Every change applies right away (saved and sent to the meter).

use super::theme;
use super::{ui, App};
use crate::config::{Connection, Layout as MeterLayout, Placement, ThemePref, FONT_CHOICES, MB};
use crate::format::{Decimals, UnitMode};
use crate::net::Adapter;
use eframe::egui::{self, pos2, vec2, RichText, Sense, Ui};

fn adapter_desc(a: &Adapter) -> String {
    let mut parts = vec![a.description.clone()];
    if a.up {
        if let Some(ip) = &a.ipv4 {
            parts.push(ip.clone());
        }
        if a.link_speed >= 1_000_000_000 {
            parts.push(format!("{} Gbit/s link", a.link_speed / 1_000_000_000));
        } else if a.link_speed > 0 {
            parts.push(format!("{} Mbit/s link", a.link_speed / 1_000_000));
        }
    } else {
        parts.push("not connected".into());
    }
    parts.join(", ")
}

fn same_choice(a: &Connection, b: &Connection) -> bool {
    match (a, b) {
        (Connection::Adapter { id: x, .. }, Connection::Adapter { id: y, .. }) => x == y,
        _ => a == b,
    }
}

fn connection_list(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    let now = match (&app.cfg.connection, &app.snap) {
        (Connection::Auto, Some(s)) if s.is_fresh() && !s.iface_name.is_empty() => format!(" Right now that's {}.", s.iface_name),
        _ => String::new(),
    };
    let mut options = vec![
        (Connection::Auto, "Automatic".to_string(), format!("Follows the network Windows uses to reach the internet.{now}")),
        (
            Connection::All,
            "All networks combined".to_string(),
            "Adds every connected adapter together. Virtual adapters such as Hyper-V or a VPN can count the same traffic twice."
                .to_string(),
        ),
    ];
    // Connected physical adapters first, then connected virtual ones. Disconnected
    // adapters (old VPNs, unused Hyper-V switches) wait behind "Show more".
    let mut adapters: Vec<&Adapter> = app.adapters.iter().collect();
    adapters.sort_by_key(|a| (!a.up, a.virtual_like, a.name.to_lowercase()));
    let mut hidden = 0;
    for a in adapters {
        let conn = Connection::Adapter { id: a.id.clone(), name: a.name.clone() };
        if !a.up && !app.show_all_adapters && !same_choice(&app.cfg.connection, &conn) {
            hidden += 1;
            continue;
        }
        options.push((conn, a.name.clone(), adapter_desc(a)));
    }
    for (i, (conn, title, desc)) in options.into_iter().enumerate() {
        let selected = same_choice(&app.cfg.connection, &conn);
        let frame = egui::Frame::NONE
            .fill(p.layer)
            .stroke(egui::Stroke::new(1.0_f32, if selected { p.faint } else { p.line }))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin { left: 50, right: 18, top: 10, bottom: 10 })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.label(RichText::new(&title).font(theme::body(14.0)).color(p.text));
                ui.add(egui::Label::new(RichText::new(&desc).font(theme::body(12.5)).color(p.dim)).wrap());
            });
        let resp = ui.interact(frame.response.rect, ui.id().with(("conn", i)), Sense::click());
        ui::radio_mark(ui.painter(), &p, pos2(frame.response.rect.left() + 25.0, frame.response.rect.center().y), selected, resp.hovered());
        if resp.clicked() && !selected {
            app.cfg.connection = conn;
        }
        ui.add_space(3.0);
    }
    let toggle_label = if app.show_all_adapters {
        "Hide disconnected adapters".to_string()
    } else if hidden > 0 {
        format!("Show {hidden} disconnected adapter{}", if hidden == 1 { "" } else { "s" })
    } else {
        String::new()
    };
    if !toggle_label.is_empty() {
        let link = ui.add(egui::Label::new(RichText::new(toggle_label).font(theme::body(13.0)).color(p.accent)).sense(Sense::click()));
        if link.clicked() {
            app.show_all_adapters = !app.show_all_adapters;
        }
    }
}

fn threshold(ui: &mut Ui, value: &mut f64) {
    let mut mb = *value / MB;
    if ui
        .add(egui::DragValue::new(&mut mb).range(0.1..=100_000.0).speed(0.1).max_decimals(1).suffix(" MB/s"))
        .changed()
    {
        *value = mb * MB;
    }
}

fn swatch(ui: &mut Ui, color: egui::Color32) {
    let (r, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
    ui.painter().rect_filled(r, 4, color);
}

/// Daily, weekly and monthly limits, with how much of each is used.
fn data_limits(app: &mut App, ui: &mut Ui) {
    use crate::limits::Period;
    let p = app.pal;
    let usage = app.limit_usage();
    let used = |period: Period| usage.iter().find(|u| u.period == period).copied();
    let rows = [
        (Period::Day, "Daily limit", "today"),
        (Period::Week, "Weekly limit", "this week"),
        (Period::Month, "Monthly limit", "this month"),
    ];
    for (period, title, when) in rows {
        let desc = match used(period) {
            Some(u) => format!("Used {} {when}. It {}.", u.summary(), u.resets_text()),
            None => "Off.".to_string(),
        };
        let l = &mut app.cfg.limits;
        let limit = match period {
            Period::Day => &mut l.daily,
            Period::Week => &mut l.weekly,
            Period::Month => &mut l.monthly,
        };
        ui::setting_row(ui, &p, title, Some(&desc), |ui| {
            ui::toggle(ui, &p, &mut limit.on);
            if limit.on {
                ui.add_space(10.0);
                ui.add(
                    egui::DragValue::new(&mut limit.gb)
                        .range(0.1..=100_000.0)
                        .speed(0.25)
                        .suffix(" GB")
                        // "5 GB", "2.5 GB": no ".0" on whole numbers.
                        .custom_formatter(|v, _| if v.fract().abs() < 0.05 { format!("{v:.0}") } else { format!("{v:.1}") }),
                );
            }
        });
        if period == Period::Month && app.cfg.limits.monthly.on {
            ui::setting_row(
                ui,
                &p,
                "Monthly limit starts on",
                Some("The day of the month your allowance starts again, like a billing date."),
                |ui| {
                    ui.add(egui::DragValue::new(&mut app.cfg.limits.month_starts).range(1..=28).speed(0.1).prefix("Day "));
                },
            );
        }
    }
    let l = &mut app.cfg.limits;
    ui::setting_row(ui, &p, "What counts", Some("Some plans only count what you download."), |ui| {
        ui::segmented(ui, &p, "limit-count", &mut l.download_only, &[(false, "Download and upload"), (true, "Download only")]);
    });
    ui::setting_row(
        ui,
        &p,
        "Bar on the taskbar meter",
        Some("A thin bar under the numbers: blue, amber from 80%, red at 100%. Hover the meter to see every limit."),
        |ui| {
            ui::toggle(ui, &p, &mut l.bar);
        },
    );
    if l.bar && l.any() {
        use crate::config::BarShows;
        let mut options = vec![(BarShows::Closest, "Closest to its limit")];
        for (on, choice, label) in [(l.daily.on, BarShows::Daily, "Daily"), (l.weekly.on, BarShows::Weekly, "Weekly"), (l.monthly.on, BarShows::Monthly, "Monthly")] {
            if on {
                options.push((choice, label));
            }
        }
        // A picked limit that was turned off falls back to the closest one.
        if !options.iter().any(|(c, _)| *c == l.bar_shows) {
            l.bar_shows = BarShows::Closest;
        }
        if options.len() > 2 {
            ui::setting_row(ui, &p, "The bar shows", None, |ui| {
                ui::segmented(ui, &p, "bar-shows", &mut l.bar_shows, &options);
            });
        }
    }
    ui::setting_row(ui, &p, "Notifications", Some("A Windows notification at 80%, at 100%, and every 25% over."), |ui| {
        ui::toggle(ui, &p, &mut l.notify);
    });
}

/// The per-app counting service: on, off, its status, and its data.
fn per_app(app: &mut App, ui: &mut Ui) {
    use crate::apps::ServiceState;
    use std::sync::atomic::Ordering;
    let p = app.pal;
    app.refresh_service();
    let busy = app.service.busy.load(Ordering::Acquire);
    let s = &app.service;
    // (title, description, button, primary, install?)
    let (title, desc, button, primary, install): (&str, String, Option<&str>, bool, bool) = if busy {
        ("Waiting for administrator approval", "Windows asks before a service is added, changed or removed.".into(), None, false, true)
    } else if s.state == ServiceState::NotInstalled {
        (
            "Count data per app",
            "Shows which apps use your data, live and in History. A small background service does the counting under Windows' \
             limited Local Service account and shares only byte counts per app. Turning it on asks for administrator approval once."
                .into(),
            Some("Turn on"),
            true,
            true,
        )
    } else if s.needs_update() {
        ("Per-app counter update", "This version of SpeedMeter comes with a newer counter.".into(), Some("Update"), true, true)
    } else if s.state == ServiceState::Stopped {
        ("Per-app counter is stopped", "The SpeedMeter per-app counter service isn't running.".into(), Some("Start"), true, true)
    } else if let Some(e) = &s.error {
        ("Per-app counter has a problem", e.clone(), Some("Restart"), false, true)
    } else {
        (
            "Counting data per app",
            "The SpeedMeter per-app counter service is running. Turning it off removes the service; your per-app history stays."
                .into(),
            Some("Turn off"),
            false,
            false,
        )
    };
    ui::setting_row(ui, &p, title, Some(&desc), |ui| match button {
        Some(label) => {
            if ui::button(ui, &p, label, primary).clicked() {
                app.change_service(install);
            }
            if install && app.service.installed() && label != "Turn on" {
                if ui::button(ui, &p, "Turn off", false).clicked() {
                    app.change_service(false);
                }
            }
        }
        None => {
            ui.add(egui::Spinner::new().color(p.accent));
        }
    });
    let failed = app.service.failed.lock().ok().and_then(|f| f.clone());
    if let Some(e) = failed {
        ui::setting_row(ui, &p, "That didn't work", Some(&e), |_| {});
    }

    if app.confirm_clear_apps {
        ui::setting_row(ui, &p, "Delete per-app history?", Some("This removes which apps used how much, for every day. Your totals stay."), |ui| {
            if ui::button(ui, &p, "Cancel", false).clicked() {
                app.confirm_clear_apps = false;
            }
            if ui::button(ui, &p, "Delete", true).clicked() {
                app.clear_app_history();
                app.confirm_clear_apps = false;
            }
        });
    } else {
        let desc = format!(
            "Kept like the rest of your history: days forever, hours for {} days. Only app names and byte counts are stored, never addresses or content.",
            app.cfg.keep_minutes_days
        );
        ui::setting_row(ui, &p, "Per-app history", Some(&desc), |ui| {
            if ui::button(ui, &p, "Delete", false).clicked() {
                app.confirm_clear_apps = true;
            }
        });
    }
}

pub fn show(app: &mut App, ui: &mut Ui) {
    let p = app.pal;
    app.refresh_adapters();

    ui.label(RichText::new("Network").font(theme::strong(15.0)).color(p.text));
    ui.add_space(4.0);
    connection_list(app, ui);

    ui::section(ui, &p, "Taskbar meter");
    {
        let cfg = &mut app.cfg;
        ui::setting_row(ui, &p, "Position", None, |ui| {
            ui::segmented(
                ui,
                &p,
                "placement",
                &mut cfg.placement,
                &[(Placement::TaskbarRight, "Next to tray icons"), (Placement::TaskbarLeft, "Left side"), (Placement::Floating, "Floating")],
            );
        });
        if cfg.placement != Placement::Floating {
            let desc = if cfg.placement == Placement::TaskbarRight {
                "Moves the meter further left from the tray icons."
            } else {
                "Moves the meter further right from your app icons."
            };
            ui::setting_row(ui, &p, "Fine-tune position", Some(desc), |ui| {
                ui.add(egui::Slider::new(&mut cfg.offset, -200..=400).suffix(" px"));
            });
        }
        ui::setting_row(ui, &p, "Layout", None, |ui| {
            ui::segmented(ui, &p, "layout", &mut cfg.layout, &[(MeterLayout::Stacked, "Two lines"), (MeterLayout::Inline, "One line")]);
        });
        let first_label = if cfg.layout == MeterLayout::Stacked { "Show first" } else { "Order" };
        ui::setting_row(ui, &p, first_label, None, |ui| {
            ui::segmented(ui, &p, "order", &mut cfg.upload_first, &[(true, "Upload"), (false, "Download")]);
        });
        ui::setting_row(ui, &p, "Font", None, |ui| {
            egui::ComboBox::from_id_salt("font").selected_text(cfg.font.clone()).width(220.0).show_ui(ui, |ui| {
                for f in FONT_CHOICES {
                    ui.selectable_value(&mut cfg.font, f.to_string(), f);
                }
            });
        });
        ui::setting_row(ui, &p, "Text size", None, |ui| {
            ui.add(egui::Slider::new(&mut cfg.font_size, 9.0..=16.0).step_by(0.5));
        });
        ui::setting_row(ui, &p, "Arrows", Some("Show ↑ and ↓ next to the numbers."), |ui| {
            ui::toggle(ui, &p, &mut cfg.show_arrows);
        });
        ui::setting_row(
            ui,
            &p,
            "Animations",
            Some("Numbers roll smoothly, and busy speeds grow and leave speed streaks. Off keeps the meter still. Also applies to this window."),
            |ui| {
                ui::toggle(ui, &p, &mut cfg.animations);
            },
        );
        ui::setting_row(ui, &p, "Speed units", None, |ui| {
            ui::segmented(ui, &p, "unit", &mut cfg.unit, &[(UnitMode::Bytes, "MB/s"), (UnitMode::Bits, "Mbps")]);
        });
        ui::setting_row(ui, &p, "Decimal places", Some("Auto keeps three digits: 6.35, 63.5, 635."), |ui| {
            ui::segmented(ui, &p, "decimals", &mut cfg.decimals, &[(Decimals::Auto, "Auto"), (Decimals::One, "1"), (Decimals::Two, "2")]);
        });
    }

    ui::section(ui, &p, "Speed colors");
    {
        let cfg = &mut app.cfg;
        ui::setting_row(
            ui,
            &p,
            "Color busy speeds",
            Some("Speeds change color as the connection gets busier, in the taskbar and in the charts."),
            |ui| {
                ui::toggle(ui, &p, &mut cfg.heat.enabled);
            },
        );
        if cfg.heat.enabled {
            let h = &mut cfg.heat;
            ui::setting_row(ui, &p, "Busy from", None, |ui| {
                threshold(ui, &mut h.busy);
                swatch(ui, p.busy);
            });
            ui::setting_row(ui, &p, "Heavy from", None, |ui| {
                threshold(ui, &mut h.heavy);
                swatch(ui, p.heavy);
            });
            ui::setting_row(ui, &p, "Very heavy from", None, |ui| {
                threshold(ui, &mut h.extreme);
                swatch(ui, p.extreme);
            });
            h.heavy = h.heavy.max(h.busy * 1.1);
            h.extreme = h.extreme.max(h.heavy * 1.1);
        }
    }

    ui::section(ui, &p, "Data limits");
    data_limits(app, ui);

    ui::section(ui, &p, "General");
    {
        let cfg = &mut app.cfg;
        ui::setting_row(ui, &p, "Start with Windows", Some("Starts the taskbar meter when you sign in."), |ui| {
            ui::toggle(ui, &p, &mut cfg.start_with_windows);
        });
        ui::setting_row(ui, &p, "Tray icon", Some("Also shows SpeedMeter in the notification area."), |ui| {
            ui::toggle(ui, &p, &mut cfg.show_tray_icon);
        });
        ui::setting_row(ui, &p, "Window theme", Some("Colors for this window. The taskbar meter follows the taskbar."), |ui| {
            ui::segmented(ui, &p, "theme", &mut cfg.theme, &[(ThemePref::System, "System"), (ThemePref::Dark, "Dark"), (ThemePref::Light, "Light")]);
        });
    }

    ui::section(ui, &p, "Per-app usage");
    per_app(app, ui);

    ui::section(ui, &p, "Updates");
    super::update_rows(app, ui);

    ui::section(ui, &p, "Data");
    {
        let cfg = &mut app.cfg;
        ui::setting_row(
            ui,
            &p,
            "Keep minute-by-minute detail",
            Some("Detail older than this is removed. Daily totals are kept forever."),
            |ui| {
                ui.add(egui::Slider::new(&mut cfg.keep_minutes_days, 7..=365).suffix(" days"));
            },
        );
    }
    let dir = crate::config::data_dir();
    ui::setting_row(ui, &p, "Data folder", Some(&dir.display().to_string()), |ui| {
        if ui::button(ui, &p, "Open folder", false).clicked() {
            crate::sys::open_in_explorer(&dir);
        }
    });
    if app.confirm_clear {
        ui::setting_row(ui, &p, "Delete all recorded traffic?", Some("This removes every day, month and year of history. It can't be undone."), |ui| {
            if ui::button(ui, &p, "Cancel", false).clicked() {
                app.confirm_clear = false;
            }
            if ui::button(ui, &p, "Delete history", true).clicked() {
                app.clear_history();
                app.confirm_clear = false;
            }
        });
    } else {
        ui::setting_row(ui, &p, "Clear history", Some("Deletes all recorded traffic."), |ui| {
            if ui::button(ui, &p, "Clear history", false).clicked() {
                app.confirm_clear = true;
            }
        });
    }

    ui::section(ui, &p, "About");
    let version = format!("SpeedMeter {}", env!("CARGO_PKG_VERSION"));
    let credit = format!(
        "Made by {}. Free and open source under the {} License.",
        env!("CARGO_PKG_AUTHORS"),
        env!("CARGO_PKG_LICENSE")
    );
    let repo = format!("https://github.com/{}", crate::update::REPO);
    ui::setting_row(ui, &p, &version, Some(&credit), |ui| {
        // The row lays out right to left: GitHub ends up rightmost.
        if ui::button(ui, &p, "GitHub", false).clicked() {
            crate::sys::open_url(&repo);
        }
        if ui::button(ui, &p, "License", false).clicked() {
            crate::sys::open_url(&format!("{repo}/blob/main/LICENSE"));
        }
        if ui::button(ui, &p, "What's new", false).clicked() {
            crate::sys::open_url(&format!("{repo}/blob/main/CHANGELOG.md"));
        }
    });
    let running = crate::monitor::is_running();
    let desc = if running {
        "Running. Stopping closes the taskbar meter and pauses recording until you start it again."
    } else {
        "Not running, so nothing is being recorded."
    };
    ui::setting_row(ui, &p, "Taskbar meter", Some(desc), |ui| {
        if running {
            if ui::button(ui, &p, "Stop SpeedMeter", false).clicked() {
                crate::monitor::request_quit();
            }
        } else if ui::button(ui, &p, "Start SpeedMeter", true).clicked() {
            crate::monitor::start_in_background();
            app.reader_retry_now();
        }
    });
    ui.add_space(24.0);
}
