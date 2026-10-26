//! Traffic history in SQLite (%LOCALAPPDATA%\SpeedMeter\history.db).
//!
//! * `minute`: one row per minute with traffic, kept for `keep_minutes_days`.
//! * `day`: one row per local calendar day, kept forever.
//! * `app`, `app_day`, `app_hour`: per-app usage (see apps.rs), written each minute
//!   alongside `minute`. App days are kept forever, app hours as long as minutes.
//!
//! The monitor writes through a background thread so a slow disk never stalls the
//! taskbar. The dashboard opens its own read connection (WAL mode allows both).

use chrono::{Local, NaiveDate, TimeZone, Timelike};
use rusqlite::{params, Connection};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct MinuteRecord {
    /// Unix seconds at the start of the minute.
    pub ts: i64,
    /// Local calendar day the minute belongs to, YYYY-MM-DD.
    pub day: String,
    pub rx: u64,
    pub tx: u64,
    pub peak_rx: f64,
    pub peak_tx: f64,
}

/// One minute of per-app bytes. Includes an `Other` row (key "") for what the
/// meter measured but no app accounts for, so apps add up to the minute's total.
#[derive(Clone, Debug)]
pub struct AppMinute {
    /// Unix start of the local hour the minute belongs to.
    pub hour: i64,
    pub day: String,
    pub rows: Vec<crate::apps::AppDelta>,
}

pub enum WriteMsg {
    Minute(MinuteRecord),
    Apps(AppMinute),
    Prune { keep_days: u32 },
    /// Replies once everything queued before it is on disk.
    Flush(mpsc::Sender<()>),
}

pub fn open(path: &Path) -> rusqlite::Result<Connection> {
    let c = Connection::open(path)?;
    c.busy_timeout(Duration::from_secs(5))?;
    c.query_row("PRAGMA journal_mode=WAL", [], |_| Ok(()))?;
    c.execute_batch(
        "PRAGMA synchronous=NORMAL;
         CREATE TABLE IF NOT EXISTS minute(
            ts INTEGER PRIMARY KEY,
            rx INTEGER NOT NULL DEFAULT 0,
            tx INTEGER NOT NULL DEFAULT 0,
            peak_rx REAL NOT NULL DEFAULT 0,
            peak_tx REAL NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS day(
            day TEXT PRIMARY KEY,
            rx INTEGER NOT NULL DEFAULT 0,
            tx INTEGER NOT NULL DEFAULT 0,
            peak_rx REAL NOT NULL DEFAULT 0,
            peak_tx REAL NOT NULL DEFAULT 0);
         CREATE TABLE IF NOT EXISTS app(
            id INTEGER PRIMARY KEY,
            key TEXT NOT NULL UNIQUE,
            name TEXT NOT NULL,
            file TEXT NOT NULL DEFAULT '');
         CREATE TABLE IF NOT EXISTS app_day(
            day TEXT NOT NULL,
            app INTEGER NOT NULL,
            rx INTEGER NOT NULL DEFAULT 0,
            tx INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(day, app)) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS app_hour(
            ts INTEGER NOT NULL,
            app INTEGER NOT NULL,
            rx INTEGER NOT NULL DEFAULT 0,
            tx INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY(ts, app)) WITHOUT ROWID;
",
    )?;
    Ok(c)
}

fn write_minute(c: &mut Connection, r: &MinuteRecord) -> rusqlite::Result<()> {
    let t = c.transaction()?;
    t.execute(
        "INSERT INTO minute(ts,rx,tx,peak_rx,peak_tx) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(ts) DO UPDATE SET rx=rx+excluded.rx, tx=tx+excluded.tx,
            peak_rx=max(peak_rx,excluded.peak_rx), peak_tx=max(peak_tx,excluded.peak_tx)",
        params![r.ts, r.rx as i64, r.tx as i64, r.peak_rx, r.peak_tx],
    )?;
    t.execute(
        "INSERT INTO day(day,rx,tx,peak_rx,peak_tx) VALUES(?1,?2,?3,?4,?5)
         ON CONFLICT(day) DO UPDATE SET rx=rx+excluded.rx, tx=tx+excluded.tx,
            peak_rx=max(peak_rx,excluded.peak_rx), peak_tx=max(peak_tx,excluded.peak_tx)",
        params![r.day, r.rx as i64, r.tx as i64, r.peak_rx, r.peak_tx],
    )?;
    t.commit()
}

fn prune(c: &Connection, keep_days: u32) -> rusqlite::Result<usize> {
    let cutoff = Local::now().timestamp() - keep_days as i64 * 86_400;
    c.execute("DELETE FROM app_hour WHERE ts < ?1", params![cutoff])?;
    c.execute("DELETE FROM minute WHERE ts < ?1", params![cutoff])
}

pub fn spawn_writer(path: PathBuf) -> mpsc::Sender<WriteMsg> {
    let (tx, rx) = mpsc::channel::<WriteMsg>();
    std::thread::Builder::new()
        .name("history-writer".into())
        .spawn(move || {
            let mut conn = match open(&path) {
                Ok(c) => c,
                Err(e) => {
                    crate::sys::log(&format!("history: cannot open {}: {e}", path.display()));
                    for m in rx {
                        if let WriteMsg::Flush(ack) = m {
                            let _ = ack.send(());
                        }
                    }
                    return;
                }
            };
            let mut names = crate::apps::Resolver::new();
            for msg in rx {
                match msg {
                    WriteMsg::Minute(r) => {
                        if let Err(e) = write_minute(&mut conn, &r) {
                            crate::sys::log(&format!("history: write failed: {e}"));
                        }
                    }
                    WriteMsg::Apps(m) => {
                        if let Err(e) = write_apps(&mut conn, &m, &mut names) {
                            crate::sys::log(&format!("history: app write failed: {e}"));
                        }
                    }
                    WriteMsg::Prune { keep_days } => {
                        let _ = prune(&conn, keep_days);
                    }
                    WriteMsg::Flush(ack) => {
                        let _ = ack.send(());
                    }
                }
            }
        })
        .expect("spawn history writer");
    tx
}

/// One bar in a chart or one line in a table.
#[derive(Clone, Debug, Default)]
pub struct Row {
    /// "2026-10-24", "2026-10", "2026", or the hour "07".
    pub key: String,
    pub rx: u64,
    pub tx: u64,
    pub peak_rx: f64,
    pub peak_tx: f64,
}

impl Row {
    pub fn total(&self) -> u64 {
        self.rx + self.tx
    }
}

fn rows(c: &Connection, sql: &str, arg: &str) -> Vec<Row> {
    let Ok(mut st) = c.prepare(sql) else { return Vec::new() };
    let it = st.query_map(params![arg], |r| {
        Ok(Row {
            key: r.get(0)?,
            rx: r.get::<_, i64>(1)?.max(0) as u64,
            tx: r.get::<_, i64>(2)?.max(0) as u64,
            peak_rx: r.get(3)?,
            peak_tx: r.get(4)?,
        })
    });
    match it {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

pub fn day_key(d: NaiveDate) -> String {
    d.format("%Y-%m-%d").to_string()
}

pub fn month_key(d: NaiveDate) -> String {
    d.format("%Y-%m").to_string()
}

pub fn day_totals(c: &Connection, day: &str) -> (u64, u64) {
    c.query_row("SELECT rx, tx FROM day WHERE day=?1", params![day], |r| {
        Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64))
    })
    .unwrap_or((0, 0))
}

pub fn month_totals(c: &Connection, month: &str) -> (u64, u64) {
    c.query_row(
        "SELECT COALESCE(SUM(rx),0), COALESCE(SUM(tx),0) FROM day WHERE substr(day,1,7)=?1",
        params![month],
        |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, i64>(1)? as u64)),
    )
    .unwrap_or((0, 0))
}

/// Totals of the days from `from` up to, but not including, `to` (YYYY-MM-DD).
pub fn sum_days(c: &Connection, from: &str, to: &str) -> (u64, u64) {
    c.query_row(
        "SELECT COALESCE(SUM(rx),0), COALESCE(SUM(tx),0) FROM day WHERE day >= ?1 AND day < ?2",
        params![from, to],
        |r| Ok((r.get::<_, i64>(0)?.max(0) as u64, r.get::<_, i64>(1)?.max(0) as u64)),
    )
    .unwrap_or((0, 0))
}

/// Days of one month (YYYY-MM) that have data.
pub fn days_of_month(c: &Connection, month: &str) -> Vec<Row> {
    rows(
        c,
        "SELECT day, rx, tx, peak_rx, peak_tx FROM day WHERE substr(day,1,7)=?1 ORDER BY day",
        month,
    )
}

pub fn months_of_year(c: &Connection, year: i32) -> Vec<Row> {
    rows(
        c,
        "SELECT substr(day,1,7) m, SUM(rx), SUM(tx), MAX(peak_rx), MAX(peak_tx)
         FROM day WHERE substr(day,1,4)=?1 GROUP BY m ORDER BY m",
        &year.to_string(),
    )
}

pub fn years(c: &Connection) -> Vec<Row> {
    rows(
        c,
        "SELECT substr(day,1,4) y, SUM(rx), SUM(tx), MAX(peak_rx), MAX(peak_tx)
         FROM day WHERE ?1 = ?1 GROUP BY y ORDER BY y",
        "",
    )
}

pub fn first_day(c: &Connection) -> Option<NaiveDate> {
    let s: String = c.query_row("SELECT MIN(day) FROM day", [], |r| r.get(0)).ok()?;
    NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
}

fn local_ts(d: NaiveDate) -> i64 {
    let naive = d.and_hms_opt(0, 0, 0).unwrap();
    Local
        .from_local_datetime(&naive)
        .earliest()
        .map(|t| t.timestamp())
        .unwrap_or_else(|| naive.and_utc().timestamp())
}

fn minute_rows(c: &Connection, from: i64, to: i64) -> Vec<(i64, u64, u64, f64, f64)> {
    let Ok(mut st) = c.prepare(
        "SELECT ts, rx, tx, peak_rx, peak_tx FROM minute WHERE ts >= ?1 AND ts < ?2 ORDER BY ts",
    ) else {
        return Vec::new();
    };
    let it = st.query_map(params![from, to], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, i64>(1)?.max(0) as u64,
            r.get::<_, i64>(2)?.max(0) as u64,
            r.get::<_, f64>(3)?,
            r.get::<_, f64>(4)?,
        ))
    });
    match it {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Vec::new(),
    }
}

fn add(row: &mut Row, rx: u64, tx: u64, prx: f64, ptx: f64) {
    row.rx += rx;
    row.tx += tx;
    row.peak_rx = row.peak_rx.max(prx);
    row.peak_tx = row.peak_tx.max(ptx);
}

/// 24 hourly rows for one local day, from minute-level data. Hours with no data are zero.
pub fn hours_of_day(c: &Connection, date: NaiveDate) -> Vec<Row> {
    let from = local_ts(date);
    let to = local_ts(date.succ_opt().unwrap_or(date));
    let mut out: Vec<Row> = (0..24).map(|h| Row { key: format!("{h:02}"), ..Default::default() }).collect();
    for (ts, rx, tx, prx, ptx) in minute_rows(c, from, to) {
        if let Some(t) = Local.timestamp_opt(ts, 0).single() {
            add(&mut out[t.hour() as usize], rx, tx, prx, ptx);
        }
    }
    out
}

/// The last `n` local hours (oldest first), keyed by the hour's unix start time.
pub fn recent_hours(c: &Connection, n: usize) -> Vec<Row> {
    let now = Local::now();
    let hour_start = now
        .with_minute(0)
        .and_then(|t| t.with_second(0))
        .and_then(|t| t.with_nanosecond(0))
        .map(|t| t.timestamp())
        .unwrap_or(now.timestamp() / 3600 * 3600);
    let first = hour_start - (n as i64 - 1) * 3600;
    let mut out: Vec<Row> = (0..n)
        .map(|i| Row { key: (first + i as i64 * 3600).to_string(), ..Default::default() })
        .collect();
    for (ts, rx, tx, prx, ptx) in minute_rows(c, first, hour_start + 3600) {
        let i = ((ts - first) / 3600) as usize;
        if i < n {
            add(&mut out[i], rx, tx, prx, ptx);
        }
    }
    out
}

pub fn clear(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("DELETE FROM minute; DELETE FROM day;")?;
    clear_apps(c)
}

// ---------------------------------------------------------------------------
// Per-app usage

/// One app in a list, with its totals.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppRow {
    /// Windows' id for the app: an exe path such as `\device\harddiskvolume3\...\app.exe`,
    /// a package family name, or `System`. Builds of one app that live in different
    /// folders share a row; this is the most recently seen id.
    pub key: String,
    pub name: String,
    /// The exe's file name or the package family name ("" for System and Other).
    pub file: String,
    pub rx: u64,
    pub tx: u64,
}

impl AppRow {
    pub fn total(&self) -> u64 {
        self.rx + self.tx
    }
}

/// Every known app key with its row id.
#[cfg(test)]
pub fn app_ids(c: &Connection) -> std::collections::HashMap<String, i64> {
    let Ok(mut st) = c.prepare("SELECT key, id FROM app") else { return Default::default() };
    let it = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)));
    match it {
        Ok(it) => it.filter_map(Result::ok).collect(),
        Err(_) => Default::default(),
    }
}

pub fn add_app(c: &Connection, key: &str, name: &str, file: &str) -> rusqlite::Result<i64> {
    c.execute("INSERT OR IGNORE INTO app(key,name,file) VALUES(?1,?2,?3)", params![key, name, file])?;
    c.query_row("SELECT id FROM app WHERE key=?1", params![key], |r| r.get(0))
}

/// Adds one minute of per-app bytes to its hour and its day. An app is named
/// once, the first time it shows up.
pub fn write_apps(c: &mut Connection, m: &AppMinute, names: &mut crate::apps::Resolver) -> rusqlite::Result<()> {
    let t = c.transaction()?;
    for r in m.rows.iter().filter(|r| r.rx + r.tx > 0) {
        let known: Option<i64> = t.query_row("SELECT id FROM app WHERE key=?1", params![r.key], |row| row.get(0)).ok();
        let id = match known {
            Some(id) => id,
            None => {
                let (name, file) = names.describe(&r.key, r.hint.as_deref());
                add_app(&t, &r.key, &name, &file)?
            }
        };
        let (rx, tx) = (r.rx as i64, r.tx as i64);
        t.execute(
            "INSERT INTO app_hour(ts,app,rx,tx) VALUES(?1,?2,?3,?4)
             ON CONFLICT(ts,app) DO UPDATE SET rx=rx+excluded.rx, tx=tx+excluded.tx",
            params![m.hour, id, rx, tx],
        )?;
        t.execute(
            "INSERT INTO app_day(day,app,rx,tx) VALUES(?1,?2,?3,?4)
             ON CONFLICT(day,app) DO UPDATE SET rx=rx+excluded.rx, tx=tx+excluded.tx",
            params![m.day, id, rx, tx],
        )?;
    }
    t.commit()
}

/// Deletes all per-app data.
pub fn clear_apps(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("DELETE FROM app_hour; DELETE FROM app_day; DELETE FROM app;")
}

fn app_rows(c: &Connection, sql: &str, a: &dyn rusqlite::ToSql, b: &dyn rusqlite::ToSql) -> Vec<AppRow> {
    let Ok(mut st) = c.prepare(sql) else { return Vec::new() };
    let it = st.query_map(params![a, b], |r| {
        Ok(AppRow {
            name: r.get(0)?,
            file: r.get(1)?,
            key: r.get(2)?,
            rx: r.get::<_, i64>(4)?.max(0) as u64,
            tx: r.get::<_, i64>(5)?.max(0) as u64,
        })
    });
    let mut rows: Vec<AppRow> = match it {
        Ok(it) => it.filter_map(Result::ok).filter(|r| r.total() > 0).collect(),
        Err(_) => Vec::new(),
    };
    rows.sort_by(|a, b| b.total().cmp(&a.total()).then_with(|| a.name.cmp(&b.name)));
    rows
}

/// Apps between two days (YYYY-MM-DD, both included), busiest first. Builds of
/// the same app (same name and file) are added together.
pub fn apps_between(c: &Connection, from_day: &str, to_day: &str) -> Vec<AppRow> {
    // With a single MAX(), SQLite takes the bare `key` from the row holding the max id.
    app_rows(
        c,
        "SELECT a.name, a.file, a.key, MAX(a.id), SUM(d.rx), SUM(d.tx)
         FROM app_day d JOIN app a ON a.id = d.app
         WHERE d.day >= ?1 AND d.day <= ?2
         GROUP BY a.name, a.file",
        &from_day,
        &to_day,
    )
}

/// Apps from the hourly detail between two unix times.
pub fn apps_in_hours(c: &Connection, from: i64, to: i64) -> Vec<AppRow> {
    app_rows(
        c,
        "SELECT a.name, a.file, a.key, MAX(a.id), SUM(h.rx), SUM(h.tx)
         FROM app_hour h JOIN app a ON a.id = h.app
         WHERE h.ts >= ?1 AND h.ts < ?2
         GROUP BY a.name, a.file",
        &from,
        &to,
    )
}

/// The first day with per-app records.
pub fn first_app_day(c: &Connection) -> Option<NaiveDate> {
    let s: String = c.query_row("SELECT MIN(day) FROM app_day", [], |r| r.get(0)).ok()?;
    NaiveDate::parse_from_str(&s, "%Y-%m-%d").ok()
}

/// Unix start of local hour `h` on `date` (the earlier one when a clock change repeats it).
pub fn hour_ts(date: NaiveDate, h: u32) -> i64 {
    date.and_hms_opt(h, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).earliest())
        .map(|t| t.timestamp())
        .unwrap_or_else(|| local_ts(date) + h as i64 * 3600)
}

pub fn today() -> NaiveDate {
    Local::now().date_naive()
}

pub fn days_in_month(year: i32, month: u32) -> u32 {
    let first = NaiveDate::from_ymd_opt(year, month, 1).unwrap();
    let next = if month == 12 {
        NaiveDate::from_ymd_opt(year + 1, 1, 1).unwrap()
    } else {
        NaiveDate::from_ymd_opt(year, month + 1, 1).unwrap()
    };
    (next - first).num_days() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_in_memory() {
        let mut c = open(Path::new(":memory:")).unwrap();
        let r = MinuteRecord { ts: 1_700_000_040, day: "2023-11-14".into(), rx: 100, tx: 7, peak_rx: 5.0, peak_tx: 1.0 };
        write_minute(&mut c, &r).unwrap();
        write_minute(&mut c, &r).unwrap();
        assert_eq!(day_totals(&c, "2023-11-14"), (200, 14));
        assert_eq!(month_totals(&c, "2023-11"), (200, 14));
        assert_eq!(days_of_month(&c, "2023-11").len(), 1);
        assert_eq!(months_of_year(&c, 2023)[0].rx, 200);
        assert_eq!(years(&c)[0].key, "2023");
    }

    #[test]
    fn apps_add_up_and_group() {
        use crate::apps::AppDelta;
        let mut c = open(Path::new(":memory:")).unwrap();
        let mut names = crate::apps::Resolver::new();
        let d = |key: &str, rx, tx| AppDelta { key: key.into(), hint: None, rx, tx };
        let m = AppMinute { hour: 3600, day: "2026-10-01".into(), rows: vec![d("chat.exe", 100, 10), d("", 5, 5)] };
        write_apps(&mut c, &m, &mut names).unwrap();
        write_apps(&mut c, &m, &mut names).unwrap();
        let next = AppMinute { hour: 7200, day: "2026-10-02".into(), rows: vec![d("chat.exe", 1, 1)] };
        write_apps(&mut c, &next, &mut names).unwrap();

        let rows = apps_between(&c, "2026-10-01", "2026-10-31");
        assert_eq!(rows.len(), 2);
        assert_eq!((rows[0].name.as_str(), rows[0].rx, rows[0].tx), ("chat.exe", 201, 21));
        assert_eq!((rows[1].name.as_str(), rows[1].rx), ("Other", 10));
        assert_eq!(apps_in_hours(&c, 0, 7200)[0].rx, 200);
        assert_eq!(apps_between(&c, "2026-10-02", "2026-10-02")[0].rx, 1);
        assert_eq!(first_app_day(&c), NaiveDate::from_ymd_opt(2026, 10, 1));

        // Builds of one app in different folders share a row; the newest id stands for it.
        add_app(&c, r"c:\app-1\x.exe", "X", "x.exe").unwrap();
        let newer = add_app(&c, r"c:\app-2\x.exe", "X", "x.exe").unwrap();
        let old = app_ids(&c)[r"c:\app-1\x.exe"];
        c.execute_batch(&format!("INSERT INTO app_day VALUES('2026-10-03',{old},5,0); INSERT INTO app_day VALUES('2026-10-03',{newer},6,0);")).unwrap();
        let x = apps_between(&c, "2026-10-03", "2026-10-03");
        assert_eq!((x.len(), x[0].rx), (1, 11));
        assert!(x[0].key.contains("app-2"));

        clear(&c).unwrap();
        assert!(apps_between(&c, "0000", "9999").is_empty());
    }
}
