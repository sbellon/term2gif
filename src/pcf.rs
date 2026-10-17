//! Reading X11 bitmap fonts in PCF format (`.pcf`, `.pcf.gz`), such as
//! xterm's default `fixed` font.

// Pixel and font geometry mixes integer types and floats throughout; the
// values are small (cell and image sizes), so these conversions are intended.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;

const PROPERTIES: u32 = 1 << 0;
const ACCELERATORS: u32 = 1 << 1;
const METRICS: u32 = 1 << 2;
const BITMAPS: u32 = 1 << 3;
const BDF_ENCODINGS: u32 = 1 << 5;
const BDF_ACCELERATORS: u32 = 1 << 8;

const COMPRESSED_METRICS: u32 = 0x100;
const BYTE_MSB_FIRST: u32 = 1 << 2;
const BIT_MSB_FIRST: u32 = 1 << 3;

#[derive(Clone, Copy, Debug, Default)]
struct Metrics {
    left_bearing: i16,
    right_bearing: i16,
    ascent: i16,
    descent: i16,
}

pub struct PcfGlyph {
    /// Offset of the bitmap from the pen position (x right, y up).
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    /// One byte per pixel, 0 or 255.
    pub mask: Vec<u8>,
}

pub struct PcfFont {
    /// Character cell: the font's advance width, ascent and descent.
    pub width: u32,
    pub ascent: i32,
    pub descent: i32,
    glyphs: HashMap<char, (Metrics, Vec<u8>)>,
}

/// A little cursor over a table, honoring the table's byte order.
struct Table<'a> {
    data: &'a [u8],
    pos: usize,
    msb: bool,
}

impl<'a> Table<'a> {
    /// Opens a table; the first (always little-endian) word is its format.
    fn open(data: &'a [u8], offset: usize) -> Result<(Table<'a>, u32)> {
        let mut t = Table {
            data,
            pos: offset,
            msb: false,
        };
        let format = t.u32()?;
        t.msb = format & BYTE_MSB_FIRST != 0;
        Ok((t, format))
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8]> {
        let b = self
            .data
            .get(self.pos..self.pos + n)
            .context("truncated PCF font")?;
        self.pos += n;
        Ok(b)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        let b: [u8; 2] = self.bytes(2)?.try_into().unwrap();
        Ok(if self.msb {
            u16::from_be_bytes(b)
        } else {
            u16::from_le_bytes(b)
        })
    }

    fn i16(&mut self) -> Result<i16> {
        Ok(self.u16()? as i16)
    }

    fn u32(&mut self) -> Result<u32> {
        let b: [u8; 4] = self.bytes(4)?.try_into().unwrap();
        Ok(if self.msb {
            u32::from_be_bytes(b)
        } else {
            u32::from_le_bytes(b)
        })
    }

    fn i32(&mut self) -> Result<i32> {
        Ok(self.u32()? as i32)
    }

    fn metrics(&mut self, compressed: bool) -> Result<Metrics> {
        if compressed {
            let mut b = || -> Result<i16> { Ok(i16::from(self.u8()?) - 0x80) };
            let (left_bearing, right_bearing, _width, ascent, descent) =
                (b()?, b()?, b()?, b()?, b()?);
            Ok(Metrics {
                left_bearing,
                right_bearing,
                ascent,
                descent,
            })
        } else {
            let m = Metrics {
                left_bearing: self.i16()?,
                right_bearing: self.i16()?,
                ascent: {
                    self.i16()?; // character width
                    self.i16()?
                },
                descent: self.i16()?,
            };
            self.u16()?; // attributes
            Ok(m)
        }
    }
}

/// Reads the whole file, decompressing `.gz` files.
fn read_file(path: &Path) -> Result<Vec<u8>> {
    let raw = std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))?;
    if raw.starts_with(&[0x1f, 0x8b]) {
        let mut data = Vec::new();
        GzDecoder::new(&raw[..])
            .read_to_end(&mut data)
            .with_context(|| format!("cannot decompress {}", path.display()))?;
        Ok(data)
    } else {
        Ok(raw)
    }
}

/// Maps encodings of the font's charset to Unicode, for the charsets that
/// are subsets of Unicode.
fn charset_mapper(registry: &str, encoding: &str) -> Option<fn(u32) -> Option<char>> {
    let registry = registry.to_ascii_lowercase();
    match (registry.as_str(), encoding) {
        ("iso10646", _) => Some(char::from_u32),
        ("iso8859", "1") => Some(|c| if c < 256 { char::from_u32(c) } else { None }),
        _ => None,
    }
}

/// The properties with string values, such as `CHARSET_REGISTRY`.
fn read_properties(data: &[u8], offset: usize) -> Result<HashMap<String, String>> {
    let (mut table, _) = Table::open(data, offset)?;
    let count = table.u32()? as usize;
    let mut entries = Vec::new();
    for _ in 0..count {
        let name = table.u32()? as usize;
        let is_string = table.u8()? != 0;
        let value = table.u32()? as usize;
        entries.push((name, is_string, value));
    }
    if count % 4 != 0 {
        table.bytes(4 - count % 4)?;
    }
    let size = table.u32()? as usize;
    let strings = table.bytes(size)?;
    let string_at = |offset: usize| {
        strings.get(offset..).map(|s| {
            let end = s.iter().position(|&b| b == 0).unwrap_or(s.len());
            String::from_utf8_lossy(&s[..end]).into_owned()
        })
    };

    let mut props = HashMap::new();
    for (name, is_string, value) in entries {
        if let (Some(name), true) = (string_at(name), is_string) {
            if let Some(value) = string_at(value) {
                props.insert(name, value);
            }
        }
    }
    Ok(props)
}

/// The font's ascent, descent and maximal character width.
fn read_accelerators(data: &[u8], offset: usize) -> Result<(i32, i32, i16)> {
    let (mut table, _) = Table::open(data, offset)?;
    table.bytes(8)?; // flags and padding
    let ascent = table.i32()?;
    let descent = table.i32()?;
    table.i32()?; // max overlap
    table.metrics(false)?; // min bounds
    // Max bounds' character width is the cell width of a terminal font.
    table.i16()?;
    table.i16()?;
    let max_width = table.i16()?;
    Ok((ascent, descent, max_width))
}

fn read_metrics(data: &[u8], offset: usize) -> Result<Vec<Metrics>> {
    let (mut table, format) = Table::open(data, offset)?;
    let compressed = format & COMPRESSED_METRICS != 0;
    let count = if compressed {
        table.u16()? as usize
    } else {
        table.u32()? as usize
    };
    (0..count).map(|_| table.metrics(compressed)).collect()
}

/// The bitmaps table: packed glyph bitmaps in the font's bit layout.
struct Bitmaps<'a> {
    offsets: Vec<u32>,
    data: &'a [u8],
    /// Rows are padded to a multiple of this many bytes.
    pad: usize,
    /// Bytes are grouped in units of this many bytes.
    unit: usize,
    byte_msb: bool,
    bit_msb: bool,
}

impl<'a> Bitmaps<'a> {
    fn read(data: &'a [u8], offset: usize) -> Result<Bitmaps<'a>> {
        let (mut table, format) = Table::open(data, offset)?;
        let count = table.u32()? as usize;
        let offsets = (0..count)
            .map(|_| table.u32())
            .collect::<Result<Vec<_>>>()?;
        let sizes = (0..4).map(|_| table.u32()).collect::<Result<Vec<_>>>()?;
        Ok(Bitmaps {
            offsets,
            data: table.bytes(sizes[(format & 3) as usize] as usize)?,
            pad: 1 << (format & 3),
            unit: 1 << ((format >> 4) & 3),
            byte_msb: format & BYTE_MSB_FIRST != 0,
            bit_msb: format & BIT_MSB_FIRST != 0,
        })
    }

    /// Glyph `index` as one byte per pixel.
    fn decode(&self, index: usize, metrics: Metrics) -> Option<Vec<u8>> {
        let width = (metrics.right_bearing - metrics.left_bearing).max(0) as usize;
        let height = (metrics.ascent + metrics.descent).max(0) as usize;
        if width == 0 || height == 0 {
            return Some(Vec::new()); // e.g. the space character
        }
        let stride = width.div_ceil(8).div_ceil(self.pad) * self.pad;
        let start = *self.offsets.get(index)? as usize;
        let rows = self.data.get(start..start + stride * height)?;

        let mut mask = vec![0u8; width * height];
        for (y, row) in rows.chunks_exact(stride).enumerate() {
            for x in 0..width {
                let mut byte = x / 8;
                // Bytes within a scan unit are swapped if the byte and bit
                // orders differ.
                if self.byte_msb != self.bit_msb && self.unit > 1 {
                    byte = byte - byte % self.unit + (self.unit - 1 - byte % self.unit);
                }
                let bits = *row.get(byte)?;
                let bit = if self.bit_msb { 7 - x % 8 } else { x % 8 };
                if bits >> bit & 1 != 0 {
                    mask[y * width + x] = 255;
                }
            }
        }
        Some(mask)
    }
}

impl PcfFont {
    pub fn load(path: &Path) -> Result<PcfFont> {
        let data = read_file(path)?;
        PcfFont::parse(&data).with_context(|| format!("cannot load font {}", path.display()))
    }

    pub fn parse(data: &[u8]) -> Result<PcfFont> {
        if !data.starts_with(b"\x01fcp") {
            bail!("not a PCF font");
        }
        let mut header = Table {
            data,
            pos: 4,
            msb: false,
        };
        let count = header.u32()?;
        let mut tables = HashMap::new();
        for _ in 0..count.min(64) {
            let kind = header.u32()?;
            let _format = header.u32()?;
            let _size = header.u32()?;
            let offset = header.u32()? as usize;
            tables.insert(kind, offset);
        }
        let table = |kind| tables.get(&kind).copied();

        // The charset, so that encodings can be mapped to Unicode.
        let props = match table(PROPERTIES) {
            Some(offset) => read_properties(data, offset)?,
            None => HashMap::new(),
        };
        let registry = props
            .get("CHARSET_REGISTRY")
            .map_or("ISO8859", String::as_str);
        let encoding = props.get("CHARSET_ENCODING").map_or("1", String::as_str);
        let to_char = charset_mapper(registry, encoding)
            .with_context(|| format!("unsupported font charset {registry}-{encoding}"))?;

        let accelerators = table(BDF_ACCELERATORS)
            .or(table(ACCELERATORS))
            .context("PCF font without accelerators table")?;
        let (ascent, descent, max_width) = read_accelerators(data, accelerators)?;

        let offset = table(METRICS).context("PCF font without metrics table")?;
        let metrics = read_metrics(data, offset)?;

        let offset = table(BITMAPS).context("PCF font without bitmaps table")?;
        let bitmaps = Bitmaps::read(data, offset)?;
        let glyph_count = bitmaps.offsets.len().min(metrics.len());

        // Encodings: character code -> glyph index.
        let offset = table(BDF_ENCODINGS).context("PCF font without encodings table")?;
        let (mut table, _) = Table::open(data, offset)?;
        let (min2, max2, min1, max1) = (table.u16()?, table.u16()?, table.u16()?, table.u16()?);
        table.u16()?; // default char
        let mut glyphs = HashMap::new();
        for byte1 in min1..=max1 {
            for byte2 in min2..=max2 {
                let index = table.u16()? as usize;
                if index == 0xFFFF || index >= glyph_count {
                    continue;
                }
                let code = u32::from(byte1) << 8 | u32::from(byte2);
                let metric = metrics[index];
                if let (Some(ch), Some(mask)) = (to_char(code), bitmaps.decode(index, metric)) {
                    glyphs.insert(ch, (metric, mask));
                }
            }
        }

        Ok(PcfFont {
            width: max_width.max(1) as u32,
            ascent,
            descent,
            glyphs,
        })
    }

    pub fn glyph(&self, ch: char) -> Option<PcfGlyph> {
        let (m, mask) = self.glyphs.get(&ch)?;
        Some(PcfGlyph {
            left: i32::from(m.left_bearing),
            top: i32::from(m.ascent),
            width: (m.right_bearing - m.left_bearing).max(0) as u32,
            height: (m.ascent + m.descent).max(0) as u32,
            mask: mask.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal PCF font with one 2x2 glyph for 'A' (0x41).
    fn tiny_pcf(byte_msb: bool) -> Vec<u8> {
        let fmt = if byte_msb {
            BYTE_MSB_FIRST | BIT_MSB_FIRST
        } else {
            BIT_MSB_FIRST
        };
        let w16 = |v: u16| {
            if byte_msb {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };
        let w32 = |v: u32| {
            if byte_msb {
                v.to_be_bytes()
            } else {
                v.to_le_bytes()
            }
        };

        let mut accel = fmt.to_le_bytes().to_vec();
        accel.extend([0u8; 8]);
        accel.extend(w32(3)); // ascent
        accel.extend(w32(1)); // descent
        accel.extend(w32(0));
        for _ in 0..2 {
            // min and max bounds: lsb, rsb, width, ascent, descent, attrs
            for v in [0u16, 2, 4, 2, 0, 0] {
                accel.extend(w16(v));
            }
        }

        let mut metrics = (fmt | COMPRESSED_METRICS).to_le_bytes().to_vec();
        metrics.extend(w16(1));
        metrics.extend([0x80, 0x82, 0x84, 0x82, 0x80]);

        // Glyph pad 1 byte; rows "10" and "01".
        let mut bitmaps = fmt.to_le_bytes().to_vec();
        bitmaps.extend(w32(1));
        bitmaps.extend(w32(0));
        for size in [2u32, 4, 8, 16] {
            bitmaps.extend(w32(size));
        }
        bitmaps.extend([0b1000_0000, 0b0100_0000]);

        let mut enc = fmt.to_le_bytes().to_vec();
        for v in [0x41u16, 0x41, 0, 0, 0, 0] {
            enc.extend(w16(v));
        }

        let tables = [
            (ACCELERATORS, accel),
            (METRICS, metrics),
            (BITMAPS, bitmaps),
            (BDF_ENCODINGS, enc),
        ];
        let mut out = b"\x01fcp".to_vec();
        out.extend((tables.len() as u32).to_le_bytes());
        let mut offset = 8 + tables.len() * 16;
        let mut body: Vec<u8> = Vec::new();
        for (kind, data) in &tables {
            for v in [*kind, 0, data.len() as u32, offset as u32] {
                out.extend(v.to_le_bytes());
            }
            offset += data.len();
            body.extend(data);
        }
        out.extend(body);
        out
    }

    #[test]
    fn parses_both_byte_orders() {
        for msb in [false, true] {
            let font = PcfFont::parse(&tiny_pcf(msb)).unwrap();
            assert_eq!((font.width, font.ascent, font.descent), (4, 3, 1));
            let g = font.glyph('A').unwrap();
            assert_eq!((g.width, g.height, g.top), (2, 2, 2));
            assert_eq!(g.mask, vec![255, 0, 0, 255]);
            assert!(font.glyph('B').is_none());
        }
    }

    #[test]
    fn system_fixed_font() {
        // Only where xfonts-base is installed.
        let path = Path::new("/usr/share/fonts/X11/misc/6x13.pcf.gz");
        if !path.exists() {
            return;
        }
        let font = PcfFont::load(path).unwrap();
        assert_eq!(font.width, 6);
        assert_eq!(font.ascent + font.descent, 13);
        assert!(['ä', '─', '€'].iter().all(|&c| font.glyph(c).is_some()));
    }
}
