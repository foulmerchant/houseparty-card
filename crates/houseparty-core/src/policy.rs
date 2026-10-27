//! Timing policy for things a person at the screen can see or leave behind.

use crate::alarm::{AlarmState, DisplayState, Tone};

/// Abandon a confirm or code entry after this long without a touch, so a
/// typed-but-unsent code isn't left for the next person.
pub const IDLE_TIMEOUT_MS: f64 = 20_000.0;

/// How long "Show details" reveals sensors and who-changed-it in privacy mode.
pub const REVEAL_MS: f64 = 30_000.0;

/// Error banners clear after this long.
pub const ERROR_TTL_MS: f64 = 60_000.0;

/// Number of dots shown during code entry, whatever the code's length, so
/// an onlooker can't learn how long the code is.
pub const MASK_DOTS: usize = 6;

pub fn is_idle(last_input_ms: f64, now_ms: f64) -> bool {
    // Non-finite input counts as idle: fail closed.
    !(last_input_ms.is_finite() && now_ms.is_finite()) || now_ms - last_input_ms >= IDLE_TIMEOUT_MS
}

pub fn error_expired(since_ms: f64, now_ms: f64) -> bool {
    !(since_ms.is_finite() && now_ms.is_finite()) || now_ms - since_ms >= ERROR_TTL_MS
}

pub fn is_revealed(revealed_until_ms: f64, now_ms: f64) -> bool {
    revealed_until_ms.is_finite() && now_ms.is_finite() && now_ms < revealed_until_ms
}

// ---- idle screensaver ------------------------------------------------------

pub const IDLE_AFTER_DEFAULT_S: u32 = 120;
pub const ROTATE_DEFAULT_S: u32 = 60;
/// After waking, ignore taps on controls this long: a tap that wakes the
/// screen must never also arm or disarm.
pub const WAKE_GUARD_MS: f64 = 600.0;

/// The screensaver may show only when nothing needs attention: a proven-live
/// link showing disarmed or fully armed, and `calm` (no error, nothing
/// pending, no code entry, readable data). Anything else keeps it awake.
pub fn screensaver_allowed(display: DisplayState, calm: bool) -> bool {
    calm && matches!(display, DisplayState::Live(s) if s == AlarmState::Disarmed || s.tone() == Tone::Armed)
}

pub fn should_idle(last_activity_ms: f64, now_ms: f64, after_s: u32) -> bool {
    last_activity_ms.is_finite()
        && now_ms.is_finite()
        && now_ms - last_activity_ms >= f64::from(after_s) * 1000.0
}

/// A different item from `current` (when there's more than one). `r` in [0, 1).
pub fn next_media(current: usize, len: usize, r: f64) -> usize {
    if len <= 1 {
        return 0;
    }
    let r = if r.is_finite() {
        r.clamp(0.0, 0.999_999)
    } else {
        0.0
    };
    let pick = (r * (len - 1) as f64) as usize;
    if pick >= current.min(len - 1) {
        pick + 1
    } else {
        pick
    }
}

/// Video plays hardware-decoded and is far cheaper than an animated GIF.
pub fn is_video(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.ends_with(".mp4") || p.ends_with(".webm")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle() {
        assert!(!is_idle(0.0, IDLE_TIMEOUT_MS - 1.0));
        assert!(is_idle(0.0, IDLE_TIMEOUT_MS));
        assert!(is_idle(f64::NAN, 0.0));
    }

    #[test]
    fn reveal_expires() {
        assert!(is_revealed(1_000.0, 999.0));
        assert!(!is_revealed(1_000.0, 1_000.0));
        assert!(!is_revealed(f64::NAN, 0.0));
    }

    #[test]
    fn errors_expire() {
        assert!(!error_expired(0.0, ERROR_TTL_MS - 1.0));
        assert!(error_expired(0.0, ERROR_TTL_MS));
        assert!(error_expired(f64::NAN, 0.0));
    }

    #[test]
    fn screensaver_only_when_calm_and_settled() {
        use crate::alarm::AlarmState::*;
        let live = |s| DisplayState::Live(s);
        assert!(screensaver_allowed(live(Disarmed), true));
        assert!(screensaver_allowed(live(ArmedAway), true));
        assert!(!screensaver_allowed(live(ArmedAway), false));
        for s in [Arming, Pending, Triggered, Disarming, Unknown, Unavailable] {
            assert!(!screensaver_allowed(live(s), true), "{s:?}");
        }
        assert!(!screensaver_allowed(DisplayState::ConnectionLost, true));
        assert!(!screensaver_allowed(DisplayState::Checking, true));
        assert!(!screensaver_allowed(DisplayState::Missing, true));
    }

    #[test]
    fn idle_timing() {
        assert!(!should_idle(0.0, 119_999.0, 120));
        assert!(should_idle(0.0, 120_000.0, 120));
        assert!(!should_idle(f64::NAN, 1e9, 120));
    }

    #[test]
    fn rotation_never_repeats() {
        for cur in 0..4 {
            for r in [0.0, 0.3, 0.6, 0.999] {
                let n = next_media(cur, 4, r);
                assert!(n < 4 && n != cur, "cur {cur} r {r} -> {n}");
            }
        }
        assert_eq!(next_media(0, 1, 0.5), 0);
        assert!(next_media(1, 2, f64::NAN) < 2);
    }

    #[test]
    fn video_detection() {
        assert!(is_video("/local/houseparty/idle/Cat.MP4"));
        assert!(!is_video("/local/houseparty/idle/cat.gif"));
    }
}
