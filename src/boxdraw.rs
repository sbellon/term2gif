//! Box drawing (U+2500-U+257F) and block elements (U+2580-U+259F) are drawn
//! procedurally, so that they connect seamlessly across cells regardless of
//! the font and the line height.

// Pixel and font geometry mixes integer types and floats throughout; the
// values are small (cell and image sizes), so these conversions are intended.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

/// Line weight of one arm of a box drawing character.
const NONE: u8 = 0;
const LIGHT: u8 = 1;
const HEAVY: u8 = 2;
const DOUBLE: u8 = 3;

/// Arms (up, right, down, left) of U+2500..=U+257F; dashed lines are drawn
/// solid and the diagonals (U+2571-U+2573) are left to the font.
fn arms(ch: char) -> Option<[u8; 4]> {
    const L: u8 = LIGHT;
    const H: u8 = HEAVY;
    const D: u8 = DOUBLE;
    const N: u8 = NONE;
    #[rustfmt::skip]
    const TABLE: [[u8; 4]; 128] = [
        [N,L,N,L], [N,H,N,H], [L,N,L,N], [H,N,H,N], // ─━│┃
        [N,L,N,L], [N,H,N,H], [L,N,L,N], [H,N,H,N], // ┄┅┆┇
        [N,L,N,L], [N,H,N,H], [L,N,L,N], [H,N,H,N], // ┈┉┊┋
        [N,L,L,N], [N,H,L,N], [N,L,H,N], [N,H,H,N], // ┌┍┎┏
        [N,N,L,L], [N,N,L,H], [N,N,H,L], [N,N,H,H], // ┐┑┒┓
        [L,L,N,N], [L,H,N,N], [H,L,N,N], [H,H,N,N], // └┕┖┗
        [L,N,N,L], [L,N,N,H], [H,N,N,L], [H,N,N,H], // ┘┙┚┛
        [L,L,L,N], [L,H,L,N], [H,L,L,N], [L,L,H,N], // ├┝┞┟
        [H,L,H,N], [H,H,L,N], [L,H,H,N], [H,H,H,N], // ┠┡┢┣
        [L,N,L,L], [L,N,L,H], [H,N,L,L], [L,N,H,L], // ┤┥┦┧
        [H,N,H,L], [H,N,L,H], [L,N,H,H], [H,N,H,H], // ┨┩┪┫
        [N,L,L,L], [N,L,L,H], [N,H,L,L], [N,H,L,H], // ┬┭┮┯
        [N,L,H,L], [N,L,H,H], [N,H,H,L], [N,H,H,H], // ┰┱┲┳
        [L,L,N,L], [L,L,N,H], [L,H,N,L], [L,H,N,H], // ┴┵┶┷
        [H,L,N,L], [H,L,N,H], [H,H,N,L], [H,H,N,H], // ┸┹┺┻
        [L,L,L,L], [L,L,L,H], [L,H,L,L], [L,H,L,H], // ┼┽┾┿
        [H,L,L,L], [L,L,H,L], [H,L,H,L], [H,L,L,H], // ╀╁╂╃
        [H,H,L,L], [L,L,H,H], [L,H,H,L], [H,H,L,H], // ╄╅╆╇
        [L,H,H,H], [H,L,H,H], [H,H,H,L], [H,H,H,H], // ╈╉╊╋
        [N,L,N,L], [N,H,N,H], [L,N,L,N], [H,N,H,N], // ╌╍╎╏
        [N,D,N,D], [D,N,D,N], [N,D,L,N], [N,L,D,N], // ═║╒╓
        [N,D,D,N], [N,N,L,D], [N,N,D,L], [N,N,D,D], // ╔╕╖╗
        [L,D,N,N], [D,L,N,N], [D,D,N,N], [L,N,N,D], // ╘╙╚╛
        [D,N,N,L], [D,N,N,D], [L,D,L,N], [D,L,D,N], // ╜╝╞╟
        [D,D,D,N], [L,N,L,D], [D,N,D,L], [D,N,D,D], // ╠╡╢╣
        [N,D,L,D], [N,L,D,L], [N,D,D,D], [L,D,N,D], // ╤╥╦╧
        [D,L,N,L], [D,D,N,D], [L,D,L,D], [D,L,D,L], // ╨╩╪╫
        [D,D,D,D], [N,L,L,N], [N,N,L,L], [L,N,N,L], // ╬╭╮╯
        [L,L,N,N], [N,N,N,N], [N,N,N,N], [N,N,N,N], // ╰╱╲╳
        [N,N,N,L], [L,N,N,N], [N,L,N,N], [N,N,L,N], // ╴╵╶╷
        [N,N,N,H], [H,N,N,N], [N,H,N,N], [N,N,H,N], // ╸╹╺╻
        [N,H,N,L], [L,N,H,N], [N,L,N,H], [H,N,L,N], // ╼╽╾╿
    ];
    let i = (ch as u32).checked_sub(0x2500)? as usize;
    TABLE.get(i).copied().filter(|a| *a != [N; 4])
}

struct Mask {
    w: i32,
    h: i32,
    data: Vec<u8>,
}

impl Mask {
    fn new(w: u32, h: u32) -> Self {
        Mask {
            w: w as i32,
            h: h as i32,
            data: vec![0; (w * h) as usize],
        }
    }

    /// Fills `[x0, x1) x [y0, y1)`, clipped to the cell.
    fn fill(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, value: u8) {
        for y in y0.max(0)..y1.min(self.h) {
            for x in x0.max(0)..x1.min(self.w) {
                self.data[(y * self.w + x) as usize] = value;
            }
        }
    }
}

/// Coverage mask (`w` x `h`, one byte per pixel) for `ch`, if it is drawn
/// procedurally. `stroke` is the width of a light line.
pub fn mask(ch: char, w: u32, h: u32, stroke: u32) -> Option<Vec<u8>> {
    let mut m = Mask::new(w, h);
    if let Some(arms) = arms(ch) {
        draw_lines(&mut m, arms, stroke as i32);
    } else if !draw_block(&mut m, ch) {
        return None;
    }
    Some(m.data)
}

fn draw_lines(m: &mut Mask, [up, right, down, left]: [u8; 4], light: i32) {
    let heavy = light * 2 + 1;
    // Offset of the two lines of a double line from the center line.
    let gap = light + light / 2 + 1;
    let (cx, cy) = (m.w / 2, m.h / 2);

    // Extent [from, to) across the center line `c` of lines of weight `w`.
    let span = |c: i32, w: u8| -> (i32, i32) {
        let line = |c: i32, t: i32| (c - t / 2, c - t / 2 + t);
        match w {
            NONE => (c, c + 1),
            LIGHT => line(c, light),
            HEAVY => line(c, heavy),
            _ => (line(c - gap, light).0, line(c + gap, light).1),
        }
    };

    // Single lines run from the edge to the far side of the perpendicular
    // lines, so that they connect to all of them.
    let vertical = up.max(down);
    let horizontal = left.max(right);
    let (vx0, vx1) = span(cx, vertical);
    let (hy0, hy1) = span(cy, horizontal);
    let single = |w: u8| w == LIGHT || w == HEAVY;

    if single(left) {
        let (y0, y1) = span(cy, left);
        m.fill(0, y0, vx1, y1, 255);
    }
    if single(right) {
        let (y0, y1) = span(cy, right);
        m.fill(vx0, y0, m.w, y1, 255);
    }
    if single(up) {
        let (x0, x1) = span(cx, up);
        m.fill(x0, 0, x1, hy1, 255);
    }
    if single(down) {
        let (x0, x1) = span(cx, down);
        m.fill(x0, hy0, x1, m.h, 255);
    }

    // Double lines: two parallel light lines. Where they meet a perpendicular
    // double line, the outer line goes round the corner and the inner one
    // stops at the other arm's inner line.
    let (top, bottom) = (span(cy - gap, LIGHT), span(cy + gap, LIGHT));
    let (west, east) = (span(cx - gap, LIGHT), span(cx + gap, LIGHT));

    // Where a horizontal double line ends that comes from the left (or starts
    // that goes to the right), given the vertical arm `other` on its side.
    let reach_x = |other: u8, to_right: bool| match (other, vertical, to_right) {
        (DOUBLE, _, false) => west.1,
        (DOUBLE, _, true) => east.0,
        (NONE, DOUBLE, false) => east.1,
        (NONE, DOUBLE, true) => west.0,
        (_, _, false) => vx1,
        (_, _, true) => vx0,
    };
    let reach_y = |other: u8, downwards: bool| match (other, horizontal, downwards) {
        (DOUBLE, _, false) => top.1,
        (DOUBLE, _, true) => bottom.0,
        (NONE, DOUBLE, false) => bottom.1,
        (NONE, DOUBLE, true) => top.0,
        (_, _, false) => hy1,
        (_, _, true) => hy0,
    };

    if left == DOUBLE {
        m.fill(0, top.0, reach_x(up, false), top.1, 255);
        m.fill(0, bottom.0, reach_x(down, false), bottom.1, 255);
    }
    if right == DOUBLE {
        m.fill(reach_x(up, true), top.0, m.w, top.1, 255);
        m.fill(reach_x(down, true), bottom.0, m.w, bottom.1, 255);
    }
    if up == DOUBLE {
        m.fill(west.0, 0, west.1, reach_y(left, false), 255);
        m.fill(east.0, 0, east.1, reach_y(right, false), 255);
    }
    if down == DOUBLE {
        m.fill(west.0, reach_y(left, true), west.1, m.h, 255);
        m.fill(east.0, reach_y(right, true), east.1, m.h, 255);
    }
}

fn draw_block(m: &mut Mask, ch: char) -> bool {
    let (w, h) = (m.w, m.h);
    // Eighths of the cell, rounded so that adjacent blocks tile exactly.
    let ex = |n: i32| (w * n + 4) / 8;
    let ey = |n: i32| (h * n + 4) / 8;
    let (hx, hy) = (ex(4), ey(4));

    match ch {
        '\u{2580}' => m.fill(0, 0, w, hy, 255),
        '\u{2581}'..='\u{2588}' => {
            let n = ch as i32 - 0x2580;
            m.fill(0, h - ey(n), w, h, 255);
        }
        '\u{2589}'..='\u{258F}' => {
            let n = 0x2590 - ch as i32; // 7/8 .. 1/8
            m.fill(0, 0, ex(n), h, 255);
        }
        '\u{2590}' => m.fill(hx, 0, w, h, 255),
        '\u{2591}' => m.fill(0, 0, w, h, 64),
        '\u{2592}' => m.fill(0, 0, w, h, 128),
        '\u{2593}' => m.fill(0, 0, w, h, 192),
        '\u{2594}' => m.fill(0, 0, w, ey(1), 255),
        '\u{2595}' => m.fill(w - ex(1), 0, w, h, 255),
        '\u{2596}'..='\u{259F}' => {
            // Quadrants: upper left, upper right, lower left, lower right.
            let quads: [bool; 4] = match ch {
                '\u{2596}' => [false, false, true, false],
                '\u{2597}' => [false, false, false, true],
                '\u{2598}' => [true, false, false, false],
                '\u{2599}' => [true, false, true, true],
                '\u{259A}' => [true, false, false, true],
                '\u{259B}' => [true, true, true, false],
                '\u{259C}' => [true, true, false, true],
                '\u{259D}' => [false, true, false, false],
                '\u{259E}' => [false, true, true, false],
                _ => [false, true, true, true],
            };
            let rects = [
                (0, 0, hx, hy),
                (hx, 0, w, hy),
                (0, hy, hx, h),
                (hx, hy, w, h),
            ];
            for (on, (x0, y0, x1, y1)) in quads.iter().zip(rects) {
                if *on {
                    m.fill(x0, y0, x1, y1, 255);
                }
            }
        }
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(mask: &[u8], w: u32, y: u32) -> Vec<u8> {
        mask[(y * w) as usize..((y + 1) * w) as usize].to_vec()
    }

    #[test]
    fn horizontal_line_spans_the_cell() {
        let (w, h) = (8, 16);
        let m = mask('─', w, h, 1).unwrap();
        assert!(row(&m, w, h / 2).iter().all(|&v| v == 255));
        assert!(row(&m, w, 0).iter().all(|&v| v == 0));
    }

    #[test]
    fn vertical_line_spans_the_cell() {
        let (w, h) = (8, 16);
        let m = mask('│', w, h, 1).unwrap();
        for y in 0..h {
            assert_eq!(m[(y * w + w / 2) as usize], 255);
        }
    }

    #[test]
    fn full_block_and_shade() {
        assert!(mask('█', 8, 16, 1).unwrap().iter().all(|&v| v == 255));
        assert!(mask('▒', 8, 16, 1).unwrap().iter().all(|&v| v == 128));
    }

    #[test]
    fn not_procedural() {
        assert!(mask('a', 8, 16, 1).is_none());
        assert!(mask('╱', 8, 16, 1).is_none());
    }

    #[test]
    fn table_covers_all_lines() {
        // Every box drawing character except the diagonals has arms.
        for c in 0x2500..=0x257F {
            let ch = char::from_u32(c).unwrap();
            assert_eq!(arms(ch).is_some(), !(0x2571..=0x2573).contains(&c), "{ch}");
        }
    }
}
