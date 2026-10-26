//! The motion shared by the taskbar widget and the dashboard readouts: numbers roll
//! toward each new reading, busy rows grow on a spring, and colors crossfade
//! between heat tiers.

use crate::config::Heat;
use crate::heat;

/// How much a readout may grow when traffic is very heavy.
pub const MAX_GROW: f32 = 1.3;

/// What one readout (download or upload) looks like right now.
#[derive(Clone, Copy, Debug, Default)]
pub struct RowState {
    /// The number to show, bytes per second (rolls toward the real reading).
    pub speed: f64,
    /// Size multiplier, 1.0 at rest.
    pub scale: f32,
    /// Streak animation clock, in cycles.
    pub phase: f32,
    /// Current text color, 0-255 per channel (crossfades between tier colors).
    pub color: [f32; 3],
    /// 0 when calm, 1 when busy or hotter (fades along with the color).
    pub heat: f32,
}

impl RowState {
    pub fn rgb(&self) -> [u8; 3] {
        [self.color[0].round() as u8, self.color[1].round() as u8, self.color[2].round() as u8]
    }
}

/// Tier color for a speed, or `ink` when calm. Returns (color, hot).
pub fn tier_ink(speed: f64, heat: &Heat, dark: bool, ink: [u8; 3]) -> ([u8; 3], f32) {
    match heat::text_color(speed, heat, dark) {
        Some(c) => (c, 1.0),
        None => (ink, 0.0),
    }
}

/// Rolls a readout toward its target on a log scale (so 10 KB/s to 30 MB/s feels
/// like revving up) and drives its size with a slightly underdamped spring, which
/// gives a small "boost" overshoot when traffic jumps.
#[derive(Default)]
pub struct Track {
    pub row: RowState,
    pub target: f64,
    velocity: f32,
}

impl Track {
    pub fn snap(&mut self, target: f64, heat: &Heat, dark: bool, ink: [u8; 3]) {
        self.target = target;
        self.row.speed = target;
        self.row.scale = 1.0;
        self.velocity = 0.0;
        let (c, hot) = tier_ink(target, heat, dark, ink);
        self.row.color = [c[0] as f32, c[1] as f32, c[2] as f32];
        self.row.heat = hot;
    }

    /// Advances by `dt` seconds. Returns true while anything is still moving.
    pub fn step(&mut self, dt: f32, heat: &Heat, dark: bool, ink: [u8; 3]) -> bool {
        let x = self.row.speed.max(0.0).ln_1p();
        let goal_x = self.target.max(0.0).ln_1p();
        let next = x + (goal_x - x) * (1.0 - (-(dt as f64) / 0.22).exp());
        self.row.speed = if (goal_x - next).abs() < 0.003 { self.target } else { next.exp_m1() };

        let intensity = heat::intensity(self.row.speed, heat);
        let goal = 1.0 + (MAX_GROW - 1.0) * intensity;
        const STIFFNESS: f32 = 160.0;
        let damping = 2.0 * STIFFNESS.sqrt() * 0.55;
        let accel = STIFFNESS * (goal - self.row.scale) - damping * self.velocity;
        self.velocity += accel * dt;
        self.row.scale += self.velocity * dt;
        if (goal - self.row.scale).abs() < 0.0008 && self.velocity.abs() < 0.01 {
            self.row.scale = goal;
            self.velocity = 0.0;
        }
        if intensity > 0.02 {
            self.row.phase = (self.row.phase + dt * (0.6 + 1.9 * intensity)) % 10_000.0;
        }

        // Quick crossfade to the color of the tier the rolling number is in.
        let (goal_color, goal_hot) = tier_ink(self.row.speed, heat, dark, ink);
        let k = 1.0 - (-dt / 0.12).exp();
        self.row.color = heat::lerp_rgb(self.row.color, goal_color, k);
        self.row.heat += (goal_hot - self.row.heat) * k;
        let fading = self.row.color.iter().zip(goal_color).any(|(a, b)| (a - b as f32).abs() > 0.5)
            || (self.row.heat - goal_hot).abs() > 0.004;
        if !fading {
            self.row.color = [goal_color[0] as f32, goal_color[1] as f32, goal_color[2] as f32];
            self.row.heat = goal_hot;
        }

        self.row.speed != self.target || self.velocity != 0.0 || intensity > 0.02 || fading
    }
}
