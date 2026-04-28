use ratatui::style::{Modifier, Style};

/// Pulse the selected-row highlight: every 10 frames (~1s) toggle bold on/off.
pub fn pulse(frame: u64, base: Style) -> Style {
    if (frame / 10).is_multiple_of(2) {
        base.add_modifier(Modifier::BOLD)
    } else {
        base
    }
}

/// Ease-in-out cubic tween from `prev` to `curr` at `progress` ∈ [0.0, 1.0].
#[allow(dead_code)]
pub fn count_up(prev: f64, curr: f64, progress: f32) -> f64 {
    let p = progress.clamp(0.0, 1.0) as f64;
    let eased = if p < 0.5 {
        4.0 * p * p * p
    } else {
        1.0 - (-2.0 * p + 2.0_f64).powi(3) / 2.0
    };
    prev + (curr - prev) * eased
}

/// Reveal the first N chars of `s` where N grows by 2 per frame since `started_at`.
/// Returns a valid UTF-8 sub-slice.
#[allow(dead_code)]
pub fn reveal_str(s: &str, frame: u64, started_at: u64) -> &str {
    let elapsed = frame.saturating_sub(started_at);
    let chars_to_show = (elapsed * 2) as usize;
    match s.char_indices().nth(chars_to_show) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_up_is_monotone() {
        let (prev, curr) = (1.0f64, 10.0f64);
        let mut last = prev;
        for i in 0..=10 {
            let v = count_up(prev, curr, i as f32 / 10.0);
            assert!(
                v >= last - 1e-12,
                "count_up not monotone at step {} (prev={}, v={})",
                i,
                last,
                v
            );
            last = v;
        }
        assert!((count_up(prev, curr, 1.0) - curr).abs() < 1e-10);
    }

    #[test]
    fn reveal_str_grows_and_caps() {
        let s = "Hello, World!";
        let mut last_len = 0;
        for frame in 0u64..20 {
            let revealed = reveal_str(s, frame + 5, 5);
            assert!(
                revealed.len() >= last_len,
                "revealed should grow at frame {}",
                frame
            );
            last_len = revealed.len();
        }
        // Eventually returns full string
        assert_eq!(reveal_str(s, 1000, 0), s);
    }
}
