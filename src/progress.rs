// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! A progress bar on stderr for rendering long recordings.
//!
//! It only appears once the work has taken a moment, so short recordings
//! render without it, and only when stderr is a terminal.

use std::io::{self, IsTerminal, Write};
use std::time::{Duration, Instant};

/// Time before the bar appears.
const DELAY: Duration = Duration::from_secs(1);
/// Minimum time between redraws.
const INTERVAL: Duration = Duration::from_millis(100);

pub struct Progress {
    enabled: bool,
    start: Instant,
    last_draw: Option<Instant>,
    /// Characters of the line currently shown, to clear it again.
    shown: usize,
}

impl Progress {
    /// A progress bar on stderr, if `enabled` and stderr is a terminal.
    pub fn new(enabled: bool) -> Self {
        Progress {
            enabled: enabled && io::stderr().is_terminal(),
            start: Instant::now(),
            last_draw: None,
            shown: 0,
        }
    }

    /// Reports that `done` of `total` steps are complete.
    pub fn update(&mut self, done: usize, total: usize) {
        if !self.enabled || total == 0 {
            return;
        }
        let now = Instant::now();
        let elapsed = now - self.start;
        if elapsed < DELAY || self.last_draw.is_some_and(|t| now - t < INTERVAL) {
            return;
        }
        self.last_draw = Some(now);
        let width = match crossterm::terminal::size() {
            Ok((cols, _)) if cols > 0 => usize::from(cols),
            _ => 80,
        };
        let line = line(done, total, elapsed, width);
        self.draw(&line);
    }

    /// Removes the bar, if it is shown.
    pub fn finish(&mut self) {
        if self.shown > 0 {
            self.draw("");
        }
    }

    fn draw(&mut self, line: &str) {
        let len = line.chars().count();
        let pad = self.shown.saturating_sub(len);
        let mut err = io::stderr().lock();
        // Errors writing to the terminal only cost the progress display.
        let _ = write!(err, "\r{line}{:pad$}\r", "");
        let _ = err.flush();
        self.shown = len;
    }
}

impl Drop for Progress {
    fn drop(&mut self) {
        self.finish();
    }
}

/// The progress line, e.g. `rendering  42% [========>          ] 0:07 left`,
/// at most `width - 1` characters (a full line would wrap in some terminals).
#[allow(clippy::cast_precision_loss)] // step counts far below 2^52
fn line(done: usize, total: usize, elapsed: Duration, width: usize) -> String {
    let done = done.min(total);
    let fraction = done as f64 / total as f64;
    let left = if done > 0 {
        let secs = elapsed.as_secs_f64() * (1.0 - fraction) / fraction;
        format_duration(secs)
    } else {
        "--:--".to_owned()
    };
    let head = format!("term2gif: rendering {:3.0}% ", fraction * 100.0);
    let tail = format!(" {left} left");
    // Brackets plus at least 10 characters of bar, or no bar at all.
    let room = width
        .saturating_sub(1 + head.len() + tail.len() + 2)
        .min(50);
    if room < 10 {
        return format!("{}{}", head.trim_end(), tail);
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0..=room
    let filled = (fraction * room as f64).floor() as usize;
    let bar = if filled == room {
        "=".repeat(room)
    } else {
        format!("{}>{}", "=".repeat(filled), " ".repeat(room - filled - 1))
    };
    format!("{head}[{bar}]{tail}")
}

/// Formats seconds as `m:ss`, or `h:mm:ss` from an hour on.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // rounded, non-negative
fn format_duration(secs: f64) -> String {
    let secs = secs.max(0.0).round() as u64;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bar_fits_the_width() {
        assert_eq!(
            line(1, 4, Duration::from_secs(10), 60),
            "term2gif: rendering  25% [=====>                ] 0:30 left"
        );
        assert_eq!(line(1, 4, Duration::from_secs(10), 80).len(), 79);
        // The bar is at most 50 characters wide.
        assert_eq!(line(1, 4, Duration::from_secs(10), 200).len(), 25 + 52 + 10);
        assert!(line(4, 4, Duration::from_secs(1), 80).contains("=]"));
        // Too narrow for a bar.
        assert_eq!(
            line(0, 4, Duration::from_secs(1), 30),
            "term2gif: rendering   0% --:-- left"
        );
    }

    #[test]
    fn durations() {
        assert_eq!(format_duration(0.4), "0:00");
        assert_eq!(format_duration(75.0), "1:15");
        assert_eq!(format_duration(3725.0), "1:02:05");
    }
}
