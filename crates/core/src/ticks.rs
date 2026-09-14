pub fn ticks_to_seconds(ticks: i64) -> f64 {
    ticks as f64 / 10_000_000.0
}

pub fn seconds_to_ticks(seconds: f64) -> i64 {
    (seconds * 10_000_000.0).round() as i64
}

/// MPRIS speaks microseconds; a tick is 100 ns.
pub fn ticks_to_micros(ticks: i64) -> i64 {
    ticks / 10
}

pub fn micros_to_ticks(micros: i64) -> i64 {
    micros.saturating_mul(10)
}

/// `hh:mm:ss`, dropping the hours when there are none.
pub fn format_hms(ticks: i64) -> String {
    let total = ticks_to_seconds(ticks).max(0.0).round() as u64;
    let (hours, minutes, seconds) = (total / 3600, (total % 3600) / 60, total % 60);
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Prefer a live mpv sample, but never replace a known position with 0/missing.
/// A Stopped POST of 0 wipes Jellyfin's resume point (`UpdatePlayState`).
pub fn coalesce_position_ticks(live_seconds: Option<f64>, last_ticks: i64) -> i64 {
    live_seconds
        .filter(|s| s.is_finite() && *s > 0.0)
        .map(seconds_to_ticks)
        .unwrap_or(last_ticks)
}

#[cfg(test)]
#[path = "ticks_test.rs"]
mod tests;
