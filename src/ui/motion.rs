//! Shared, low-cost motion primitives for the terminal renderer.
//!
//! XTUI deliberately avoids a timer task or animation dependency. The event
//! loop samples this clock and repaints only when the quantized frame changes.
//! Every effect is derived from the same phase, which makes the interface feel
//! orchestrated instead of like a collection of unrelated spinners.

use std::{env, sync::OnceLock, time::Instant};

static START: OnceLock<Instant> = OnceLock::new();
static ENABLED: OnceLock<bool> = OnceLock::new();

/// Whether ambient motion is enabled for this process.
///
/// `XTUI_REDUCED_MOTION=1` and `XTUI_MOTION=off` both freeze decorative
/// effects on their most legible frame. Request spinners remain functional
/// because they communicate real application state rather than decoration.
pub(super) fn enabled() -> bool {
    *ENABLED.get_or_init(|| {
        let reduced = env::var("XTUI_REDUCED_MOTION")
            .ok()
            .is_some_and(|value| truthy(&value));
        let explicitly_disabled = env::var("XTUI_MOTION").ok().is_some_and(|value| {
            matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "0" | "off" | "false" | "reduced" | "none"
            )
        });
        !reduced && !explicitly_disabled
    })
}

fn truthy(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "on" | "true" | "yes"
    )
}

/// Seconds elapsed on XTUI's shared animation clock.
pub(super) fn phase() -> f32 {
    if enabled() {
        START.get_or_init(Instant::now).elapsed().as_secs_f32()
    } else {
        0.0
    }
}

/// A quantized frame number suitable for deciding whether to repaint.
pub(super) fn frame(fps: f32) -> u64 {
    (phase() * fps.max(1.0)) as u64
}

/// Smooth 0 → 1 → 0 pulse with a configurable period and phase offset.
pub(super) fn pulse(period: f32, offset: f32) -> f32 {
    let angle = ((phase() + offset) / period.max(0.01)) * std::f32::consts::TAU;
    0.5 - 0.5 * angle.cos()
}

/// A repeating linear sweep within the `0..len` cell range.
pub(super) fn sweep(len: usize, period: f32, offset: f32) -> usize {
    if len <= 1 {
        return 0;
    }
    let progress = ((phase() + offset) / period.max(0.01)).rem_euclid(1.0);
    (progress * (len - 1) as f32).round() as usize
}

/// Circular distance between two normalized phase positions.
#[allow(dead_code)]
pub(super) fn loop_distance(a: f32, b: f32) -> f32 {
    let direct = (a - b).abs();
    direct.min(1.0 - direct)
}

/// Interpolate a grayscale intensity without overflowing at the edges.
pub(super) fn luma(from: u8, to: u8, amount: f32) -> u8 {
    let amount = amount.clamp(0.0, 1.0);
    (f32::from(from) + (f32::from(to) - f32::from(from)) * amount).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn luma_interpolates_and_clamps() {
        assert_eq!(luma(20, 220, 0.0), 20);
        assert_eq!(luma(20, 220, 0.5), 120);
        assert_eq!(luma(20, 220, 1.0), 220);
        assert_eq!(luma(20, 220, 2.0), 220);
    }

    #[test]
    fn loop_distance_wraps_at_the_seam() {
        assert!((loop_distance(0.98, 0.02) - 0.04).abs() < 0.001);
        assert!((loop_distance(0.2, 0.7) - 0.5).abs() < 0.001);
    }
}
