//! Turning raw byte counts into short, stable, human-readable text.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitMode {
    /// KB/s, MB/s, GB/s (1 KB = 1024 bytes)
    Bytes,
    /// Kbps, Mbps, Gbps (1 Kb = 1000 bits)
    Bits,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Decimals {
    /// Three significant digits: 6.35, 63.5, 635
    Auto,
    One,
    Two,
}

pub const BYTE_UNITS: [&str; 4] = ["KB/s", "MB/s", "GB/s", "TB/s"];
pub const BIT_UNITS: [&str; 4] = ["Kbps", "Mbps", "Gbps", "Tbps"];

/// A speed split into the number and its unit, so callers can style them separately.
#[derive(Clone, Debug, PartialEq)]
pub struct Speed {
    pub value: String,
    pub unit: &'static str,
}

impl std::fmt::Display for Speed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.value, self.unit)
    }
}

fn number(v: f64, decimals: Decimals) -> String {
    match decimals {
        Decimals::Auto => {
            if v < 9.995 {
                format!("{v:.2}")
            } else if v < 99.95 {
                format!("{v:.1}")
            } else {
                format!("{v:.0}")
            }
        }
        Decimals::One => format!("{v:.1}"),
        Decimals::Two => format!("{v:.2}"),
    }
}

/// Formats a rate given in bytes per second. Starts at KB/s and climbs to MB/s and GB/s,
/// switching unit before the number reaches four digits so the width stays steady.
pub fn speed(bytes_per_sec: f64, mode: UnitMode, decimals: Decimals) -> Speed {
    let bps = if bytes_per_sec.is_finite() && bytes_per_sec > 0.0 { bytes_per_sec } else { 0.0 };
    let (mut v, base, units) = match mode {
        UnitMode::Bytes => (bps / 1024.0, 1024.0, &BYTE_UNITS),
        UnitMode::Bits => (bps * 8.0 / 1000.0, 1000.0, &BIT_UNITS),
    };
    let mut i = 0;
    while i < units.len() - 1 && v >= 999.5 {
        v /= base;
        i += 1;
    }
    Speed { value: number(v, decimals), unit: units[i] }
}

/// The widest strings `speed` can produce, used to reserve a fixed width.
pub fn widest_values(decimals: Decimals) -> &'static [&'static str] {
    match decimals {
        Decimals::Auto => &["8.88", "88.8", "888"],
        Decimals::One => &["888.8"],
        Decimals::Two => &["888.88"],
    }
}

pub fn units(mode: UnitMode) -> &'static [&'static str] {
    match mode {
        UnitMode::Bytes => &BYTE_UNITS[..3],
        UnitMode::Bits => &BIT_UNITS[..3],
    }
}

/// Splits a byte total into number and unit: (135.5, "GB").
pub fn bytes_parts(n: u64) -> (String, &'static str) {
    const UNITS: [&str; 6] = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut v = n as f64;
    let mut i = 0;
    while i < UNITS.len() - 1 && v >= 999.95 {
        v /= 1024.0;
        i += 1;
    }
    let s = if i == 0 {
        format!("{v:.0}")
    } else if v < 99.995 {
        format!("{v:.2}")
    } else {
        format!("{v:.1}")
    };
    (s, UNITS[i])
}

pub fn bytes(n: u64) -> String {
    let (v, u) = bytes_parts(n);
    format!("{v} {u}")
}

/// Short form for axis labels: 12 MB/s, 800 KB/s.
pub fn speed_short(bytes_per_sec: f64, mode: UnitMode) -> String {
    let s = speed(bytes_per_sec, mode, Decimals::Auto);
    let v: f64 = s.value.parse().unwrap_or(0.0);
    let value = if v >= 10.0 || v == v.trunc() { format!("{v:.0}") } else { format!("{v:.1}") };
    format!("{value} {}", s.unit)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn climbs_through_units() {
        let s = speed(6.35 * 1024.0, UnitMode::Bytes, Decimals::Auto);
        assert_eq!(s.to_string(), "6.35 KB/s");
        let s = speed(1.46 * 1024.0 * 1024.0, UnitMode::Bytes, Decimals::Auto);
        assert_eq!(s.to_string(), "1.46 MB/s");
        let s = speed(2.5 * 1024.0 * 1024.0 * 1024.0, UnitMode::Bytes, Decimals::Auto);
        assert_eq!(s.to_string(), "2.50 GB/s");
    }

    #[test]
    fn never_shows_four_integer_digits() {
        let s = speed(1000.0 * 1024.0, UnitMode::Bytes, Decimals::Auto);
        assert_eq!(s.to_string(), "0.98 MB/s");
        let s = speed(999.0 * 1024.0, UnitMode::Bytes, Decimals::Auto);
        assert_eq!(s.to_string(), "999 KB/s");
    }

    #[test]
    fn auto_decimals_keep_three_digits() {
        assert_eq!(speed(63.5 * 1024.0, UnitMode::Bytes, Decimals::Auto).value, "63.5");
        assert_eq!(speed(9.999 * 1024.0, UnitMode::Bytes, Decimals::Auto).value, "10.0");
        assert_eq!(speed(0.0, UnitMode::Bytes, Decimals::Auto).to_string(), "0.00 KB/s");
        assert_eq!(speed(f64::NAN, UnitMode::Bytes, Decimals::Two).to_string(), "0.00 KB/s");
    }

    #[test]
    fn bits_mode() {
        // 12.5 MB/s (decimal) = 100 Mbps
        assert_eq!(speed(12_500_000.0, UnitMode::Bits, Decimals::Auto).to_string(), "100 Mbps");
        assert_eq!(speed(125_000.0, UnitMode::Bits, Decimals::Auto).to_string(), "1.00 Mbps");
    }

    #[test]
    fn byte_totals() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(4_520_000_000), "4.21 GB");
        assert_eq!(bytes(135_500 * 1024 * 1024), "132.3 GB");
        assert_eq!(bytes(1024 * 1024 * 1024 * 1000), "0.98 TB");
    }
}
