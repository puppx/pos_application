//! Printing text the printer's own fonts cannot show (Thai, mostly).
//!
//! ESC/POS printers only know a few code pages, so a line with Thai in it is
//! shaped with the bundled Sarabun font (rustybuzz places the vowels and tone
//! marks), drawn into a 1-bit image and sent as a raster graphic (GS v 0),
//! which every ESC/POS compatible printer supports.

use std::sync::OnceLock;

use ab_glyph_rasterizer::{point, Point, Rasterizer};
use rustybuzz::ttf_parser::{GlyphId, OutlineBuilder};
use rustybuzz::{Face, UnicodeBuffer};

use serde::{Deserialize, Serialize};

use crate::printing::Size;

/// Sarabun Bold, SIL Open Font License 1.1 (see fonts/OFL.txt).
static FONT: &[u8] = include_bytes!("../fonts/Sarabun-Bold.ttf");

fn face() -> &'static Face<'static> {
    static FACE: OnceLock<Face<'static>> = OnceLock::new();
    FACE.get_or_init(|| Face::from_slice(FONT, 0).expect("bundled font is valid"))
}

/// Dots per character of the printer's normal font (Font A is 12 x 24).
pub(crate) const DOTS_PER_CHAR: usize = 12;

/// Largest raster command sent to a printer (bytes of image data).
const BAND_BYTES: usize = 2048;

/// "Small pieces": rows per raster command, and never more than
/// [`SMALL_BAND_BYTES`] (16 rows on 80 mm paper, 21 on 58 mm).
pub(crate) const SMALL_BAND_ROWS: usize = 24;
const SMALL_BAND_BYTES: usize = 1024;

/// Rows per stripe of the column bit image (ESC * 33, 24-dot double density).
const STRIPE: usize = 24;

/// How a printer is sent images (Thai text, the app's font, team bars).
/// Printers differ: cheap clones may not take the standard command, or only
/// small pieces of it. "Test Thai printing" prints a sample in every mode so
/// the owner can pick the one that works.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImageMode {
    /// GS v 0 raster image in bands of at most 2 KB (all printers so far).
    #[default]
    Raster,
    /// ESC * 33 column bit image in 24-dot stripes: the oldest image command,
    /// known to nearly every ESC/POS printer. Also cancels Chinese mode.
    Column,
    /// GS v 0 in small bands (at most 24 rows and 1 KB), for printers with a
    /// tiny buffer. Also cancels Chinese mode.
    RasterSmall,
}

impl ImageMode {
    /// Every mode, in the order the Thai test page prints them: the most
    /// widely supported first, the standard raster last, as a raster command
    /// the printer does not take prints its image as junk that can upset
    /// everything after it.
    pub const TEST_ORDER: [ImageMode; 3] = [ImageMode::Column, ImageMode::RasterSmall, ImageMode::Raster];

    /// Name in the settings (plain ASCII: printed on the test page).
    pub fn label(self) -> &'static str {
        match self {
            ImageMode::Raster => "Standard",
            ImageMode::Column => "Compatible (older printers)",
            ImageMode::RasterSmall => "Small pieces",
        }
    }

    /// Bytes after the printer's init. Outside the standard mode this adds
    /// FS . (cancel Chinese character mode): many Chinese printers start in
    /// GB18030 double-byte mode, where a byte of 0x80 or more takes the next
    /// byte with it. Printers without Chinese mode ignore it. The standard
    /// mode sends exactly what it always has.
    pub(crate) fn init_extra(self) -> &'static [u8] {
        match self {
            ImageMode::Raster => &[],
            ImageMode::Column | ImageMode::RasterSmall => &[0x1C, b'.'],
        }
    }
}

/// Font size in dots, chosen to sit well next to the printer's own text.
fn px(size: Size) -> f32 {
    match size {
        Size::Normal => 28.0,
        Size::Tall => 40.0,
        Size::Big => 48.0,
    }
}

/// True when some character has no place in code page 437.
pub(crate) fn needs_raster(text: &str) -> bool {
    text.chars().any(|c| c != '?' && crate::printing::cp437(c) == b'?')
}

struct Shaped {
    /// (glyph, x offset, y offset, advance, cluster byte index), in dots.
    glyphs: Vec<(u16, f32, f32, f32, usize)>,
}

fn shape(text: &str, scale: f32) -> Shaped {
    let mut buf = UnicodeBuffer::new();
    buf.push_str(text);
    let out = rustybuzz::shape(face(), &[], buf);
    let glyphs = out
        .glyph_infos()
        .iter()
        .zip(out.glyph_positions())
        .map(|(i, p)| {
            (i.glyph_id as u16, p.x_offset as f32 * scale, p.y_offset as f32 * scale, p.x_advance as f32 * scale, i.cluster as usize)
        })
        .collect();
    Shaped { glyphs }
}

impl Shaped {
    fn width(&self) -> f32 {
        self.glyphs.iter().map(|g| g.3).sum()
    }
}

/// Split `text` into pieces at most `room` dots wide. Breaks at spaces when
/// it can, otherwise between characters (Thai is written without spaces).
fn wrap_dots(text: &str, room: f32, scale: f32) -> Vec<String> {
    let mut lines = Vec::new();
    let mut rest = text.trim().to_string();
    while !rest.is_empty() {
        let shaped = shape(&rest, scale);
        if shaped.width() <= room {
            lines.push(rest);
            break;
        }
        // Width up to the start of each cluster.
        let mut fit = 0; // byte index of the last cluster start that fits
        let mut pen = 0.0;
        let mut i = 0;
        let g = &shaped.glyphs;
        while i < g.len() {
            let cluster = g[i].4;
            let mut adv = 0.0;
            let mut j = i;
            while j < g.len() && g[j].4 == cluster {
                adv += g[j].3;
                j += 1;
            }
            if pen + adv > room {
                break;
            }
            pen += adv;
            fit = if j < g.len() { g[j].4 } else { rest.len() };
            i = j;
        }
        if fit == 0 {
            // Not even one character fits: take it anyway.
            fit = rest.char_indices().nth(1).map_or(rest.len(), |(b, _)| b);
        }
        let mut cut = fit;
        if let Some(sp) = rest[..fit].rfind(' ').filter(|&sp| sp > 0) {
            if !rest[fit..].starts_with(' ') {
                cut = sp;
            }
        }
        lines.push(rest[..cut].trim_end().to_string());
        rest = rest[cut..].trim_start().to_string();
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Draws glyph outlines into a coverage rasterizer.
struct Pen<'a> {
    r: &'a mut Rasterizer,
    scale: f32,
    /// Glyph origin within the rasterizer, in dots.
    ox: f32,
    oy: f32,
    last: Point,
    start: Point,
}

impl Pen<'_> {
    fn at(&self, x: f32, y: f32) -> Point {
        point(self.ox + x * self.scale, self.oy - y * self.scale)
    }
}

impl OutlineBuilder for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.last = self.at(x, y);
        self.start = self.last;
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let p = self.at(x, y);
        self.r.draw_line(self.last, p);
        self.last = p;
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (c, p) = (self.at(x1, y1), self.at(x, y));
        self.r.draw_quad(self.last, c, p);
        self.last = p;
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (c1, c2, p) = (self.at(x1, y1), self.at(x2, y2), self.at(x, y));
        self.r.draw_cubic(self.last, c1, c2, p);
        self.last = p;
    }
    fn close(&mut self) {
        if self.last != self.start {
            self.r.draw_line(self.last, self.start);
        }
        self.last = self.start;
    }
}

/// A 1-bit image, one bit per dot, rows padded to whole bytes.
pub(crate) struct Bitmap {
    pub width: usize,
    pub height: usize,
    pub bits: Vec<u8>,
}

impl Bitmap {
    fn row_bytes(&self) -> usize {
        self.width.div_ceil(8)
    }

    fn set(&mut self, x: usize, y: usize) {
        if x < self.width && y < self.height {
            let rb = self.row_bytes();
            self.bits[y * rb + x / 8] |= 0x80 >> (x % 8);
        }
    }

    #[cfg(test)]
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.dot(x, y)
    }

    /// The image in `mode`.
    pub(crate) fn escpos_in(&self, mode: ImageMode) -> Vec<u8> {
        let rb = self.row_bytes().max(1);
        match mode {
            ImageMode::Raster => self.escpos(),
            ImageMode::RasterSmall => self.raster_bands(SMALL_BAND_ROWS.min(SMALL_BAND_BYTES / rb).max(1)),
            ImageMode::Column => self.columns(),
        }
    }

    /// GS v 0: print a raster image, in bands of at most [`BAND_BYTES`]
    /// each. Some printers refuse a bigger image and print its bytes as
    /// text instead (a big Thai table name came out as rows of junk).
    fn escpos(&self) -> Vec<u8> {
        self.raster_bands((BAND_BYTES / self.row_bytes().max(1)).max(1))
    }

    /// GS v 0 commands of at most `band` rows each.
    fn raster_bands(&self, band: usize) -> Vec<u8> {
        let rb = self.row_bytes();
        let mut b = Vec::with_capacity(8 * self.height.div_ceil(band) + self.bits.len());
        for (i, rows) in self.bits.chunks(band * rb).enumerate() {
            let h = rows.len() / rb.max(1);
            debug_assert!(i * band + h <= self.height);
            b.extend_from_slice(&[0x1D, b'v', b'0', 0, (rb & 0xFF) as u8, (rb >> 8) as u8]);
            b.extend_from_slice(&[(h & 0xFF) as u8, (h >> 8) as u8]);
            b.extend_from_slice(rows);
        }
        b
    }

    /// ESC * 33 (24-dot double density): the image in stripes 24 dots tall,
    /// each a column of 3 bytes per dot across (top dot = most significant
    /// bit of the first byte), ended by LF. Line spacing is 24 dots while it
    /// prints (ESC 3 24) so the stripes touch, then back to normal (ESC 2).
    /// The last stripe is padded with white rows.
    fn columns(&self) -> Vec<u8> {
        let n = self.width;
        let stripes = self.height.div_ceil(STRIPE);
        let mut b = Vec::with_capacity(5 + stripes * (6 + 3 * n));
        b.extend_from_slice(&[0x1B, b'3', STRIPE as u8]);
        for s in 0..stripes {
            b.extend_from_slice(&[0x1B, b'*', 33, (n & 0xFF) as u8, (n >> 8) as u8]);
            for x in 0..n {
                for k in 0..3 {
                    let mut byte = 0u8;
                    for bit in 0..8 {
                        let y = s * STRIPE + k * 8 + bit;
                        if y < self.height && self.dot(x, y) {
                            byte |= 0x80 >> bit;
                        }
                    }
                    b.push(byte);
                }
            }
            b.push(b'\n');
        }
        b.extend_from_slice(&[0x1B, b'2']);
        b
    }

    fn dot(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.row_bytes() + x / 8] & (0x80 >> (x % 8)) != 0
    }
}

/// Draw one line of text, `indent` dots from the left (or centred), on an
/// image `dots` wide.
fn draw(text: &str, scale: f32, dots: usize, indent: usize, center: bool) -> Bitmap {
    let f = face();
    let ascent = f.ascender() as f32 * scale;
    let descent = -(f.descender() as f32) * scale;
    let height = (ascent + descent).ceil() as usize + 2;
    let mut bmp = Bitmap { width: dots, height, bits: vec![0; dots.div_ceil(8) * height] };
    let shaped = shape(text, scale);
    let mut x = if center { (dots as f32 - shaped.width()).max(0.0) / 2.0 } else { indent as f32 };
    let baseline = ascent + 1.0;
    for &(gid, dx, dy, adv, _) in &shaped.glyphs {
        let gx = x + dx;
        let gy = baseline - dy;
        if let Some(bb) = f.glyph_bounding_box(GlyphId(gid)) {
            let left = (gx + bb.x_min as f32 * scale).floor();
            let top = (gy - bb.y_max as f32 * scale).floor();
            let w = ((bb.x_max - bb.x_min) as f32 * scale).ceil() as usize + 2;
            let h = ((bb.y_max - bb.y_min) as f32 * scale).ceil() as usize + 2;
            let mut r = Rasterizer::new(w, h);
            let mut pen = Pen { r: &mut r, scale, ox: gx - left, oy: gy - top, last: point(0.0, 0.0), start: point(0.0, 0.0) };
            f.outline_glyph(GlyphId(gid), &mut pen);
            r.for_each_pixel_2d(|px, py, v| {
                let (bx, by) = (left as i64 + px as i64, top as i64 + py as i64);
                if v >= 0.5 && bx >= 0 && by >= 0 {
                    bmp.set(bx as usize, by as usize);
                }
            });
        }
        x += adv;
    }
    bmp
}

/// True when `text` fits on one line `dots` wide in the app's font.
pub(crate) fn fits_line(text: &str, size: Size, dots: usize) -> bool {
    let scale = px(size) / face().units_per_em() as f32;
    shape(text.trim(), scale).width() <= dots as f32
}

/// Images for one laid-out line, wrapped to the paper. Leading spaces in
/// `text` become the indent of every wrapped piece.
pub(crate) fn line_images(text: &str, size: Size, center: bool, dots: usize) -> Vec<Bitmap> {
    let scale = px(size) / face().units_per_em() as f32;
    let spaces = text.chars().take_while(|c| *c == ' ').count();
    let indent = (spaces * if size == Size::Big { DOTS_PER_CHAR * 2 } else { DOTS_PER_CHAR }).min(dots / 2);
    let room = (dots - if center { 0 } else { indent }) as f32;
    wrap_dots(text, room, scale).iter().map(|l| draw(l, scale, dots, indent, center)).collect()
}

/// A big header: white text on a solid black bar `dots` wide. `text` is the
/// name, then "\n" and a smaller second name. Kept short (about 100 dots for
/// one line) because large black areas print slowly on cheap thermal heads.
pub(crate) fn banner(text: &str, dots: usize) -> Bitmap {
    let em = face().units_per_em() as f32;
    let mut parts: Vec<Bitmap> = Vec::new();
    // A leading \u{1} asks for the huge size (the table on a "call mains" ticket).
    let (huge, text) = match text.strip_prefix('\u{1}') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let mut lines = text.split('\n');
    for (t, px) in [(lines.next(), if huge { 104.0 } else { 32.0 }), (lines.next(), 26.0)] {
        let Some(t) = t.map(str::trim).filter(|t| !t.is_empty()) else { continue };
        let scale = px / em;
        for piece in wrap_dots(t, dots.saturating_sub(24) as f32, scale) {
            parts.push(draw(&piece, scale, dots, 0, true));
        }
    }
    let pad = if huge { 8 } else { 4 };
    let rb = dots.div_ceil(8);
    let height = parts.iter().map(|p| p.height).sum::<usize>() + 2 * pad;
    // All black, then each line of text laid in as white ink.
    let mut out = Bitmap { width: dots, height, bits: vec![0xFF; rb * height] };
    let mut y = pad;
    for p in &parts {
        for row in 0..p.height {
            for b in 0..rb {
                out.bits[(y + row) * rb + b] = !p.bits[row * rb + b];
            }
        }
        y += p.height;
    }
    out
}

/// ESC/POS bytes printing a header bar (see [`banner`]) in the standard mode.
#[cfg(test)]
pub(crate) fn escpos_banner(text: &str, dots: usize) -> Vec<u8> {
    escpos_banner_in(text, dots, ImageMode::Raster)
}

/// ESC/POS bytes printing a header bar (see [`banner`]) in `mode`.
pub(crate) fn escpos_banner_in(text: &str, dots: usize, mode: ImageMode) -> Vec<u8> {
    let mut b = vec![0x1B, b'a', 0];
    b.extend(banner(text, dots).escpos_in(mode));
    b
}

/// ESC/POS bytes printing `text` as images (see [`line_images`]) in the
/// standard mode.
#[cfg(test)]
pub(crate) fn escpos_line(text: &str, size: Size, center: bool, dots: usize) -> Vec<u8> {
    escpos_line_in(text, size, center, dots, ImageMode::Raster)
}

/// ESC/POS bytes printing `text` as images (see [`line_images`]) in `mode`.
pub(crate) fn escpos_line_in(text: &str, size: Size, center: bool, dots: usize, mode: ImageMode) -> Vec<u8> {
    let mut b = vec![0x1B, b'a', 0];
    for img in line_images(text, size, center, dots) {
        b.extend(img.escpos_in(mode));
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn big_images_go_out_in_small_bands() {
        // A tall banner (the huge table name) at 80 mm: many rows of 72 bytes.
        let img = banner("\u{1}โต๊ะ 12", 576);
        assert!(img.bits.len() > BAND_BYTES);
        let b = img.escpos();
        let (mut i, mut rows, mut data) = (0, 0, Vec::new());
        while i < b.len() {
            assert_eq!(&b[i..i + 4], &[0x1D, b'v', b'0', 0]);
            let rb = usize::from(b[i + 4]) | usize::from(b[i + 5]) << 8;
            let h = usize::from(b[i + 6]) | usize::from(b[i + 7]) << 8;
            assert_eq!(rb, 72);
            assert!(rb * h <= BAND_BYTES && h > 0);
            data.extend_from_slice(&b[i + 8..i + 8 + rb * h]);
            rows += h;
            i += 8 + rb * h;
        }
        // Every row printed once, in order.
        assert_eq!((rows, data), (img.height, img.bits));
    }

    /// Decode an ESC 3 24 / ESC * 33 ... / ESC 2 stream back into rows of
    /// dots (`width` wide, a multiple of 24 rows).
    fn decode_columns(b: &[u8]) -> (usize, Vec<Vec<bool>>) {
        assert_eq!(&b[..3], &[0x1B, b'3', 24], "line spacing 24 dots first");
        assert_eq!(&b[b.len() - 2..], &[0x1B, b'2'], "line spacing back to normal after");
        let (mut i, end, mut width, mut rows) = (3, b.len() - 2, None, Vec::new());
        while i < end {
            assert_eq!(&b[i..i + 3], &[0x1B, b'*', 33], "stripe header at {i}");
            let n = usize::from(b[i + 3]) | usize::from(b[i + 4]) << 8;
            assert_eq!(*width.get_or_insert(n), n, "every stripe as wide");
            let data = &b[i + 5..i + 5 + 3 * n];
            assert_eq!(b[i + 5 + 3 * n], b'\n', "each stripe ends with LF");
            for y in 0..24 {
                // 3 bytes per column, the top dot in the most significant bit.
                rows.push((0..n).map(|x| data[3 * x + y / 8] & (0x80 >> (y % 8)) != 0).collect());
            }
            i += 6 + 3 * n;
        }
        assert_eq!(i, end);
        (width.unwrap(), rows)
    }

    #[test]
    fn column_mode_decodes_back_to_the_image() {
        for dots in [384, 504, 576] {
            let mut imgs = line_images("ทดสอบภาษาไทย ผัดไทย 120.00", Size::Normal, false, dots);
            imgs.push(banner("\u{1}โต๊ะ 12", dots));
            imgs.push(banner("CURRY / แกง", dots));
            for img in imgs {
                let (w, rows) = decode_columns(&img.escpos_in(ImageMode::Column));
                // As wide as the paper (42 x 12 = 504 dots on 80 mm, 384 on 58 mm).
                assert_eq!(w, dots);
                assert_eq!(rows.len(), img.height.div_ceil(24) * 24);
                for (y, row) in rows.iter().enumerate() {
                    for (x, &dot) in row.iter().enumerate() {
                        // The image itself, then white padding to the last stripe.
                        assert_eq!(dot, y < img.height && img.get(x, y), "dot {x},{y} at {dots}");
                    }
                }
            }
        }
        // A known pattern: one dot at the top left, one at row 23 of column 1,
        // and a full row 8 (the top bit of the second byte of every column).
        let mut img = Bitmap { width: 16, height: 24, bits: vec![0; 2 * 24] };
        img.set(0, 0);
        img.set(1, 23);
        (0..16).for_each(|x| img.set(x, 8));
        let b = img.escpos_in(ImageMode::Column);
        assert_eq!(&b[3..8], &[0x1B, b'*', 33, 16, 0]);
        assert_eq!(&b[8..14], &[0x80, 0x80, 0x00, 0x00, 0x80, 0x01]);
        assert_eq!(b.len(), 3 + 5 + 48 + 1 + 2);
    }

    #[test]
    fn small_pieces_mode_sends_bands_of_at_most_24_rows() {
        for dots in [384, 504, 576] {
            let img = banner("\u{1}โต๊ะ 12", dots);
            let b = img.escpos_in(ImageMode::RasterSmall);
            let (mut i, mut data, mut bands) = (0, Vec::new(), 0);
            while i < b.len() {
                assert_eq!(&b[i..i + 4], &[0x1D, b'v', b'0', 0]);
                let rb = usize::from(b[i + 4]) | usize::from(b[i + 5]) << 8;
                let h = usize::from(b[i + 6]) | usize::from(b[i + 7]) << 8;
                assert_eq!(rb, dots / 8);
                assert!(h > 0 && h <= SMALL_BAND_ROWS && rb * h <= SMALL_BAND_BYTES, "band {rb}x{h}");
                data.extend_from_slice(&b[i + 8..i + 8 + rb * h]);
                i += 8 + rb * h;
                bands += 1;
            }
            assert_eq!(data, img.bits, "every row once, in order");
            assert!(bands > img.escpos().len() / 2048, "more pieces than the standard mode");
        }
    }

    #[test]
    fn standard_mode_is_unchanged() {
        let img = banner("\u{1}โต๊ะ 12", 504);
        assert_eq!(img.escpos_in(ImageMode::Raster), img.escpos());
        assert_eq!(escpos_line_in("ผัดไทย", Size::Tall, false, 504, ImageMode::Raster), escpos_line("ผัดไทย", Size::Tall, false, 504));
        assert_eq!(escpos_banner_in("แกง", 384, ImageMode::Raster), escpos_banner("แกง", 384));
        assert!(ImageMode::Raster.init_extra().is_empty());
        assert_eq!(ImageMode::default(), ImageMode::Raster);
        // The other modes leave Chinese mode (FS .).
        assert_eq!(ImageMode::Column.init_extra(), &[0x1C, b'.']);
        assert_eq!(ImageMode::RasterSmall.init_extra(), &[0x1C, b'.']);
        // Labels print in the printer's own font: plain ASCII.
        assert!(ImageMode::TEST_ORDER.iter().all(|m| m.label().is_ascii()));
    }

    #[test]
    fn detects_thai() {
        assert!(needs_raster("ผัดไทย"));
        assert!(needs_raster("2 x Pad Thai ผัดไทย"));
        assert!(!needs_raster("2 x Crème brûlée £5?"));
    }

    #[test]
    fn draws_ink_inside_the_image() {
        let imgs = line_images("    ผัดไทยกุ้งสด", Size::Tall, false, 504);
        assert_eq!(imgs.len(), 1);
        let img = &imgs[0];
        assert_eq!(img.width, 504);
        let ink: Vec<usize> = (0..img.width).filter(|&x| (0..img.height).any(|y| img.get(x, y))).collect();
        assert!(!ink.is_empty());
        // Indented by 4 characters (48 dots) and not touching the far edge.
        assert!(ink[0] >= 46 && ink[0] < 70, "starts at {}", ink[0]);
        assert!(*ink.last().unwrap() < 400);
    }

    #[test]
    fn wraps_long_thai_and_keeps_marks_with_their_letter() {
        let long = "ข้าวผัดกระเพราหมูสับไข่ดาวพิเศษเผ็ดมากไม่ใส่ถั่วฝักยาว";
        let scale = px(Size::Tall) / face().units_per_em() as f32;
        let parts = wrap_dots(long, 200.0, scale);
        assert!(parts.len() > 1);
        assert_eq!(parts.concat(), long);
        for p in &parts {
            assert!(shape(p, scale).width() <= 200.0, "{p}");
            // No piece starts with a vowel or tone mark that belongs to the
            // previous letter.
            let first = p.chars().next().unwrap();
            assert!(!('\u{0E31}'..='\u{0E3A}').contains(&first) && !('\u{0E47}'..='\u{0E4E}').contains(&first), "{p}");
        }
        assert_eq!(wrap_dots("Pad Thai with prawns", 120.0, scale).concat().replace(' ', ""), "PadThaiwithprawns");
    }

    #[test]
    fn banner_is_a_black_bar_with_white_text() {
        let bar = banner("CURRY / แกง", 504);
        assert_eq!(bar.width, 504);
        // One slim line of text.
        assert!(bar.height > 30 && bar.height < 80, "{}", bar.height);
        let black = (0..bar.height).flat_map(|y| (0..504).map(move |x| (x, y))).filter(|&(x, y)| bar.get(x, y)).count();
        let total = 504 * bar.height;
        assert!(black * 100 / total > 70, "mostly black: {black}/{total}");
        assert!(black < total, "has white text in it");
        // Top and bottom rows are solid black; some middle row has white in it.
        assert!((0..504).all(|x| bar.get(x, 0) && bar.get(x, bar.height - 1)));
        assert!((0..bar.height).any(|y| (0..504).any(|x| !bar.get(x, y))));
        assert_eq!(&escpos_banner("MAIN", 384)[..7], &[0x1B, b'a', 0, 0x1D, b'v', b'0', 0]);
    }

    #[test]
    fn escpos_raster_header() {
        let b = escpos_line("ผัดไทย", Size::Normal, true, 384);
        assert_eq!(&b[..3], &[0x1B, b'a', 0]);
        assert_eq!(&b[3..7], &[0x1D, b'v', b'0', 0]);
        assert_eq!((b[7], b[8]), (48, 0)); // 384 dots = 48 bytes per row
        let rows = b[9] as usize + ((b[10] as usize) << 8);
        assert_eq!(b.len(), 11 + 48 * rows);
    }
}
