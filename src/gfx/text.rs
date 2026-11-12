//! Text rendering with an embedded font.
//!
//! Glyph outlines are parsed lazily (ab_glyph / ttf-parser), so creating a
//! [`Face`] costs microseconds and only the glyphs actually shown are ever
//! rasterized. Rasterized glyphs are cached per size and sub-pixel offset.

use super::{Canvas, Color};
use ab_glyph::{point, Font as _, FontRef, Glyph, GlyphId, PxScale, ScaleFont as _};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

static UI_FONT: &[u8] = include_bytes!("../../assets/fonts/Inter-Medium.ttf");

struct Bitmap {
    w: usize,
    h: usize,
    left: i32,
    top: i32,
    data: Vec<u8>,
}

pub struct Face {
    font: FontRef<'static>,
    cache: Mutex<HashMap<(u16, u16, u8), Option<Arc<Bitmap>>>>,
}

/// The font used for all UI and annotation text.
pub fn ui() -> &'static Face {
    static FACE: OnceLock<Face> = OnceLock::new();
    FACE.get_or_init(|| Face::from_static(UI_FONT).expect("embedded font is valid"))
}

pub struct Metrics {
    pub ascent: f32,
    pub descent: f32,
    pub line_height: f32,
}

const SUBPIXEL_BINS: f32 = 4.0;

impl Face {
    pub fn from_static(data: &'static [u8]) -> Option<Face> {
        Some(Face {
            font: FontRef::try_from_slice(data).ok()?,
            cache: Mutex::new(HashMap::new()),
        })
    }

    /// ab_glyph scales by ascent-descent height; convert from an em size so
    /// sizes match what designers call "13 px text".
    fn scale(&self, em: f32) -> PxScale {
        let upem = self.font.units_per_em().unwrap_or(1000.0);
        PxScale::from(em * self.font.height_unscaled() / upem)
    }

    pub fn metrics(&self, em: f32) -> Metrics {
        let f = self.font.as_scaled(self.scale(em));
        Metrics {
            ascent: f.ascent(),
            descent: -f.descent(),
            line_height: f.height() + f.line_gap(),
        }
    }

    pub fn measure(&self, text: &str, em: f32) -> f32 {
        let f = self.font.as_scaled(self.scale(em));
        let mut w = 0.0;
        let mut prev: Option<GlyphId> = None;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            if let Some(p) = prev {
                w += f.kern(p, id);
            }
            w += f.h_advance(id);
            prev = Some(id);
        }
        w
    }

    fn bitmap(&self, id: GlyphId, em: f32, bin: u8) -> Option<Arc<Bitmap>> {
        let key = (id.0, (em * 8.0).round() as u16, bin);
        if let Some(hit) = self.cache.lock().unwrap().get(&key) {
            return hit.clone();
        }
        let glyph = Glyph {
            id,
            scale: self.scale(em),
            position: point(bin as f32 / SUBPIXEL_BINS, 0.0),
        };
        let bmp = self.font.outline_glyph(glyph).map(|og| {
            let b = og.px_bounds();
            let (w, h) = (b.width().ceil() as usize, b.height().ceil() as usize);
            let mut data = vec![0u8; w * h];
            og.draw(|x, y, c| {
                let (x, y) = (x as usize, y as usize);
                if x < w && y < h {
                    data[y * w + x] = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
            });
            Arc::new(Bitmap {
                w,
                h,
                left: b.min.x.floor() as i32,
                top: b.min.y.floor() as i32,
                data,
            })
        });
        self.cache.lock().unwrap().insert(key, bmp.clone());
        bmp
    }

    /// Draw a single line with its baseline at `y`. Returns the advance width.
    pub fn draw(&self, c: &mut Canvas, text: &str, x: f32, y: f32, em: f32, color: Color) -> f32 {
        let f = self.font.as_scaled(self.scale(em));
        let base_y = (y + c.oy).round() as i32;
        let mut pen = x + c.ox;
        let start = pen;
        let mut prev: Option<GlyphId> = None;
        for ch in text.chars() {
            let id = self.font.glyph_id(ch);
            if let Some(p) = prev {
                pen += f.kern(p, id);
            }
            let ix = pen.floor();
            let bin = (((pen - ix) * SUBPIXEL_BINS) as u8).min(SUBPIXEL_BINS as u8 - 1);
            if let Some(bmp) = self.bitmap(id, em, bin) {
                blend_glyph(c, &bmp, ix as i32 + bmp.left, base_y + bmp.top, color);
            }
            pen += f.h_advance(id);
            prev = Some(id);
        }
        pen - start
    }

    /// Draw text horizontally centered on `cx`, vertically centered on `cy`.
    pub fn draw_centered(
        &self,
        c: &mut Canvas,
        text: &str,
        cx: f32,
        cy: f32,
        em: f32,
        color: Color,
    ) {
        let w = self.measure(text, em);
        let m = self.metrics(em);
        self.draw(
            c,
            text,
            cx - w / 2.0,
            cy + (m.ascent - m.descent) / 2.0,
            em,
            color,
        );
    }

    /// Baseline that vertically centers a line of text on `cy`.
    pub fn baseline_for_center(&self, cy: f32, em: f32) -> f32 {
        let m = self.metrics(em);
        cy + (m.ascent - m.descent) / 2.0
    }
}

fn blend_glyph(c: &mut Canvas, bmp: &Bitmap, x0: i32, y0: i32, color: Color) {
    let (cw, ch) = (c.w as i32, c.h as i32);
    let (sr, sg, sb, sa) = (
        color.r as u32,
        color.g as u32,
        color.b as u32,
        color.a as u32,
    );
    for gy in 0..bmp.h as i32 {
        let y = y0 + gy;
        if y < 0 || y >= ch {
            continue;
        }
        let row = (y * cw) as usize;
        for gx in 0..bmp.w as i32 {
            let x = x0 + gx;
            if x < 0 || x >= cw {
                continue;
            }
            let cov = bmp.data[gy as usize * bmp.w + gx as usize] as u32;
            if cov == 0 {
                continue;
            }
            let a = cov * sa / 255;
            let inv = 255 - a;
            let d = &mut c.buf[row + x as usize];
            // Premultiplied source-over on all four channels, so this works
            // on both opaque screen buffers and transparent icon canvases.
            let da = (*d >> 24) & 0xFF;
            let dr = (*d >> 16) & 0xFF;
            let dg = (*d >> 8) & 0xFF;
            let db = *d & 0xFF;
            let oa = a + da * inv / 255;
            let or = (sr * a + dr * inv) / 255;
            let og = (sg * a + dg * inv) / 255;
            let ob = (sb * a + db * inv) / 255;
            *d = oa.min(255) << 24 | or << 16 | og << 8 | ob;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn measure_grows_with_text_and_size() {
        let f = ui();
        let a = f.measure("Krab", 13.0);
        let b = f.measure("Krab Krab", 13.0);
        let c = f.measure("Krab", 26.0);
        assert!(a > 10.0 && b > a * 1.9);
        assert!((c - a * 2.0).abs() < 0.5);
    }

    #[test]
    fn draw_touches_pixels_inside_bounds_only() {
        let mut buf = vec![0xFF00_0000u32; 64 * 24];
        let mut c = Canvas::new(&mut buf, 64, 24);
        // Deliberately start off-canvas to exercise clipping.
        ui().draw(&mut c, "Krab!", -6.0, 17.0, 14.0, Color::WHITE);
        assert!(buf.iter().any(|p| *p != 0xFF00_0000));
        assert!(buf.iter().all(|p| p >> 24 == 0xFF));
    }
}
