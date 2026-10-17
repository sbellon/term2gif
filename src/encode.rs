//! GIF encoding of a sequence of canvases.
//!
//! Each frame stores only the rectangle that changed since the previous one,
//! with unchanged pixels inside it transparent. Every frame has its own
//! palette: exact when the rectangle has at most 255 colors (the usual case
//! for terminal content), otherwise the 255 most frequent colors with all other
//! colors mapped to the nearest of those, so the dominant theme colors always
//! stay exact.

// Pixel and font geometry mixes integer types and floats throughout; the
// values are small (cell and image sizes), so these conversions are intended.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::borrow::Cow;
use std::collections::HashMap;
use std::io::Write;

use anyhow::{Context, Result};
use gif::{DisposalMethod, Encoder, Frame, Repeat};

use crate::render::{Canvas, unpack};

/// GIF delays are in centiseconds; most viewers treat delays below 2 as 10.
pub const MIN_DELAY: u32 = 2;

pub struct GifWriter<W: Write> {
    encoder: Encoder<W>,
    /// What the viewer shows after the frames written so far.
    shown: Option<Canvas>,
    /// The latest canvas and its start time, written once its duration is
    /// known.
    held: Option<(Canvas, u32)>,
    frames: usize,
}

impl<W: Write> GifWriter<W> {
    pub fn new(out: W, width: u32, height: u32, repeat: bool) -> Result<Self> {
        let (w, h) = (dimension(width)?, dimension(height)?);
        let mut encoder = Encoder::new(out, w, h, &[]).context("cannot write the GIF")?;
        if repeat {
            encoder
                .set_repeat(Repeat::Infinite)
                .context("cannot write the GIF")?;
        }
        Ok(GifWriter {
            encoder,
            shown: None,
            held: None,
            frames: 0,
        })
    }

    /// Adds `canvas`, to be shown from time `at` (centiseconds).
    pub fn push(&mut self, canvas: &Canvas, at: u32) -> Result<()> {
        match &mut self.held {
            Some((held, _)) if held == canvas => {}
            // Too soon after the held frame to be shown on its own: replace
            // the held frame's content, keep its start.
            Some((held, start)) if at < *start + MIN_DELAY => held.clone_from(canvas),
            Some((_, start)) => {
                let delay = at - *start;
                self.write_held(delay)?;
                self.held = Some((canvas.clone(), at));
            }
            None => self.held = Some((canvas.clone(), at)),
        }
        Ok(())
    }

    /// Writes the last frame, shown for `duration` centiseconds. Returns the
    /// output and the number of frames written.
    pub fn finish(mut self, duration: u32) -> Result<(W, usize)> {
        self.write_held(duration.max(MIN_DELAY))?;
        let frames = self.frames;
        let out = self.encoder.into_inner().context("cannot write the GIF")?;
        Ok((out, frames))
    }

    fn write_held(&mut self, delay: u32) -> Result<()> {
        let Some((canvas, _)) = self.held.take() else {
            return Ok(());
        };

        let mut frame = match &self.shown {
            None => full_frame(&canvas),
            Some(prev) => match changed_rect(prev, &canvas) {
                Some(rect) => delta_frame(prev, &canvas, rect),
                None => empty_frame(),
            },
        };

        // Delays above the 16-bit limit continue in invisible frames.
        let max = u32::from(u16::MAX);
        let mut remaining = delay;
        frame.delay = remaining.min(max) as u16;
        remaining -= u32::from(frame.delay);
        self.write(&frame)?;
        while remaining > 0 {
            let mut pad = empty_frame();
            pad.delay = remaining.min(max) as u16;
            remaining -= u32::from(pad.delay);
            self.write(&pad)?;
        }

        self.shown = Some(canvas);
        Ok(())
    }

    fn write(&mut self, frame: &Frame) -> Result<()> {
        self.frames += 1;
        self.encoder
            .write_frame(frame)
            .context("cannot write the GIF")
    }
}

fn dimension(n: u32) -> Result<u16> {
    u16::try_from(n).context("the GIF would be too large (more than 65535 pixels wide or high)")
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Rect {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

/// Bounding box of the pixels that differ between `a` and `b`.
fn changed_rect(a: &Canvas, b: &Canvas) -> Option<Rect> {
    let w = a.width as usize;
    let rows: Vec<_> = a
        .pixels
        .chunks_exact(w)
        .zip(b.pixels.chunks_exact(w))
        .collect();

    let top = rows.iter().position(|(ra, rb)| ra != rb)?;
    let bottom = rows.iter().rposition(|(ra, rb)| ra != rb)?;
    let (mut left, mut right) = (w, 0);
    for (ra, rb) in &rows[top..=bottom] {
        if let Some(l) = ra.iter().zip(rb.iter()).position(|(x, y)| x != y) {
            left = left.min(l);
            let r = ra.iter().zip(rb.iter()).rposition(|(x, y)| x != y).unwrap();
            right = right.max(r);
        }
    }

    Some(Rect {
        x: left as u32,
        y: top as u32,
        w: (right - left + 1) as u32,
        h: (bottom - top + 1) as u32,
    })
}

/// A 1x1 transparent frame, which changes nothing.
fn empty_frame() -> Frame<'static> {
    Frame {
        width: 1,
        height: 1,
        buffer: Cow::Owned(vec![0]),
        palette: Some(vec![0, 0, 0]),
        transparent: Some(0),
        dispose: DisposalMethod::Keep,
        ..Frame::default()
    }
}

fn full_frame(canvas: &Canvas) -> Frame<'static> {
    let rect = Rect {
        x: 0,
        y: 0,
        w: canvas.width,
        h: canvas.height,
    };
    indexed_frame(canvas, rect, None)
}

fn delta_frame(prev: &Canvas, canvas: &Canvas, rect: Rect) -> Frame<'static> {
    indexed_frame(canvas, rect, Some(prev))
}

const TRANSPARENT: u32 = u32::MAX;

fn indexed_frame(canvas: &Canvas, rect: Rect, prev: Option<&Canvas>) -> Frame<'static> {
    let mut pixels = Vec::with_capacity((rect.w * rect.h) as usize);
    for y in rect.y..rect.y + rect.h {
        let start = (y * canvas.width + rect.x) as usize;
        let row = &canvas.pixels[start..start + rect.w as usize];
        match prev {
            Some(prev) => {
                let prev_row = &prev.pixels[start..start + rect.w as usize];
                pixels.extend(
                    row.iter()
                        .zip(prev_row)
                        .map(|(&p, &q)| if p == q { TRANSPARENT } else { p }),
                );
            }
            None => pixels.extend_from_slice(row),
        }
    }

    let (palette, indices, transparent) = quantize(&pixels);

    Frame {
        left: rect.x as u16,
        top: rect.y as u16,
        width: rect.w as u16,
        height: rect.h as u16,
        buffer: Cow::Owned(indices),
        palette: Some(palette),
        transparent,
        dispose: DisposalMethod::Keep,
        ..Frame::default()
    }
}

fn distance(a: u32, b: u32) -> u32 {
    let (a, b) = (unpack(a), unpack(b));
    // Weighted for perceived brightness.
    let d = |i: usize, w: u32| w * (i32::from(a[i]) - i32::from(b[i])).pow(2) as u32;
    d(0, 3) + d(1, 4) + d(2, 2)
}

/// Returns the RGB palette, the pixel indices and the transparent index.
fn quantize(pixels: &[u32]) -> (Vec<u8>, Vec<u8>, Option<u8>) {
    // Terminal frames consist of long runs of one color, so work per run
    // instead of per pixel.
    let runs = pixels.chunk_by(|a, b| a == b);

    let mut counts: HashMap<u32, usize> = HashMap::new();
    let mut has_transparent = false;
    for run in runs.clone() {
        if run[0] == TRANSPARENT {
            has_transparent = true;
        } else {
            *counts.entry(run[0]).or_default() += run.len();
        }
    }

    let mut colors: Vec<(u32, usize)> = counts.into_iter().collect();
    // Most frequent first; ties broken by value for deterministic output.
    colors.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let capacity = if has_transparent { 255 } else { 256 };
    let kept: Vec<u32> = colors.iter().take(capacity).map(|&(c, _)| c).collect();

    let mut index: HashMap<u32, u8> = kept
        .iter()
        .enumerate()
        .map(|(i, &c)| (c, i as u8))
        .collect();
    let transparent = has_transparent.then_some(kept.len() as u8);

    let mut indices = Vec::with_capacity(pixels.len());
    for run in runs {
        let p = run[0];
        let i = if p == TRANSPARENT {
            transparent.unwrap()
        } else {
            *index.entry(p).or_insert_with(|| {
                kept.iter()
                    .enumerate()
                    .min_by_key(|&(_, &c)| distance(p, c))
                    .map_or(0, |(i, _)| i as u8)
            })
        };
        indices.resize(indices.len() + run.len(), i);
    }

    let mut palette: Vec<u8> = kept.iter().flat_map(|&c| unpack(c)).collect();
    if has_transparent {
        palette.extend([0, 0, 0]);
    }
    (palette, indices, transparent)
}

#[cfg(test)]
#[allow(clippy::unreadable_literal)] // RGB colors as 0xRRGGBB
mod tests {
    use super::*;

    fn canvas(w: u32, h: u32, color: u32) -> Canvas {
        Canvas {
            width: w,
            height: h,
            pixels: vec![color; (w * h) as usize],
        }
    }

    #[test]
    fn rect_of_changes() {
        let a = canvas(10, 10, 0);
        let mut b = a.clone();
        assert_eq!(changed_rect(&a, &b), None);
        b.pixels[3 * 10 + 2] = 1;
        b.pixels[5 * 10 + 7] = 1;
        assert_eq!(
            changed_rect(&a, &b),
            Some(Rect {
                x: 2,
                y: 3,
                w: 6,
                h: 3
            })
        );
    }

    #[test]
    fn exact_palette() {
        let (palette, indices, transparent) =
            quantize(&[0x112233, TRANSPARENT, 0x112233, 0x445566]);
        assert_eq!(transparent, Some(2));
        assert_eq!(&palette[..6], &[0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
        assert_eq!(indices, vec![0, 2, 0, 1]);
    }

    #[test]
    fn too_many_colors_keeps_frequent_ones_exact() {
        // One dominant color plus 300 rare ones.
        let mut pixels = vec![0x00ff00; 1000];
        pixels.extend((0..300u32).map(|i| i * 3));
        let (palette, indices, transparent) = quantize(&pixels);
        assert_eq!(transparent, None);
        assert_eq!(palette.len(), 256 * 3);
        assert_eq!(&palette[..3], &[0, 0xff, 0]);
        assert!(indices[..1000].iter().all(|&i| i == 0));
    }

    #[test]
    fn writes_a_gif() {
        let mut w = GifWriter::new(Vec::new(), 4, 4, true).unwrap();
        let a = canvas(4, 4, 0x000000);
        let mut b = a.clone();
        b.pixels[5] = 0xffffff;
        w.push(&a, 0).unwrap();
        w.push(&b, 1).unwrap(); // too soon: replaces a
        w.push(&a, 50).unwrap();
        w.push(&a, 60).unwrap(); // unchanged: ignored
        let (out, frames) = w.finish(100).unwrap();
        assert_eq!(frames, 2);
        assert_eq!(&out[..6], b"GIF89a");
    }
}
