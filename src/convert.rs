// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Turning a recording into a GIF: replaying it through a virtual terminal
//! and rendering frames at the (adjusted) event times.

use std::fmt;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::Result;
use avt::Terminal;

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

/// Maps recording times to GIF times: idle time limit, then speed. Each
/// time depends only on the earlier ones, so this works during a recording.
struct Clock {
    prev: f64,
    acc: f64,
}

impl Clock {
    fn new() -> Self {
        Clock {
            prev: 0.0,
            acc: 0.0,
        }
    }

    fn adjust(&mut self, t: f64, timing: &Timing) -> f64 {
        let mut delta = (t - self.prev).max(0.0);
        if let Some(limit) = timing.idle_time_limit {
            delta = delta.min(limit);
        }
        self.prev = t;
        self.acc += delta;
        self.acc / timing.speed
    }
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

/// Turns events into GIF frames as they come, so a recording can be
/// rendered while it is still going on.
pub struct Converter<'a, W: Write> {
    renderer: &'a mut Renderer,
    timing: &'a Timing,
    gif: GifWriter<W>,
    canvas: Canvas,
    vt: Terminal,
    clock: Clock,
    /// Start of the frame that collects the latest events.
    frame_start: Option<f64>,
    /// The largest terminal size that fits on the canvas.
    max_size: (usize, usize),
    width: u32,
    height: u32,
}

impl<'a, W: Write> Converter<'a, W> {
    /// Starts a GIF of a terminal of `size` (columns, rows), on a canvas for
    /// a terminal of `max_size`. A terminal that changes size is drawn at the
    /// top left of the canvas.
    pub fn new(
        renderer: &'a mut Renderer,
        timing: &'a Timing,
        size: (usize, usize),
        max_size: (usize, usize),
        repeat: bool,
        out: W,
    ) -> Result<Self> {
        let max_size = (max_size.0.max(size.0), max_size.1.max(size.1));
        let (width, height) = renderer.canvas_size(max_size.0, max_size.1);
        let mut gif = GifWriter::new(out, width, height, repeat)?;
        let mut canvas = Canvas::new(width, height);
        let vt = Terminal::new(size.0, size.1)?;
        renderer.render(&vt, &mut canvas);
        gif.push(&canvas, 0)?;
        Ok(Converter {
            renderer,
            timing,
            gif,
            canvas,
            vt,
            clock: Clock::new(),
            frame_start: None,
            max_size,
            width,
            height,
        })
    }

    /// Whether a terminal of `cols` x `rows` fits on the canvas.
    pub fn fits(&self, cols: usize, rows: usize) -> bool {
        cols <= self.max_size.0 && rows <= self.max_size.1
    }

    /// Adds an event at recording time `t` (seconds, not decreasing). A
    /// resize must [fit](Self::fits).
    pub fn event(&mut self, t: f64, event: &Event) -> Result<()> {
        let t = self.clock.adjust(t, self.timing);
        let interval = 1.0 / f64::from(self.timing.fps_cap);
        // Events closer together than the frame interval go into one frame,
        // shown from the time of the first of them.
        if let Some(start) = self.frame_start {
            if t >= start + interval {
                self.renderer.render(&self.vt, &mut self.canvas);
                self.gif.push(&self.canvas, centiseconds(start))?;
                self.frame_start = None;
            }
        }
        self.frame_start.get_or_insert(t);

        match event {
            Event::Output(data) => {
                self.vt.feed(data);
            }
            Event::Resize(cols, rows) => {
                debug_assert!(self.fits(*cols, *rows));
                self.vt.resize(*cols, *rows);
            }
        }
        Ok(())
    }

    /// Writes the last frame and returns the output.
    pub fn finish(mut self) -> Result<(W, Summary)> {
        if let Some(start) = self.frame_start {
            self.renderer.render(&self.vt, &mut self.canvas);
            self.gif.push(&self.canvas, centiseconds(start))?;
        }
        let (out, frames) = self
            .gif
            .finish(centiseconds(self.timing.last_frame_duration))?;
        Ok((
            out,
            Summary {
                width: self.width,
                height: self.height,
                frames,
            },
        ))
    }
}

/// Renders `rec` as a GIF, on a canvas as large as the largest terminal size
/// during the recording. Setting `interrupted` stops it with an
/// [`Interrupted`] error.
pub fn to_gif<W: Write>(
    rec: &Recording,
    renderer: &mut Renderer,
    timing: &Timing,
    repeat: bool,
    out: W,
    progress: &mut Progress,
    interrupted: &AtomicBool,
) -> Result<(W, Summary)> {
    let mut conv = Converter::new(
        renderer,
        timing,
        (rec.cols, rec.rows),
        rec.max_size(),
        repeat,
        out,
    )?;
    let total = rec.events.len();
    for (i, (t, event)) in rec.events.iter().enumerate() {
        if interrupted.load(Ordering::Relaxed) {
            return Err(Interrupted.into());
        }
        progress.update(i, total);
        conv.event(*t, event)?;
    }
    let result = conv.finish()?;
    progress.finish();
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_limit_and_speed() {
        let timing = Timing {
            speed: 2.0,
            idle_time_limit: Some(1.0),
            fps_cap: 30,
            last_frame_duration: 1.0,
        };
        let mut clock = Clock::new();
        let times: Vec<f64> = [0.5, 10.0, 10.5]
            .iter()
            .map(|&t| clock.adjust(t, &timing))
            .collect();
        assert_eq!(times, vec![0.25, 0.75, 1.0]);
    }
}
