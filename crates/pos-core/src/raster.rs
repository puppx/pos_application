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

use crate::printing::Size;

/// Sarabun Bold, SIL Open Font License 1.1 (see fonts/OFL.txt).
static FONT: &[u8] = include_bytes!("../fonts/Sarabun-Bold.ttf");

fn face() -> &'static Face<'static> {
    static FACE: OnceLock<Face<'static>> = OnceLock::new();
    FACE.get_or_init(|| Face::from_slice(FONT, 0).expect("bundled font is valid"))
}

/// Dots per character of the printer's normal font (Font A is 12 x 24).
pub(crate) const DOTS_PER_CHAR: usize = 12;

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
        self.bits[y * self.row_bytes() + x / 8] & (0x80 >> (x % 8)) != 0
    }

    /// GS v 0: print a raster image.
    fn escpos(&self) -> Vec<u8> {
        let rb = self.row_bytes();
        let mut b = Vec::with_capacity(8 + self.bits.len());
        b.extend_from_slice(&[0x1D, b'v', b'0', 0, (rb & 0xFF) as u8, (rb >> 8) as u8]);
        b.extend_from_slice(&[(self.height & 0xFF) as u8, (self.height >> 8) as u8]);
        b.extend_from_slice(&self.bits);
        b
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

/// Images for one laid-out line, wrapped to the paper. Leading spaces in
/// `text` become the indent of every wrapped piece.
pub(crate) fn line_images(text: &str, size: Size, center: bool, dots: usize) -> Vec<Bitmap> {
    let scale = px(size) / face().units_per_em() as f32;
    let spaces = text.chars().take_while(|c| *c == ' ').count();
    let indent = (spaces * if size == Size::Big { DOTS_PER_CHAR * 2 } else { DOTS_PER_CHAR }).min(dots / 2);
    let room = (dots - if center { 0 } else { indent }) as f32;
    wrap_dots(text, room, scale).iter().map(|l| draw(l, scale, dots, indent, center)).collect()
}

/// ESC/POS bytes printing `text` as images (see [`line_images`]).
pub(crate) fn escpos_line(text: &str, size: Size, center: bool, dots: usize) -> Vec<u8> {
    let mut b = vec![0x1B, b'a', 0];
    for img in line_images(text, size, center, dots) {
        b.extend(img.escpos());
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn escpos_raster_header() {
        let b = escpos_line("ผัดไทย", Size::Normal, true, 384);
        assert_eq!(&b[..3], &[0x1B, b'a', 0]);
        assert_eq!(&b[3..7], &[0x1D, b'v', b'0', 0]);
        assert_eq!((b[7], b[8]), (48, 0)); // 384 dots = 48 bytes per row
        let rows = b[9] as usize + ((b[10] as usize) << 8);
        assert_eq!(b.len(), 11 + 48 * rows);
    }
}
