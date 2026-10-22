// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Turning a recording into a GIF: replaying it through a virtual terminal
//! and rendering frames at the (adjusted) event times.

use std::fmt;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use avt::Vt;

use crate::cast::{Event, Recording};
use crate::encode::GifWriter;
use crate::progress::Progress;
use crate::render::{Canvas, Renderer};

pub struct Timing {
    pub speed: f64,
    /// Pauses longer than this many seconds are shortened to it.
    pub idle_time_limit: Option<f64>,
    pub fps_cap: u32,
    pub last_frame_duration: f64,
}

/// Maps recording times to GIF times: idle time limit, then speed.
fn adjusted_times(rec: &Recording, timing: &Timing) -> Vec<f64> {
    let mut prev = 0.0;
    let mut acc = 0.0;
    rec.events
        .iter()
        .map(|(t, _)| {
            let mut delta = (t - prev).max(0.0);
            if let Some(limit) = timing.idle_time_limit {
                delta = delta.min(limit);
            }
            prev = *t;
            acc += delta;
            acc / timing.speed
        })
        .collect()
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // clamped to u32
fn centiseconds(t: f64) -> u32 {
    (t * 100.0).round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// The error of a rendering stopped because `interrupted` was set.
#[derive(Debug)]
pub struct Interrupted;

impl fmt::Display for Interrupted {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("interrupted")
    }
}

impl std::error::Error for Interrupted {}

pub struct Summary {
    pub width: u32,
    pub height: u32,
    pub frames: usize,
}

/// Renders `rec` as a GIF. A terminal that changes size is drawn at the top
/// left of a canvas as large as the largest size during the recording.
/// Setting `interrupted` stops it with an [`Interrupted`] error.
pub fn to_gif<W: Write>(
    rec: &Recording,
    renderer: &mut Renderer,
    timing: &Timing,
    repeat: bool,
    out: W,
    progress: &mut Progress,
    interrupted: &AtomicBool,
) -> Result<(W, Summary)> {
    let (max_cols, max_rows) = rec.max_size();
    let (width, height) = renderer.canvas_size(max_cols, max_rows);
    let mut gif = GifWriter::new(out, width, height, repeat)?;
    let mut canvas = Canvas::new(width, height);

    let mut vt = Vt::builder()
        .size(rec.cols, rec.rows)
        .scrollback_limit(0)
        .build();
    let times = adjusted_times(rec, timing);
    let interval = 1.0 / f64::from(timing.fps_cap);

    renderer.render(&vt, &mut canvas);
    gif.push(&canvas, 0)?;

    // Events closer together than the frame interval go into one frame,
    // shown from the time of the first of them.
    let mut frame_start: Option<f64> = None;
    let total = rec.events.len();
    for (i, ((_, event), &t)) in rec.events.iter().zip(&times).enumerate() {
        if interrupted.load(Ordering::Relaxed) {
            return Err(Interrupted.into());
        }
        progress.update(i, total);
        if let Some(start) = frame_start {
            if t >= start + interval {
                renderer.render(&vt, &mut canvas);
                gif.push(&canvas, centiseconds(start))?;
                frame_start = None;
            }
        }
        frame_start.get_or_insert(t);

        match event {
            Event::Output(data) => {
                vt.feed_str(data);
            }
            Event::Resize(cols, rows) => {
                vt.resize(*cols, *rows);
            }
        }
    }
    if let Some(start) = frame_start {
        renderer.render(&vt, &mut canvas);
        gif.push(&canvas, centiseconds(start))?;
    }

    let (out, frames) = gif.finish(centiseconds(timing.last_frame_duration))?;
    progress.finish();
    Ok((
        out,
        Summary {
            width,
            height,
            frames,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(times: &[f64]) -> Recording {
        Recording {
            cols: 80,
            rows: 24,
            events: times
                .iter()
                .map(|&t| (t, Event::Output(String::new())))
                .collect(),
        }
    }

    #[test]
    fn idle_limit_and_speed() {
        let timing = Timing {
            speed: 2.0,
            idle_time_limit: Some(1.0),
            fps_cap: 30,
            last_frame_duration: 1.0,
        };
        let times = adjusted_times(&rec(&[0.5, 10.0, 10.5]), &timing);
        assert_eq!(times, vec![0.25, 0.75, 1.0]);
    }
}
