//! Exit/entry delay math. The ring itself is animated by CSS; this only
//! supplies the starting offset and the seconds-left text.

use crate::alarm::AlarmState;

/// The delay for the current state, as reported by HouseParty.
pub fn delay_for(state: AlarmState, reported: Option<u32>) -> Option<u32> {
    match state {
        AlarmState::Arming | AlarmState::Pending => reported.filter(|d| *d > 0),
        _ => None,
    }
}

/// Whole seconds left, never more than the delay (guards against a client
/// clock that lags the server).
pub fn remaining_secs(started_ms: f64, delay_secs: u32, now_ms: f64) -> u32 {
    if !(started_ms.is_finite() && now_ms.is_finite()) {
        return 0;
    }
    let end = started_ms + f64::from(delay_secs) * 1000.0;
    let left = (end - now_ms) / 1000.0;
    if left <= 0.0 {
        0
    } else {
        left.ceil().min(f64::from(delay_secs)) as u32
    }
}

/// Seconds already elapsed, clamped to `0..=delay`.
pub fn elapsed_secs(started_ms: f64, delay_secs: u32, now_ms: f64) -> f64 {
    if !(started_ms.is_finite() && now_ms.is_finite()) {
        return 0.0;
    }
    ((now_ms - started_ms) / 1000.0).clamp(0.0, f64::from(delay_secs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_down() {
        assert_eq!(remaining_secs(0.0, 30, 0.0), 30);
        assert_eq!(remaining_secs(0.0, 30, 10_500.0), 20);
        assert_eq!(remaining_secs(0.0, 30, 30_000.0), 0);
        assert_eq!(remaining_secs(0.0, 30, 99_000.0), 0);
    }

    #[test]
    fn clock_skew_is_clamped() {
        assert_eq!(remaining_secs(5_000.0, 30, 0.0), 30);
        assert_eq!(elapsed_secs(5_000.0, 30, 0.0), 0.0);
        assert_eq!(elapsed_secs(0.0, 30, 99_000.0), 30.0);
    }

    #[test]
    fn delay_only_in_delay_states() {
        use AlarmState::*;
        assert_eq!(delay_for(Arming, Some(45)), Some(45));
        assert_eq!(delay_for(Pending, Some(20)), Some(20));
        assert_eq!(delay_for(ArmedAway, Some(45)), None);
        assert_eq!(delay_for(Arming, Some(0)), None);
        assert_eq!(delay_for(Arming, None), None);
    }

    #[test]
    fn bad_input_is_zero() {
        assert_eq!(remaining_secs(f64::NAN, 30, 0.0), 0);
    }
}
