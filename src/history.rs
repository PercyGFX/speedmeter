//! Traffic history in SQLite (%LOCALAPPDATA%\SpeedMeter\history.db).
//!
//! * `minute`: one row per minute with traffic, kept for `keep_minutes_days`.
//! * `day`: one row per local calendar day, kept forever.
//!
//! The monitor writes through a background thread so a slow disk never stalls the
//! taskbar. The dashboard opens its own read connection (WAL mode allows both).

use chrono::{Datelike, Local, NaiveDate, TimeZone, Timelike};
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

pub enum WriteMsg {
    Minute(MinuteRecord),
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
            peak_tx REAL NOT NULL DEFAULT 0);",
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
            for msg in rx {
                match msg {
                    WriteMsg::Minute(r) => {
                        if let Err(e) = write_minute(&mut conn, &r) {
                            crate::sys::log(&format!("history: write failed: {e}"));
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

pub fn oldest_minute(c: &Connection) -> Option<i64> {
    c.query_row("SELECT MIN(ts) FROM minute", [], |r| r.get::<_, Option<i64>>(0)).ok().flatten()
}

pub fn clear(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("DELETE FROM minute; DELETE FROM day;")?;
    let _ = c.execute_batch("VACUUM;");
    Ok(())
}

/// TrafficMonitor keeps daily totals in %APPDATA%\TrafficMonitor\history_traffic.dat.
pub fn trafficmonitor_file() -> Option<PathBuf> {
    let p = PathBuf::from(std::env::var_os("APPDATA")?).join("TrafficMonitor").join("history_traffic.dat");
    p.exists().then_some(p)
}

/// Parses one line: "2026/10/24 1373593/55718363" (upload KB / download KB).
/// Very old files store a single combined number; that is counted as download.
fn parse_tm_line(line: &str) -> Option<(NaiveDate, u64, u64)> {
    let mut parts = line.split_whitespace();
    let date = NaiveDate::parse_from_str(parts.next()?, "%Y/%m/%d").ok()?;
    let vals = parts.next()?;
    let (up_kb, down_kb): (u64, u64) = match vals.split_once('/') {
        Some((u, d)) => (u.parse().ok()?, d.parse().ok()?),
        None => (0, vals.parse().ok()?),
    };
    Some((date, down_kb * 1024, up_kb * 1024))
}

/// Imports TrafficMonitor's daily totals. Days that already exist keep the larger value,
/// so running the import twice (or alongside SpeedMeter's own data) never double counts.
pub fn import_trafficmonitor(c: &mut Connection, path: &Path) -> Result<usize, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    let text = if bytes.starts_with(&[0xFF, 0xFE]) {
        let units: Vec<u16> = bytes[2..].chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
        String::from_utf16_lossy(&units)
    } else {
        String::from_utf8_lossy(&bytes).into_owned()
    };
    let t = c.transaction().map_err(|e| e.to_string())?;
    let mut n = 0;
    for (date, rx, tx) in text.lines().filter_map(parse_tm_line) {
        t.execute(
            "INSERT INTO day(day,rx,tx) VALUES(?1,?2,?3)
             ON CONFLICT(day) DO UPDATE SET rx=max(rx,excluded.rx), tx=max(tx,excluded.tx)",
            params![day_key(date), rx as i64, tx as i64],
        )
        .map_err(|e| e.to_string())?;
        n += 1;
    }
    t.commit().map_err(|e| e.to_string())?;
    Ok(n)
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

pub fn same_month(a: NaiveDate, b: NaiveDate) -> bool {
    a.year() == b.year() && a.month() == b.month()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_trafficmonitor_lines() {
        let (d, rx, tx) = parse_tm_line("2026/10/24 1373593/55718363").unwrap();
        assert_eq!(d, NaiveDate::from_ymd_opt(2026, 10, 24).unwrap());
        assert_eq!(rx, 55718363 * 1024);
        assert_eq!(tx, 1373593 * 1024);
        assert!(parse_tm_line("lines: \"1203\"").is_none());
        assert_eq!(parse_tm_line("2019/01/02 500").unwrap().1, 500 * 1024);
    }

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
}
