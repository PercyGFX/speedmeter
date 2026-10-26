//! Daily, weekly and monthly data limits: which period we're in, how much of it is
//! used, and when to tell the user.
//!
//! Usage is counted from SpeedMeter's own history (the network it measures), so it
//! starts when SpeedMeter started recording. Weeks start on Monday; the monthly
//! period starts on a day of the user's choosing, like a billing date.

use crate::config::{BarShows, Limits};
use crate::format;
use chrono::{Datelike, Duration, NaiveDate};
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    pub fn name(self) -> &'static str {
        match self {
            Period::Day => "Daily",
            Period::Week => "Weekly",
            Period::Month => "Monthly",
        }
    }

    fn noun(self) -> &'static str {
        match self {
            Period::Day => "today's",
            Period::Week => "this week's",
            Period::Month => "this month's",
        }
    }
}

/// Monday of the week `d` is in.
pub fn week_start(d: NaiveDate) -> NaiveDate {
    d - Duration::days(d.weekday().num_days_from_monday() as i64)
}

fn with_day(year: i32, month: u32, day: u32) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, month, day.clamp(1, 28)).unwrap_or_else(|| NaiveDate::from_ymd_opt(year, month, 1).unwrap())
}

/// First day of the monthly period `d` is in, when periods start on `start_day`.
pub fn month_start(d: NaiveDate, start_day: u32) -> NaiveDate {
    if d.day() >= start_day {
        with_day(d.year(), d.month(), start_day)
    } else if d.month() == 1 {
        with_day(d.year() - 1, 12, start_day)
    } else {
        with_day(d.year(), d.month() - 1, start_day)
    }
}

/// First day of the next period (when this one resets).
pub fn next_start(period: Period, d: NaiveDate, start_day: u32) -> NaiveDate {
    match period {
        Period::Day => d + Duration::days(1),
        Period::Week => week_start(d) + Duration::days(7),
        Period::Month => {
            let s = month_start(d, start_day);
            if s.month() == 12 {
                with_day(s.year() + 1, 1, start_day)
            } else {
                with_day(s.year(), s.month() + 1, start_day)
            }
        }
    }
}

pub fn period_start(period: Period, d: NaiveDate, start_day: u32) -> NaiveDate {
    match period {
        Period::Day => d,
        Period::Week => week_start(d),
        Period::Month => month_start(d, start_day),
    }
}

/// A limit as people set it: "100 GB" rather than "100.0 GB".
pub fn limit_text(bytes: u64) -> String {
    const GB: u64 = 1024 * 1024 * 1024;
    if bytes % GB == 0 {
        format!("{} GB", bytes / GB)
    } else {
        format::bytes(bytes)
    }
}

/// Bytes that count toward a limit.
pub fn counted(cfg: &Limits, (rx, tx): (u64, u64)) -> u64 {
    if cfg.download_only {
        rx
    } else {
        rx + tx
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Usage {
    pub period: Period,
    pub used: u64,
    pub limit: u64,
    /// The first day of the next period.
    pub resets: NaiveDate,
    /// The first day of this period (identifies it for notifications).
    pub start: NaiveDate,
}

impl Usage {
    pub fn fraction(&self) -> f64 {
        self.used as f64 / self.limit.max(1) as f64
    }

    pub fn percent(&self) -> u32 {
        (self.fraction() * 100.0).floor() as u32
    }

    /// "82.1 GB of 100 GB (82%)".
    pub fn summary(&self) -> String {
        format!("{} of {} ({}%)", format::bytes(self.used), limit_text(self.limit), self.percent())
    }

    /// "resets at midnight", "resets on Monday", "resets on November 1".
    pub fn resets_text(&self) -> String {
        match self.period {
            Period::Day => "resets at midnight".into(),
            Period::Week => "resets on Monday".into(),
            Period::Month => format!("resets on {}", self.resets.format("%B %-d")),
        }
    }
}

/// Usage of every limit that's on. `day`, `week` and `month` are (received, sent)
/// for the current periods.
pub fn usages(cfg: &Limits, today: NaiveDate, day: (u64, u64), week: (u64, u64), month: (u64, u64)) -> Vec<Usage> {
    let mut out = Vec::new();
    for (period, limit, used) in [(Period::Day, cfg.daily, day), (Period::Week, cfg.weekly, week), (Period::Month, cfg.monthly, month)] {
        if let Some(bytes) = limit.bytes() {
            out.push(Usage {
                period,
                used: counted(cfg, used),
                limit: bytes,
                resets: next_start(period, today, cfg.month_starts),
                start: period_start(period, today, cfg.month_starts),
            });
        }
    }
    out
}

/// The limit closest to running out (or furthest over).
pub fn tightest(list: &[Usage]) -> Option<Usage> {
    list.iter().copied().max_by(|a, b| a.fraction().total_cmp(&b.fraction()))
}

/// The limit the meter's bar shows: the one picked in Settings if it's on,
/// otherwise the one closest to running out. None when the bar is off.
pub fn for_bar(cfg: &Limits, list: &[Usage]) -> Option<Usage> {
    if !cfg.bar {
        return None;
    }
    let picked = match cfg.bar_shows {
        BarShows::Closest => None,
        BarShows::Daily => Some(Period::Day),
        BarShows::Weekly => Some(Period::Week),
        BarShows::Monthly => Some(Period::Month),
    };
    picked.and_then(|p| list.iter().find(|u| u.period == p).copied()).or_else(|| tightest(list))
}

/// The notification step a usage has reached, in percent: 0, 80, 100, 125, 150, ...
pub fn level(fraction: f64) -> u32 {
    let p = (fraction * 100.0).floor() as u32;
    match p {
        0..=79 => 0,
        80..=99 => 80,
        _ => 100 + (p - 100) / 25 * 25,
    }
}

/// Title and text for a notification at `level`.
pub fn message(u: &Usage, level: u32) -> (String, String) {
    let name = u.period.name().to_lowercase();
    let resets = u.resets_text();
    let resets = format!("{}{}.", resets[..1].to_uppercase(), &resets[1..]);
    match level {
        80 => (
            format!("{}% of {} data used", u.percent(), u.period.noun()),
            format!("{} of your {} {} limit. {resets}", format::bytes(u.used), limit_text(u.limit), name),
        ),
        100 => (
            format!("{} data limit reached", u.period.name()),
            format!("You've used {} of {}. {resets}", format::bytes(u.used), limit_text(u.limit)),
        ),
        _ => (
            format!("{} data limit: {}% used", u.period.name(), u.percent()),
            format!(
                "{} of {}, {} over. {resets}",
                format::bytes(u.used),
                limit_text(u.limit),
                format::bytes(u.used.saturating_sub(u.limit))
            ),
        ),
    }
}

/// Which notification each period already got, so they aren't repeated after a
/// restart. Kept in limits.json next to the history.
#[derive(Default)]
pub struct Notified {
    levels: HashMap<String, u32>,
}

fn key(u: &Usage) -> String {
    format!("{:?}:{}", u.period, u.start)
}

impl Notified {
    pub fn load() -> Self {
        let path = crate::config::data_dir().join("limits.json");
        let levels = std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Self { levels }
    }

    fn save(&self) {
        let path = crate::config::data_dir().join("limits.json");
        if let Ok(text) = serde_json::to_string_pretty(&self.levels) {
            let _ = std::fs::write(path, text);
        }
    }

    /// The usages that crossed a new step since the last notification, with that step.
    /// Remembers them, and forgets periods that are over.
    pub fn due(&mut self, list: &[Usage]) -> Vec<(Usage, u32)> {
        let mut out = Vec::new();
        let current: Vec<String> = list.iter().map(key).collect();
        let before = self.levels.len();
        self.levels.retain(|k, _| current.contains(k));
        let mut changed = self.levels.len() != before;
        for u in list {
            let lvl = level(u.fraction());
            let seen = self.levels.get(&key(u)).copied().unwrap_or(0);
            if lvl > seen {
                self.levels.insert(key(u), lvl);
                changed = true;
                out.push((*u, lvl));
            }
        }
        if changed {
            self.save();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Limit;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    #[test]
    fn periods() {
        assert_eq!(week_start(d(2026, 10, 26)), d(2026, 10, 26)); // a Monday
        assert_eq!(week_start(d(2026, 11, 1)), d(2026, 10, 26)); // Sunday
        assert_eq!(month_start(d(2026, 10, 26), 1), d(2026, 10, 1));
        assert_eq!(month_start(d(2026, 10, 3), 15), d(2026, 9, 15));
        assert_eq!(month_start(d(2026, 1, 3), 15), d(2025, 12, 15));
        assert_eq!(next_start(Period::Month, d(2026, 12, 20), 15), d(2027, 1, 15));
        assert_eq!(next_start(Period::Month, d(2026, 10, 26), 1), d(2026, 11, 1));
        assert_eq!(next_start(Period::Week, d(2026, 10, 28), 1), d(2026, 11, 2));
    }

    #[test]
    fn levels() {
        assert_eq!(level(0.5), 0);
        assert_eq!(level(0.8), 80);
        assert_eq!(level(0.999), 80);
        assert_eq!(level(1.0), 100);
        assert_eq!(level(1.05), 100);
        assert_eq!(level(1.25), 125);
        assert_eq!(level(1.6), 150);
        assert_eq!(level(2.1), 200);
    }

    #[test]
    fn usage_and_messages() {
        let gb = 1024u64.pow(3);
        let cfg = Limits { monthly: Limit { on: true, gb: 100.0 }, daily: Limit { on: true, gb: 5.0 }, ..Default::default() };
        let list = usages(&cfg, d(2026, 10, 26), (gb, gb), (0, 0), (100 * gb, 5 * gb));
        assert_eq!(list.len(), 2);
        let month = list.iter().find(|u| u.period == Period::Month).unwrap();
        assert_eq!(month.percent(), 105);
        assert_eq!(tightest(&list).unwrap().period, Period::Month);
        // 105% is still the "reached" step; the next one is 125%.
        let (title, text) = message(month, level(month.fraction()));
        assert_eq!(title, "Monthly data limit reached");
        assert_eq!(text, "You've used 105.0 GB of 100 GB. Resets on November 1.");
        let more = Usage { used: 126 * gb, ..*month };
        let (title, text) = message(&more, level(more.fraction()));
        assert!(title.contains("126%"), "{title}");
        assert!(text.contains("26.00 GB over"), "{text}");
        // The bar follows the closest limit unless one is picked.
        assert_eq!(for_bar(&cfg, &list).unwrap().period, Period::Month);
        let daily = Limits { bar_shows: BarShows::Daily, ..cfg.clone() };
        assert_eq!(for_bar(&daily, &list).unwrap().period, Period::Day);
        let weekly_off = Limits { bar_shows: BarShows::Weekly, ..cfg.clone() };
        assert_eq!(for_bar(&weekly_off, &list).unwrap().period, Period::Month);
        assert!(for_bar(&Limits { bar: false, ..cfg.clone() }, &list).is_none());
        let only_down = Limits { download_only: true, ..cfg };
        assert_eq!(usages(&only_down, d(2026, 10, 26), (gb, gb), (0, 0), (0, 0))[0].used, gb);
    }
}
