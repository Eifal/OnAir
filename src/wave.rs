//! Pure helpers for the animated `wave` visual style. No Win32 here so the
//! level math and the speaking/quiet transition can be unit tested.

/// Anything at or above this is treated as full-scale for the bar height.
/// Speech rarely reaches 0 dBFS; -10 dB gives a usable range.
pub const CEIL_DB: f32 = -10.0;
/// Per-frame smoothing when the level rises.
pub const ATTACK: f32 = 0.6;
/// Per-frame smoothing when the level falls (slower for a natural tail).
pub const RELEASE: f32 = 0.15;
/// Smoothing never reaches exactly zero; below this the bars are considered
/// back at their minimum height and the timer may stop.
pub const MIN_SILENT_LEVEL: f32 = 0.01;
/// Oscillation speed of the per-bar shape.
pub const WAVE_HZ: f32 = 2.5;
/// Phase offset between neighbouring bars.
pub const BAR_PHASE: f32 = 0.9;
/// Shape range: 0.55x .. 1.0x of the available height.
pub const SHAPE_BASE: f32 = 0.55;
pub const SHAPE_AMP: f32 = 0.45;
/// Bar band as a fraction of the overlay size (shared with the renderer).
pub const INNER_W: f32 = 0.76;
pub const INNER_H: f32 = 0.76;
/// Tallest bar as a fraction of the inner height.
pub const MAX_H: f32 = 0.80;

const TAU: f32 = std::f32::consts::TAU;

/// Linear amplitude from Voicemeeter to dBFS. A floor keeps log10 finite.
pub fn amp_to_db(amp: f32) -> f32 {
    let a = if amp.is_finite() { amp.max(1e-6) } else { 1e-6 };
    20.0 * a.log10()
}

/// Map a level to 0..1 with `threshold` as silence and `CEIL_DB` as full.
/// A threshold at or above the ceiling disables the animation.
pub fn normalize_db(db: f32, threshold_db: f32) -> f32 {
    let span = CEIL_DB - threshold_db;
    if span <= 0.0 {
        return 0.0;
    }
    ((db - threshold_db) / span).clamp(0.0, 1.0)
}

/// One EMA step. Attack and release differ so speech starts immediately and
/// fades out gently.
pub fn smooth_level(current: f32, target: f32) -> f32 {
    let ratio = if target > current { ATTACK } else { RELEASE };
    (current + (target - current) * ratio).clamp(0.0, 1.0)
}

/// Height multiplier for bar `index` at time `seconds`.
pub fn bar_shape(index: usize, seconds: f32) -> f32 {
    SHAPE_BASE + SHAPE_AMP * (TAU * WAVE_HZ * seconds + index as f32 * BAR_PHASE).sin()
}

/// Height in pixels of bar `index`: the minimum (a dot) at silence, growing
/// towards `max_h` with the level and the oscillating shape. Never exceeds
/// the band because `bar_shape` stays inside `SHAPE_BASE..=1`.
pub fn bar_height(index: usize, level: f32, seconds: f32, min_h: f32, max_h: f32) -> f32 {
    let level = level.clamp(0.0, 1.0);
    (min_h + (max_h - min_h) * level * bar_shape(index, seconds)).clamp(min_h, max_h)
}

/// Runtime state while the animation timer is active.
#[derive(Debug, Clone, Copy)]
pub struct Speaking {
    pub level: f32,
    pub phase: f32,
    below_ms: u32,
}

impl Speaking {
    pub fn new(level: f32) -> Self {
        Self {
            level: level.clamp(0.0, 1.0),
            phase: 0.0,
            below_ms: 0,
        }
    }

    /// Advance one animation tick. Returns false once the sound has been
    /// below the threshold for `release_ms` and the smoothed bars have
    /// returned to their minimum, meaning the timer can stop.
    pub fn tick(&mut self, target: f32, dt_ms: u32, release_ms: u32) -> bool {
        self.level = smooth_level(self.level, target.clamp(0.0, 1.0));
        self.phase = (self.phase + dt_ms as f32 / 1000.0).rem_euclid(1000.0);
        if target <= 0.0 {
            self.below_ms = self.below_ms.saturating_add(dt_ms);
        } else {
            self.below_ms = 0;
        }
        !(self.below_ms >= release_ms && self.level <= MIN_SILENT_LEVEL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.001
    }

    #[test]
    fn amplitude_conversion_is_logarithmic_with_a_floor() {
        assert!(close(amp_to_db(1.0), 0.0));
        assert!(close(amp_to_db(0.1), -20.0));
        assert!(close(amp_to_db(0.0), -120.0));
        assert!(close(amp_to_db(f32::NAN), -120.0));
    }

    #[test]
    fn normalization_uses_threshold_and_ceiling() {
        assert!(close(normalize_db(-45.0, -45.0), 0.0));
        assert!(close(normalize_db(-80.0, -45.0), 0.0));
        assert!(close(normalize_db(-10.0, -45.0), 1.0));
        assert!(close(normalize_db(0.0, -45.0), 1.0));
        // Midpoint between -45 and -10 is -27.5.
        assert!(close(normalize_db(-27.5, -45.0), 0.5));
        // A nonsensical threshold cannot divide by zero.
        assert_eq!(normalize_db(0.0, -10.0), 0.0);
    }

    #[test]
    fn smoothing_attacks_faster_than_it_releases() {
        let up = smooth_level(0.0, 1.0);
        let down = smooth_level(1.0, 0.0);
        assert!(close(up, ATTACK));
        assert!(close(down, 1.0 - RELEASE));
        assert!(up > 1.0 - down);
        assert_eq!(smooth_level(0.5, 0.5), 0.5);
    }

    #[test]
    fn bar_shape_stays_inside_its_band_and_neighbours_differ() {
        for i in 0..9 {
            for step in 0..200 {
                let s = bar_shape(i, step as f32 * 0.01);
                assert!((SHAPE_BASE - SHAPE_AMP..=SHAPE_BASE + SHAPE_AMP).contains(&s));
            }
        }
        assert_ne!(bar_shape(0, 0.0), bar_shape(1, 0.0));
    }

    #[test]
    fn speaking_waits_for_release_and_returns_to_minimum() {
        let mut s = Speaking::new(1.0);
        // Full level: always continues and resets the quiet timer.
        assert!(s.tick(1.0, 33, 300));
        // Drop to silence. Bars fade with 0.15 per frame at 30 fps.
        let mut ticks = 0;
        let mut alive = true;
        while alive && ticks < 200 {
            alive = s.tick(0.0, 33, 300);
            ticks += 1;
        }
        assert!(!alive);
        assert!(s.level <= MIN_SILENT_LEVEL);
        // Release (300 ms = ~10 frames) ends first; the fade to 1% needs
        // about 29 frames at 30 fps.
        assert!((25..=40).contains(&ticks));
    }

    #[test]
    fn speaking_stops_immediately_when_release_is_very_short() {
        let mut s = Speaking::new(0.0);
        assert!(!s.tick(0.0, 100, 100));
    }

    #[test]
    fn speaking_keeps_phase_bounded() {
        let mut s = Speaking::new(1.0);
        for _ in 0..100_000 {
            s.tick(1.0, 33, 300);
        }
        assert!(s.phase < 1000.0);
    }
}
