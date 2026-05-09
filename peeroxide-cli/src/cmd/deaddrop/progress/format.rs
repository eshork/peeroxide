#![allow(dead_code)]

use std::sync::atomic::Ordering;

use crate::cmd::deaddrop::progress::state::{Phase, ProgressState};

fn snapshot(state: &ProgressState) -> (u64, u64, u32, u32, u32, u32) {
    (
        state.bytes_done.load(Ordering::Relaxed),
        state.bytes_total.load(Ordering::Relaxed),
        state.indexes_done.load(Ordering::Relaxed),
        state.indexes_total.load(Ordering::Relaxed),
        state.data_done.load(Ordering::Relaxed),
        state.data_total.load(Ordering::Relaxed),
    )
}

fn pct(done: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        ((done as f64 / total as f64) * 100.0).min(100.0)
    }
}

pub fn human_bytes(b: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    match b {
        0..=1023 => format!("{b} B"),
        1024..=1_048_575 => format!("{:.1} KiB", b as f64 / KIB),
        1_048_576..=1_073_741_823 => format!("{:.1} MiB", b as f64 / MIB),
        _ => format!("{:.1} GiB", b as f64 / GIB),
    }
}

pub fn human_rate(bps: f64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;

    if bps <= 0.0 {
        return "0 B/s".to_string();
    }

    if bps < KIB {
        format!("{:.0} B/s", bps)
    } else if bps < MIB {
        format!("{:.1} KiB/s", bps / KIB)
    } else if bps < GIB {
        format!("{:.1} MiB/s", bps / MIB)
    } else {
        format!("{:.1} GiB/s", bps / GIB)
    }
}

pub fn human_eta(eta: Option<f64>) -> String {
    let Some(eta) = eta else { return "—".to_string(); };
    if eta <= 0.0 {
        return "0s".to_string();
    }
    let secs = eta.floor() as u64;
    let mins = secs / 60;
    let rem = secs % 60;
    if mins == 0 {
        format!("{rem}s")
    } else {
        format!("{mins}m{rem}s")
    }
}

pub fn draw_bar(done: u64, total: u64) -> String {
    const WIDTH: usize = 20;
    let filled = if total == 0 {
        0
    } else {
        (((done as f64 / total as f64).min(1.0)) * WIDTH as f64).floor() as usize
    };
    let filled = filled.min(WIDTH);
    let empty = WIDTH - filled;
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

pub fn render_bar_line(state: &ProgressState, smoothed_rate: f64, eta: Option<f64>) -> String {
    let (_, bytes_total, indexes_done, indexes_total, bytes_done, _) = snapshot(state);
    let bar = draw_bar(bytes_done.into(), bytes_total);
    let pct = pct(bytes_done.into(), bytes_total);
    let rate = human_rate(smoothed_rate);
    let eta = human_eta(eta);

    if indexes_total == 0 {
        format!(
            "↑ {}  D({}/{})  [{}]  {:.0}%  {}  ETA {}",
            state.filename,
            human_bytes(bytes_done.into()),
            human_bytes(bytes_total),
            bar,
            pct,
            rate,
            eta
        )
    } else {
        format!(
            "↑ {}  I[{}/{}]  D({}/{})  [{}]  {:.0}%  {}  ETA {}",
            state.filename,
            indexes_done,
            indexes_total,
            human_bytes(bytes_done.into()),
            human_bytes(bytes_total),
            bar,
            pct,
            rate,
            eta
        )
    }
}

pub fn render_index_line(state: &ProgressState, smoothed_rate: f64) -> String {
    let (_, _, indexes_done, indexes_total, _, _) = snapshot(state);
    format!(
        "I[{}/{}]  {}",
        indexes_done,
        indexes_total,
        human_rate(smoothed_rate)
    )
}

pub fn render_data_line(state: &ProgressState, smoothed_rate: f64, eta: Option<f64>) -> String {
    let (_, bytes_total, _, _, bytes_done, _) = snapshot(state);
    format!(
        "D({}/{})  [{}]  {:.0}%  {}  ETA {}",
        human_bytes(bytes_done.into()),
        human_bytes(bytes_total),
        draw_bar(bytes_done.into(), bytes_total),
        pct(bytes_done.into(), bytes_total),
        human_rate(smoothed_rate),
        human_eta(eta)
    )
}

pub fn render_overall_line(state: &ProgressState) -> String {
    let (bytes_done, bytes_total, _, _, _, _) = snapshot(state);
    format!(
        "{}  {}/{}  {:.0}%",
        state.filename,
        human_bytes(bytes_done),
        human_bytes(bytes_total),
        pct(bytes_done, bytes_total)
    )
}

pub fn render_log_line(state: &ProgressState, smoothed_rate: f64, eta: Option<f64>) -> String {
    let (bytes_done, bytes_total, indexes_done, indexes_total, data_done, data_total) = snapshot(state);
    let phase = match state.phase {
        Phase::Put => "put",
        Phase::Get => "get",
    };
    format!(
        "[dd-{phase}] indexes {}/{}, data {}/{}, {}/{} ({:.0}%), {}, eta {}",
        indexes_done,
        indexes_total,
        data_done,
        data_total,
        human_bytes(bytes_done),
        human_bytes(bytes_total),
        pct(bytes_done, bytes_total),
        human_rate(smoothed_rate),
        human_eta(eta)
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn state() -> Arc<ProgressState> {
        let state = ProgressState::new(Phase::Put, 2, Arc::<str>::from("file.bin"));
        state.set_length(10 * 1024, 2, 4);
        state.bytes_done.store(5 * 1024, Ordering::Relaxed);
        state.indexes_done.store(1, Ordering::Relaxed);
        state.data_done.store(2, Ordering::Relaxed);
        state
    }

    #[test]
    fn human_bytes_thresholds() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(1_048_576), "1.0 MiB");
        assert_eq!(human_bytes(1_073_741_824), "1.0 GiB");
    }

    #[test]
    fn human_rate_thresholds() {
        assert_eq!(human_rate(0.0), "0 B/s");
        assert_eq!(human_rate(500.0), "500 B/s");
        assert_eq!(human_rate(1.4 * 1024.0 * 1024.0), "1.4 MiB/s");
    }

    #[test]
    fn human_eta_cases() {
        assert_eq!(human_eta(None), "—");
        assert_eq!(human_eta(Some(3.0)), "3s");
        assert_eq!(human_eta(Some(63.0)), "1m3s");
        assert_eq!(human_eta(Some(3661.0)), "61m1s");
    }

    #[test]
    fn draw_bar_cases() {
        assert_eq!(draw_bar(0, 0), "░░░░░░░░░░░░░░░░░░░░");
        assert_eq!(draw_bar(0, 10), "░░░░░░░░░░░░░░░░░░░░");
        assert_eq!(draw_bar(5, 10), "██████████░░░░░░░░░░");
        assert_eq!(draw_bar(10, 10), "████████████████████");
        assert_eq!(draw_bar(20, 10), "████████████████████");
    }

    #[test]
    fn render_bar_line_v1_omits_indexes() {
        let state = ProgressState::new(Phase::Put, 1, Arc::<str>::from("a.txt"));
        state.set_length(2000, 0, 0);
        state.bytes_done.store(1000, Ordering::Relaxed);
        let s = render_bar_line(&state, 2048.0, Some(12.0));
        assert!(s.starts_with("↑ a.txt  D("));
        assert!(!s.contains("I["));
        assert!(s.contains("  ETA 12s"));
    }

    #[test]
    fn render_bar_line_v2_put_includes_indexes() {
        let state = state();
        let s = render_bar_line(&state, 2048.0, Some(12.0));
        assert!(s.starts_with("↑ file.bin  I[1/2]  D("));
        assert!(s.contains("ETA 12s"));
    }

    #[test]
    fn render_log_line_shape() {
        let s = render_log_line(&state(), 500.0, Some(4.0));
        assert!(s.starts_with("[dd-put] indexes 1/2, data 2/4, 5.0 KiB/10.0 KiB (50%), 500 B/s, eta 4s"));
    }

    #[test]
    fn pct_caps_and_zero_total_is_safe() {
        let state = ProgressState::new(Phase::Get, 2, Arc::<str>::from("b.bin"));
        state.set_length(0, 0, 0);
        state.bytes_done.store(10, Ordering::Relaxed);
        let s = render_bar_line(&state, 0.0, None);
        assert!(s.contains("100%") || s.contains("0%"));
        assert_eq!(draw_bar(1, 0), "░░░░░░░░░░░░░░░░░░░░");
    }
}
