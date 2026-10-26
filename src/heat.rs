//! The heat scale: how speed and volume map to color, shared by the taskbar widget
//! and the dashboard so a "heavy" orange means the same thing everywhere.

use crate::config::Heat;

pub type Rgb = [u8; 3];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    Calm,
    Busy,
    Heavy,
    Extreme,
}

pub fn tier(bytes_per_sec: f64, h: &Heat) -> Tier {
    if !h.enabled || bytes_per_sec < h.busy {
        Tier::Calm
    } else if bytes_per_sec < h.heavy {
        Tier::Busy
    } else if bytes_per_sec < h.extreme {
        Tier::Heavy
    } else {
        Tier::Extreme
    }
}

pub struct Ramp {
    pub calm: Rgb,
    pub busy: Rgb,
    pub heavy: Rgb,
    pub extreme: Rgb,
}

/// Colors tuned for a dark surface (the default Windows 11 dark taskbar).
pub const DARK: Ramp = Ramp {
    calm: [124, 138, 156],
    busy: [76, 194, 255],
    heavy: [255, 200, 61],
    extreme: [255, 107, 94],
};

/// Darker variants that keep contrast on a light taskbar.
pub const LIGHT: Ramp = Ramp {
    calm: [128, 141, 158],
    busy: [0, 103, 192],
    heavy: [166, 99, 0],
    extreme: [196, 43, 28],
};

pub fn ramp(dark: bool) -> &'static Ramp {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// Discrete color for text. `None` means "use the normal text color".
pub fn text_color(bytes_per_sec: f64, h: &Heat, dark: bool) -> Option<Rgb> {
    let r = ramp(dark);
    match tier(bytes_per_sec, h) {
        Tier::Calm => None,
        Tier::Busy => Some(r.busy),
        Tier::Heavy => Some(r.heavy),
        Tier::Extreme => Some(r.extreme),
    }
}

/// How hard the connection is working: 0 below "busy", 1 at "very heavy".
pub fn intensity(bytes_per_sec: f64, h: &Heat) -> f32 {
    if bytes_per_sec <= h.busy {
        return 0.0;
    }
    ((bytes_per_sec / h.busy).ln() / (h.extreme / h.busy).ln()).clamp(0.0, 1.0) as f32
}

fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    [m(a[0], b[0]), m(a[1], b[1]), m(a[2], b[2])]
}

/// Chart color for a speed: one clean color per tier. (Blending blue into amber
/// passes through a muddy olive, so tiers stay discrete.)
pub fn speed_color(bytes_per_sec: f64, h: &Heat, dark: bool) -> Rgb {
    let r = ramp(dark);
    if !h.enabled {
        return r.busy;
    }
    match tier(bytes_per_sec, h) {
        Tier::Calm => r.calm,
        Tier::Busy => r.busy,
        Tier::Heavy => r.heavy,
        Tier::Extreme => r.extreme,
    }
}

/// Color for a total relative to the biggest one in view (0.0 to 1.0), for daily and
/// monthly volumes where speed thresholds don't apply. Same four colors as speeds.
pub fn share_color(t: f64, dark: bool) -> Rgb {
    let r = ramp(dark);
    if t < 0.15 {
        r.calm
    } else if t < 0.5 {
        r.busy
    } else if t < 0.8 {
        r.heavy
    } else {
        r.extreme
    }
}

/// Linear blend, used for short crossfades between tier colors.
pub fn lerp_rgb(a: [f32; 3], b: Rgb, t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] as f32 - a[0]) * t,
        a[1] + (b[1] as f32 - a[1]) * t,
        a[2] + (b[2] as f32 - a[2]) * t,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::MB;

    #[test]
    fn tiers_follow_thresholds() {
        let h = Heat::default();
        assert_eq!(tier(10.0 * 1024.0, &h), Tier::Calm);
        assert_eq!(tier(2.0 * MB, &h), Tier::Busy);
        assert_eq!(tier(20.0 * MB, &h), Tier::Heavy);
        assert_eq!(tier(80.0 * MB, &h), Tier::Extreme);
        let off = Heat { enabled: false, ..h };
        assert_eq!(tier(80.0 * MB, &off), Tier::Calm);
    }

    #[test]
    fn chart_colors_are_tier_colors() {
        let h = Heat::default();
        assert_eq!(speed_color(1.0, &h, true), DARK.calm);
        assert_eq!(speed_color(h.busy, &h, true), DARK.busy);
        assert_eq!(speed_color(h.heavy * 1.5, &h, true), DARK.heavy);
        assert_eq!(speed_color(h.extreme * 4.0, &h, true), DARK.extreme);
    }
}
