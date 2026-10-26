//! User settings, stored as JSON in %LOCALAPPDATA%\SpeedMeter\config.json.

use crate::format::{Decimals, UnitMode};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum Connection {
    /// Follow the adapter Windows routes internet traffic through.
    Auto,
    /// Sum of every adapter that is up.
    All,
    /// One specific adapter, identified by its GUID name.
    Adapter { id: String, name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    TaskbarRight,
    TaskbarLeft,
    Floating,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    Stacked,
    Inline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemePref {
    System,
    Dark,
    Light,
}

/// Speed thresholds (bytes per second) where the readout changes color.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Heat {
    pub enabled: bool,
    pub busy: f64,
    pub heavy: f64,
    pub extreme: f64,
}

pub const MB: f64 = 1024.0 * 1024.0;

impl Default for Heat {
    fn default() -> Self {
        Self { enabled: true, busy: 1.0 * MB, heavy: 10.0 * MB, extreme: 40.0 * MB }
    }
}

/// One data allowance.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limit {
    pub on: bool,
    /// Gigabytes (1024³ bytes, like the rest of the app).
    pub gb: f64,
}

impl Default for Limit {
    fn default() -> Self {
        Self { on: false, gb: 100.0 }
    }
}

impl Limit {
    pub fn bytes(&self) -> Option<u64> {
        (self.on && self.gb > 0.0).then(|| (self.gb * 1024.0 * 1024.0 * 1024.0) as u64)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BarShows {
    /// Whichever limit is closest to running out.
    Closest,
    Daily,
    Weekly,
    Monthly,
}

/// Daily, weekly and monthly data limits.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Limits {
    pub daily: Limit,
    pub weekly: Limit,
    pub monthly: Limit,
    /// Day of the month the monthly allowance starts again (1 to 28), like a billing date.
    pub month_starts: u32,
    /// Count only downloads (some plans), instead of downloads and uploads.
    pub download_only: bool,
    /// Show how much is used as a bar on the taskbar meter.
    pub bar: bool,
    /// Which limit the bar shows.
    pub bar_shows: BarShows,
    /// Windows notifications at 80%, 100% and every 25% beyond.
    pub notify: bool,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            daily: Limit { on: false, gb: 5.0 },
            weekly: Limit { on: false, gb: 25.0 },
            monthly: Limit { on: false, gb: 100.0 },
            month_starts: 1,
            download_only: false,
            bar: true,
            bar_shows: BarShows::Closest,
            notify: true,
        }
    }
}

impl Limits {
    pub fn any(&self) -> bool {
        self.daily.bytes().is_some() || self.weekly.bytes().is_some() || self.monthly.bytes().is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub connection: Connection,
    pub placement: Placement,
    /// Extra distance (in DIPs) between the widget and its anchor point.
    pub offset: i32,
    pub floating_pos: Option<(i32, i32)>,
    pub layout: Layout,
    pub upload_first: bool,
    pub font: String,
    pub font_size: f32,
    pub show_arrows: bool,
    /// Off: a plain, still meter. On: rolling numbers, text that grows with the
    /// traffic, smooth color blends and speed streaks.
    pub animations: bool,
    pub unit: UnitMode,
    pub decimals: Decimals,
    pub heat: Heat,
    pub start_with_windows: bool,
    pub show_tray_icon: bool,
    pub theme: ThemePref,
    /// How long minute-level data is kept. Daily totals are kept forever.
    pub keep_minutes_days: u32,
    /// Download and install new releases in the background.
    pub auto_update: bool,
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            connection: Connection::Auto,
            placement: Placement::TaskbarRight,
            offset: 0,
            floating_pos: None,
            layout: Layout::Stacked,
            upload_first: true,
            font: "Bahnschrift".into(),
            font_size: 12.0,
            show_arrows: true,
            animations: false,
            unit: UnitMode::Bytes,
            decimals: Decimals::Auto,
            heat: Heat::default(),
            start_with_windows: true,
            show_tray_icon: true,
            theme: ThemePref::System,
            keep_minutes_days: 30,
            auto_update: true,
            limits: Limits::default(),
        }
    }
}

pub const FONT_CHOICES: [&str; 4] = ["Bahnschrift", "Segoe UI Variable Text", "Segoe UI", "Cascadia Mono"];

pub fn data_dir() -> PathBuf {
    // Lets you point a test or demo copy at a different folder.
    if let Some(dir) = std::env::var_os("SPEEDMETER_DATA_DIR") {
        let dir = PathBuf::from(dir);
        let _ = std::fs::create_dir_all(&dir);
        return dir;
    }
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir());
    let dir = base.join("SpeedMeter");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Empty for the normal copy. A copy pointed at another data folder
/// (SPEEDMETER_DATA_DIR, for demos and testing) gets a suffix that keeps its
/// window, locks and shared memory apart, so it can run next to the real one.
pub fn instance_suffix() -> &'static str {
    static SUFFIX: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SUFFIX.get_or_init(|| match std::env::var("SPEEDMETER_DATA_DIR") {
        Ok(dir) if !dir.is_empty() => {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            dir.to_lowercase().hash(&mut h);
            format!(".{:016x}", h.finish())
        }
        _ => String::new(),
    })
}

pub fn config_path() -> PathBuf {
    data_dir().join("config.json")
}

pub fn db_path() -> PathBuf {
    data_dir().join("history.db")
}

pub fn log_path() -> PathBuf {
    data_dir().join("speedmeter.log")
}

pub fn exists() -> bool {
    config_path().exists()
}

pub fn load() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .map(sanitize)
        .unwrap_or_default()
}

fn sanitize(mut c: Config) -> Config {
    c.font_size = c.font_size.clamp(8.0, 20.0);
    c.offset = c.offset.clamp(-2000, 2000);
    c.keep_minutes_days = c.keep_minutes_days.clamp(1, 3650);
    c.limits.month_starts = c.limits.month_starts.clamp(1, 28);
    for l in [&mut c.limits.daily, &mut c.limits.weekly, &mut c.limits.monthly] {
        l.gb = if l.gb.is_finite() { l.gb.clamp(0.1, 1_000_000.0) } else { 100.0 };
    }
    if c.font.trim().is_empty() {
        c.font = Config::default().font;
    }
    let h = &mut c.heat;
    h.busy = h.busy.max(1024.0);
    h.heavy = h.heavy.max(h.busy * 1.01);
    h.extreme = h.extreme.max(h.heavy * 1.01);
    c
}

/// Writes atomically (temp file + rename) so the two processes never read half a file.
pub fn save(c: &Config) -> std::io::Result<()> {
    let path = config_path();
    let tmp = path.with_extension("json.tmp");
    let text = serde_json::to_string_pretty(c).map_err(std::io::Error::other)?;
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &path)
}

pub fn modified() -> Option<SystemTime> {
    std::fs::metadata(config_path()).and_then(|m| m.modified()).ok()
}
