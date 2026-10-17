//! Fonts: the bundled DejaVu Sans Mono, optionally preferred system families
//! or an X11 bitmap font, and a per-character fallback to any installed font
//! (e.g. for CJK or emoji).

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
use std::rc::Rc;

use anyhow::{Result, bail};
use swash::FontRef;
use swash::scale::image::Content;
use swash::scale::{Render, ScaleContext, Source, StrikeWith};
use swash::zeno::{Angle, Format, Transform};

use crate::pcf::{PcfFont, PcfGlyph};

const BUNDLED_REGULAR: &[u8] = include_bytes!("../fonts/DejaVuSansMono.ttf");
const BUNDLED_BOLD: &[u8] = include_bytes!("../fonts/DejaVuSansMono-Bold.ttf");

/// Families tried first for characters from the emoji planes.
const EMOJI_FAMILIES: &[&str] = &[
    "Noto Color Emoji",
    "Segoe UI Emoji",
    "Apple Color Emoji",
    "Twemoji",
    "Noto Emoji",
];

struct Face {
    data: Cow<'static, [u8]>,
    index: u32,
}

impl Face {
    fn font(&self) -> FontRef<'_> {
        FontRef::from_index(&self.data, self.index as usize).expect("face was validated on load")
    }

    fn has(&self, ch: char) -> bool {
        self.font().charmap().map(ch) != 0
    }
}

/// A regular face and, if available, a real bold face of the same family.
struct Family {
    regular: usize,
    bold: Option<usize>,
}

pub enum Bitmap {
    /// Coverage, one byte per pixel.
    Mask(Vec<u8>),
    /// Premultiplied-free RGBA, four bytes per pixel.
    Color(Vec<u8>),
}

pub struct Glyph {
    /// Offset of the bitmap from the pen position (x right, y up).
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
    pub bitmap: Bitmap,
}

/// Pixel geometry of one terminal cell.
#[derive(Clone, Copy, Debug)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    /// Baseline distance from the top of the cell.
    pub baseline: i32,
    pub underline_top: i32,
    pub strikeout_top: i32,
    pub stroke: u32,
}

/// An X11 bitmap font (e.g. xterm's `fixed`), its bold variant and its
/// companion font for wide (CJK) characters.
pub struct BitmapFonts {
    pub regular: PcfFont,
    pub bold: Option<PcfFont>,
    pub wide: Option<PcfFont>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct GlyphKey {
    ch: char,
    bold: bool,
    italic: bool,
}

pub struct Fonts {
    /// Takes precedence over all other fonts and defines the cell size.
    bitmap: Option<BitmapFonts>,
    faces: Vec<Face>,
    families: Vec<Family>,
    /// Fallback faces loaded from the system, in the order they were needed.
    fallbacks: Vec<usize>,
    system: Option<fontdb::Database>,
    system_loaded: HashMap<fontdb::ID, usize>,
    fallback_for: HashMap<char, Option<usize>>,
    size: f32,
    pub cell: CellMetrics,
    scale: ScaleContext,
    glyphs: HashMap<GlyphKey, Option<Rc<Glyph>>>,
}

fn load_face(db: &fontdb::Database, id: fontdb::ID) -> Option<Face> {
    let (data, index) = db.with_face_data(id, |data, index| (data.to_vec(), index))?;
    FontRef::from_index(&data, index as usize)?;
    Some(Face {
        data: Cow::Owned(data),
        index,
    })
}

fn system_db() -> fontdb::Database {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    db
}

impl Fonts {
    /// `families`: system font families to prefer over the bundled font.
    /// `line_height`: multiple of `size`, or the font's own height if `None`.
    /// With a bitmap font, `size` and `line_height` are ignored: the bitmap
    /// font's cell is used, and other fonts are scaled to fit it.
    pub fn new(
        families: &[String],
        size: f32,
        line_height: Option<f32>,
        bitmap: Option<BitmapFonts>,
    ) -> Result<Fonts> {
        let mut fonts = Fonts {
            bitmap: None,
            faces: Vec::new(),
            families: Vec::new(),
            fallbacks: Vec::new(),
            system: None,
            system_loaded: HashMap::new(),
            fallback_for: HashMap::new(),
            size,
            cell: CellMetrics {
                width: 0,
                height: 0,
                baseline: 0,
                underline_top: 0,
                strikeout_top: 0,
                stroke: 0,
            },
            scale: ScaleContext::new(),
            glyphs: HashMap::new(),
        };

        for name in families {
            if !fonts.add_system_family(name) {
                bail!("font family `{name}` not found");
            }
        }

        for data in [BUNDLED_REGULAR, BUNDLED_BOLD] {
            fonts.faces.push(Face {
                data: Cow::Borrowed(data),
                index: 0,
            });
        }
        let n = fonts.faces.len();
        fonts.families.push(Family {
            regular: n - 2,
            bold: Some(n - 1),
        });

        match bitmap {
            Some(bitmap) => {
                fonts.cell = bitmap_cell_metrics(&bitmap.regular);
                fonts.size = fonts.size_to_fit_cell();
                fonts.bitmap = Some(bitmap);
            }
            None => fonts.cell = fonts.compute_cell_metrics(line_height),
        }
        Ok(fonts)
    }

    fn system(&mut self) -> &fontdb::Database {
        self.system.get_or_insert_with(system_db)
    }

    fn face_for_id(&mut self, id: fontdb::ID) -> Option<usize> {
        if let Some(&i) = self.system_loaded.get(&id) {
            return Some(i);
        }
        let face = load_face(self.system(), id)?;
        self.faces.push(face);
        let i = self.faces.len() - 1;
        self.system_loaded.insert(id, i);
        Some(i)
    }

    fn add_system_family(&mut self, name: &str) -> bool {
        let families = [fontdb::Family::Name(name)];
        let query = |weight| fontdb::Query {
            families: &families,
            weight,
            ..Default::default()
        };
        let db = self.system();
        let Some(regular) = db.query(&query(fontdb::Weight::NORMAL)) else {
            return false;
        };
        let bold = db
            .query(&query(fontdb::Weight::BOLD))
            .filter(|&id| id != regular && db.face(id).is_some_and(|f| f.weight.0 >= 600));

        let Some(regular) = self.face_for_id(regular) else {
            return false;
        };
        let bold = bold.and_then(|id| self.face_for_id(id));
        self.families.push(Family { regular, bold });
        true
    }

    /// The first family's regular face defines the cell size.
    fn compute_cell_metrics(&self, line_height: Option<f32>) -> CellMetrics {
        let font = self.faces[self.families[0].regular].font();
        let metrics = font.metrics(&[]).scale(self.size);
        let advance = font
            .glyph_metrics(&[])
            .scale(self.size)
            .advance_width(font.charmap().map('0'));

        let width = advance.round().max(1.0) as u32;
        let text_height = metrics.ascent + metrics.descent;
        let height = match line_height {
            Some(lh) => (self.size * lh).round(),
            None => text_height.ceil(),
        }
        .max(1.0) as u32;
        let baseline = ((height as f32 - text_height) / 2.0 + metrics.ascent).round() as i32;
        let stroke = metrics.stroke_size.round().max(1.0) as u32;

        CellMetrics {
            width,
            height,
            baseline,
            underline_top: baseline - metrics.underline_offset.round() as i32,
            strikeout_top: baseline - metrics.strikeout_offset.round() as i32,
            stroke,
        }
    }

    pub fn is_bitmap(&self) -> bool {
        self.bitmap.is_some()
    }

    /// The largest size at which the first family's characters fit the cell.
    fn size_to_fit_cell(&self) -> f32 {
        let font = self.faces[self.families[0].regular].font();
        let metrics = font.metrics(&[]).scale(1.0);
        let advance = font
            .glyph_metrics(&[])
            .scale(1.0)
            .advance_width(font.charmap().map('0'));
        let by_height = self.cell.height as f32 / (metrics.ascent + metrics.descent);
        let by_width = self.cell.width as f32 / advance;
        by_height.min(by_width).max(1.0)
    }

    /// Finds the face to draw `ch` with. Returns the face and whether bold
    /// must be synthesized.
    fn face_for(&mut self, ch: char, bold: bool) -> Option<(usize, bool)> {
        for family in &self.families {
            if self.faces[family.regular].has(ch) {
                if !bold {
                    return Some((family.regular, false));
                }
                return Some(match family.bold {
                    Some(b) if self.faces[b].has(ch) => (b, false),
                    _ => (family.regular, true),
                });
            }
        }
        self.fallback(ch).map(|face| (face, bold))
    }

    fn fallback(&mut self, ch: char) -> Option<usize> {
        if let Some(&face) = self.fallback_for.get(&ch) {
            return face;
        }

        let emoji = ch as u32 >= 0x1F000;
        // Fallback fonts already in use are likely to cover related
        // characters too (think CJK text), so try them before searching.
        let mut found = self
            .fallbacks
            .iter()
            .copied()
            .filter(|&f| !emoji || self.is_color(f))
            .find(|&f| self.faces[f].has(ch));

        if found.is_none() {
            found = self.search_system(ch, emoji);
            if let Some(f) = found {
                if !self.fallbacks.contains(&f) {
                    self.fallbacks.push(f);
                }
            }
        }

        self.fallback_for.insert(ch, found);
        found
    }

    fn is_color(&self, face: usize) -> bool {
        let font = self.faces[face].font();
        // COLR, CBDT and sbix tables mark color fonts.
        [b"COLR", b"CBDT", b"sbix"]
            .iter()
            .any(|tag| font.table(u32::from_be_bytes(**tag)).is_some())
    }

    fn search_system(&mut self, ch: char, emoji: bool) -> Option<usize> {
        let db = self.system();

        // Prefer upright, regular-weight faces, and monospaced ones first.
        let mut candidates: Vec<_> = db
            .faces()
            .map(|f| {
                let emoji_rank = f
                    .families
                    .iter()
                    .filter_map(|(name, _)| EMOJI_FAMILIES.iter().position(|e| e == name))
                    .min()
                    .unwrap_or(EMOJI_FAMILIES.len());
                let rank = (
                    if emoji { emoji_rank } else { 0 },
                    !f.monospaced,
                    f.style != fontdb::Style::Normal,
                    f.weight.0.abs_diff(400),
                );
                (rank, f.id)
            })
            .collect();
        candidates.sort_by_key(|(rank, _)| *rank);

        candidates.into_iter().find_map(|(_, id)| {
            let covers = self
                .system()
                .with_face_data(id, |data, index| {
                    FontRef::from_index(data, index as usize)
                        .is_some_and(|f| f.charmap().map(ch) != 0)
                })
                .unwrap_or(false);
            if covers { self.face_for_id(id) } else { None }
        })
    }

    /// The rendered glyph for `ch`, which occupies `cells` cells.
    pub fn glyph(&mut self, ch: char, bold: bool, italic: bool, cells: u32) -> Option<Rc<Glyph>> {
        let key = GlyphKey { ch, bold, italic };
        if let Some(g) = self.glyphs.get(&key) {
            return g.clone();
        }
        let glyph = self.render(key, cells).map(Rc::new);
        self.glyphs.insert(key, glyph.clone());
        glyph
    }

    fn render(&mut self, key: GlyphKey, cells: u32) -> Option<Glyph> {
        if let Some(bitmap) = &self.bitmap {
            if let Some(glyph) = bitmap_glyph(bitmap, key.ch, key.bold, cells) {
                return Some(glyph);
            }
        }

        let (face, synthetic_bold) = self.face_for(key.ch, key.bold)?;
        let max_width = self.cell.width * cells;
        let max_height = self.cell.height;

        let mut size = self.size;
        let mut glyph = self.render_with(face, key, synthetic_bold, size)?;

        // Color bitmaps (emoji) come in fixed sizes that may not fit the
        // cell; render them again so that they do.
        if matches!(glyph.bitmap, Bitmap::Color(_))
            && (glyph.width > max_width || glyph.height > max_height)
        {
            let fit = (max_width as f32 / glyph.width as f32)
                .min(max_height as f32 / glyph.height as f32);
            size *= fit;
            glyph = self.render_with(face, key, synthetic_bold, size)?;
        }

        Some(glyph)
    }

    fn render_with(
        &mut self,
        face: usize,
        key: GlyphKey,
        embolden: bool,
        size: f32,
    ) -> Option<Glyph> {
        let font = self.faces[face].font();
        let id = font.charmap().map(key.ch);
        if id == 0 {
            return None;
        }

        let mut scaler = self.scale.builder(font).size(size).hint(true).build();
        let sources = [
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ];
        let mut render = Render::new(&sources);
        render.format(Format::Alpha);
        if embolden {
            render.embolden(size / 24.0);
        }
        if key.italic {
            render.transform(Some(Transform::skew(
                Angle::from_degrees(12.0),
                Angle::ZERO,
            )));
        }

        let image = render.render(&mut scaler, id)?;
        let bitmap = match image.content {
            Content::Mask => Bitmap::Mask(image.data),
            Content::Color => Bitmap::Color(image.data),
            Content::SubpixelMask => return None,
        };

        Some(Glyph {
            left: image.placement.left,
            top: image.placement.top,
            width: image.placement.width,
            height: image.placement.height,
            bitmap,
        })
    }
}

fn bitmap_cell_metrics(font: &PcfFont) -> CellMetrics {
    let baseline = font.ascent.max(1);
    CellMetrics {
        width: font.width,
        height: (font.ascent + font.descent).max(1) as u32,
        baseline,
        // Like xterm: the underline in the row below the baseline.
        underline_top: baseline + i32::from(font.descent > 1),
        strikeout_top: baseline - (font.ascent * 3 / 8).max(1),
        stroke: 1,
    }
}

/// A glyph from the bitmap font. Without a bold font, bold is drawn by
/// overstriking one pixel to the right, as xterm does.
fn bitmap_glyph(fonts: &BitmapFonts, ch: char, bold: bool, cells: u32) -> Option<Glyph> {
    let from = |g: PcfGlyph| Glyph {
        left: g.left,
        top: g.top,
        width: g.width,
        height: g.height,
        bitmap: Bitmap::Mask(g.mask),
    };

    if bold {
        if let Some(g) = fonts.bold.as_ref().and_then(|b| b.glyph(ch)) {
            return Some(from(g));
        }
    }
    let g = fonts.regular.glyph(ch).or_else(|| {
        // The wide font only for wide characters: it may also contain
        // narrow ones, drawn differently from the regular font.
        fonts.wide.as_ref().filter(|_| cells == 2)?.glyph(ch)
    })?;
    if !bold || g.width == 0 {
        return Some(from(g));
    }

    let (w, h) = (g.width as usize, g.height as usize);
    let mut mask = vec![0u8; (w + 1) * h];
    for y in 0..h {
        for x in 0..w {
            if g.mask[y * w + x] != 0 {
                mask[y * (w + 1) + x] = 255;
                mask[y * (w + 1) + x + 1] = 255;
            }
        }
    }
    Some(Glyph {
        left: g.left,
        top: g.top,
        width: g.width + 1,
        height: g.height,
        bitmap: Bitmap::Mask(mask),
    })
}
