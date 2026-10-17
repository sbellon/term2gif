// SPDX-FileCopyrightText: 2026 Stefan Bellon
//
// SPDX-License-Identifier: MIT

//! Rendering the terminal state into an RGB canvas.

// Pixel and font geometry mixes integer types and floats throughout; the
// values are small (cell and image sizes), so these conversions are intended.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::rc::Rc;

use avt::{Color, Vt};

use crate::boxdraw;
use crate::font::{Bitmap, Fonts};
use crate::theme::{Rgb, Theme};

/// An RGB image with one `0x00RRGGBB` value per pixel.
#[derive(Clone, PartialEq, Eq)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u32>,
}

fn pack([r, g, b]: Rgb) -> u32 {
    u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b)
}

pub fn unpack(p: u32) -> Rgb {
    [(p >> 16) as u8, (p >> 8) as u8, p as u8]
}

fn mix(dst: u32, src: Rgb, alpha: u8) -> u32 {
    let a = u32::from(alpha);
    let d = unpack(dst);
    let ch = |i: usize| (u32::from(d[i]) * (255 - a) + u32::from(src[i]) * a + 127) / 255;
    ch(0) << 16 | ch(1) << 8 | ch(2)
}

impl Canvas {
    pub fn new(width: u32, height: u32) -> Self {
        Canvas {
            width,
            height,
            pixels: vec![0; (width * height) as usize],
        }
    }

    fn fill(&mut self, x: i32, y: i32, w: u32, h: u32, color: Rgb) {
        let color = pack(color);
        let x0 = x.clamp(0, self.width as i32) as u32;
        let x1 = (x + w as i32).clamp(0, self.width as i32) as u32;
        for yy in y.max(0)..(y + h as i32).min(self.height as i32) {
            let row = (yy as u32 * self.width) as usize;
            self.pixels[row + x0 as usize..row + x1 as usize].fill(color);
        }
    }

    /// Blends `color` with coverage `mask` (`w` x `h`) at (x, y).
    fn blend_mask(&mut self, x: i32, y: i32, w: u32, h: u32, mask: &[u8], color: Rgb) {
        for my in 0..h as i32 {
            let cy = y + my;
            if cy < 0 || cy >= self.height as i32 {
                continue;
            }
            for mx in 0..w as i32 {
                let cx = x + mx;
                let alpha = mask[(my * w as i32 + mx) as usize];
                if alpha == 0 || cx < 0 || cx >= self.width as i32 {
                    continue;
                }
                let pixel = &mut self.pixels[(cy as u32 * self.width + cx as u32) as usize];
                *pixel = if alpha == 255 {
                    pack(color)
                } else {
                    mix(*pixel, color, alpha)
                };
            }
        }
    }

    /// Blends an RGBA image (`w` x `h`) at (x, y).
    fn blend_rgba(&mut self, x: i32, y: i32, w: u32, h: u32, rgba: &[u8]) {
        for my in 0..h as i32 {
            let cy = y + my;
            if cy < 0 || cy >= self.height as i32 {
                continue;
            }
            for mx in 0..w as i32 {
                let cx = x + mx;
                let src = ((my * w as i32 + mx) * 4) as usize;
                let alpha = rgba[src + 3];
                if alpha == 0 || cx < 0 || cx >= self.width as i32 {
                    continue;
                }
                let pixel = &mut self.pixels[(cy as u32 * self.width + cx as u32) as usize];
                *pixel = mix(*pixel, [rgba[src], rgba[src + 1], rgba[src + 2]], alpha);
            }
        }
    }
}

/// Nearest-neighbor enlargement of `src` by `scale` into `dst`.
fn upscale(src: &Canvas, dst: &mut Canvas, scale: u32) {
    let (s, dw) = (scale as usize, dst.width as usize);
    for (y, row) in src.pixels.chunks_exact(src.width as usize).enumerate() {
        let start = y * s * dw;
        let first = &mut dst.pixels[start..start + dw];
        for (x, &p) in row.iter().enumerate() {
            first[x * s..(x + 1) * s].fill(p);
        }
        for i in 1..s {
            dst.pixels.copy_within(start..start + dw, start + i * dw);
        }
    }
}

pub struct Renderer {
    fonts: Fonts,
    theme: Theme,
    bold_is_bright: bool,
    pad_x: u32,
    pad_y: u32,
    box_masks: HashMap<(char, u32), Option<Rc<Vec<u8>>>>,
    /// Each pixel becomes a `scale` x `scale` block in the output.
    scale: u32,
    /// The unscaled picture, when scaling.
    unscaled: Option<Canvas>,
}

/// Foreground work for one cell, done after all backgrounds are painted so
/// that glyphs reaching into neighbor cells are not painted over.
#[allow(clippy::struct_excessive_bools)] // the cell's text attributes
struct Text {
    x: i32,
    y: i32,
    cells: u32,
    ch: char,
    color: Rgb,
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
}

impl Renderer {
    /// `scale`: enlarge the picture by this factor, each pixel becoming a
    /// block of pixels (for bitmap fonts, which have no other sizes).
    pub fn new(fonts: Fonts, theme: Theme, bold_is_bright: bool, scale: u32) -> Self {
        let cell = fonts.cell;
        Renderer {
            pad_x: cell.width,
            pad_y: cell.width,
            fonts,
            theme,
            bold_is_bright,
            box_masks: HashMap::new(),
            scale: scale.max(1),
            unscaled: None,
        }
    }

    fn unscaled_size(&self, cols: usize, rows: usize) -> (u32, u32) {
        let cell = self.fonts.cell;
        (
            cols as u32 * cell.width + 2 * self.pad_x,
            rows as u32 * cell.height + 2 * self.pad_y,
        )
    }

    /// Canvas size for a terminal of `cols` x `rows`.
    pub fn canvas_size(&self, cols: usize, rows: usize) -> (u32, u32) {
        let (w, h) = self.unscaled_size(cols, rows);
        (w * self.scale, h * self.scale)
    }

    fn color(&self, color: Option<Color>, default: Rgb) -> Rgb {
        match color {
            None => default,
            Some(Color::Indexed(n)) => self.theme.indexed(n),
            Some(Color::RGB(c)) => [c.r, c.g, c.b],
        }
    }

    /// Draws the terminal into the top left of `canvas`; the rest of the
    /// canvas gets the background color.
    pub fn render(&mut self, vt: &Vt, canvas: &mut Canvas) {
        if self.scale == 1 {
            return self.draw(vt, canvas);
        }
        let (w, h) = (canvas.width / self.scale, canvas.height / self.scale);
        let mut unscaled = match self.unscaled.take() {
            Some(c) if (c.width, c.height) == (w, h) => c,
            _ => Canvas::new(w, h),
        };
        self.draw(vt, &mut unscaled);
        upscale(&unscaled, canvas, self.scale);
        self.unscaled = Some(unscaled);
    }

    fn draw(&mut self, vt: &Vt, canvas: &mut Canvas) {
        let cell = self.fonts.cell;
        let (bg_default, fg_default) = (self.theme.background, self.theme.foreground);
        canvas.pixels.fill(pack(bg_default));

        let cursor = vt.cursor();
        let mut texts = Vec::new();

        for (row, line) in vt.view().enumerate() {
            for (col, c) in line.cells().iter().enumerate() {
                let cells = u32::from(c.width());
                if cells == 0 {
                    continue; // right half of a wide character
                }
                let pen = c.pen();

                let mut fg_color = pen.foreground();
                if self.bold_is_bright && pen.is_bold() {
                    if let Some(Color::Indexed(n @ 0..=7)) = fg_color {
                        fg_color = Some(Color::Indexed(n + 8));
                    }
                }
                let mut fg = self.color(fg_color, fg_default);
                let mut bg = self.color(pen.background(), bg_default);

                let at_cursor = cursor.visible && cursor.row == row && cursor.col == col;
                if pen.is_inverse() != at_cursor {
                    std::mem::swap(&mut fg, &mut bg);
                }
                if pen.is_faint() {
                    fg = unpack(mix(pack(bg), fg, 128));
                }

                let x = (self.pad_x + col as u32 * cell.width) as i32;
                let y = (self.pad_y + row as u32 * cell.height) as i32;
                if bg != bg_default {
                    canvas.fill(x, y, cell.width * cells, cell.height, bg);
                }

                let ch = c.char();
                if ch != ' ' || pen.is_underline() || pen.is_strikethrough() {
                    texts.push(Text {
                        x,
                        y,
                        cells,
                        ch,
                        color: fg,
                        bold: pen.is_bold(),
                        italic: pen.is_italic(),
                        underline: pen.is_underline(),
                        strikethrough: pen.is_strikethrough(),
                    });
                }
            }
        }

        for t in texts {
            self.draw_text(canvas, &t);
        }
    }

    fn draw_text(&mut self, canvas: &mut Canvas, t: &Text) {
        let cell = self.fonts.cell;
        let width = cell.width * t.cells;

        if t.ch != ' ' {
            let stroke = cell.stroke;
            let mask = self
                .box_masks
                .entry((t.ch, t.cells))
                .or_insert_with(|| boxdraw::mask(t.ch, width, cell.height, stroke).map(Rc::new))
                .clone();

            if let Some(mask) = mask {
                canvas.blend_mask(t.x, t.y, width, cell.height, &mask, t.color);
            } else if let Some(g) = self.fonts.glyph(t.ch, t.bold, t.italic, t.cells) {
                let gx = t.x + g.left;
                let gy = t.y + cell.baseline - g.top;
                match &g.bitmap {
                    Bitmap::Mask(m) => canvas.blend_mask(gx, gy, g.width, g.height, m, t.color),
                    Bitmap::Color(rgba) => {
                        // Center color glyphs (emoji) in their cells.
                        let gx = t.x + (width as i32 - g.width as i32) / 2;
                        canvas.blend_rgba(gx, gy, g.width, g.height, rgba);
                    }
                }
            }
        }

        if t.underline {
            canvas.fill(t.x, t.y + cell.underline_top, width, cell.stroke, t.color);
        }
        if t.strikethrough {
            canvas.fill(t.x, t.y + cell.strikeout_top, width, cell.stroke, t.color);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upscale_doubles_pixels() {
        let src = Canvas {
            width: 2,
            height: 1,
            pixels: vec![1, 2],
        };
        let mut dst = Canvas::new(4, 2);
        upscale(&src, &mut dst, 2);
        assert_eq!(dst.pixels, vec![1, 1, 2, 2, 1, 1, 2, 2]);
    }
}
