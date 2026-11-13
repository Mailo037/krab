//! Pinned snips: a snip kept on top of everything in a small window of its
//! own - for comparing, copying values off, or keeping a reference in view.
//! Rendering and hit testing live here; the window itself lives in `app`.

use crate::geom::{Pt, RectF};
use crate::gfx::{icons, theme, Canvas, Color};
use crate::output::Image;

pub struct Pin {
    pub img: Image,
    /// The snip as premultiplied `0xAARRGGBB`, ready to draw.
    px: Vec<u32>,
    pub zoom: f32,
    /// Pointer position (device pixels) while it is over the window.
    pub pointer: Option<Pt>,
}

pub const MIN_ZOOM: f32 = 0.2;
pub const MAX_ZOOM: f32 = 4.0;

impl Pin {
    pub fn new(img: Image) -> Pin {
        let px = img
            .rgba
            .chunks_exact(4)
            .map(|p| {
                let a = p[3] as u32;
                let m = |c: u8| (c as u32 * a + 127) / 255;
                a << 24 | m(p[0]) << 16 | m(p[1]) << 8 | m(p[2])
            })
            .collect();
        Pin {
            img,
            px,
            zoom: 1.0,
            pointer: None,
        }
    }

    /// Window size in device pixels at the current zoom.
    pub fn size(&self) -> (u32, u32) {
        (
            ((self.img.w as f32 * self.zoom).round() as u32).max(24),
            ((self.img.h as f32 * self.zoom).round() as u32).max(24),
        )
    }

    /// Zoom by wheel `steps` (positive enlarges). Returns whether it changed.
    pub fn zoom_by(&mut self, steps: f32) -> bool {
        let z = (self.zoom * 1.1f32.powf(steps)).clamp(MIN_ZOOM, MAX_ZOOM);
        // Snap to 100% when passing it, so the original size is easy to hit.
        let z = if (self.zoom - 1.0) * (z - 1.0) < 0.0 {
            1.0
        } else {
            z
        };
        let changed = (z - self.zoom).abs() > 1e-4;
        self.zoom = z;
        changed
    }

    /// The close button in the top-right corner (device pixels).
    pub fn close_rect(w: f32, scale: f32) -> RectF {
        let d = 22.0 * scale;
        RectF::new(w - d - 6.0 * scale, 6.0 * scale, d, d)
    }

    pub fn over_close(&self, w: f32, scale: f32) -> bool {
        self.pointer
            .is_some_and(|p| Self::close_rect(w, scale).inflate(3.0 * scale).contains(p))
    }

    pub fn render(&self, buf: &mut [u32], w: u32, h: u32, scale: f32) {
        if w == 0 || h == 0 || buf.len() != (w * h) as usize {
            return;
        }
        buf.fill(theme::PANEL_SOLID.to_u32());
        let mut c = Canvas::new(buf, w, h);
        let (fw, fh) = (w as f32, h as f32);
        c.draw_image(
            &self.px,
            self.img.w,
            self.img.h,
            RectF::new(0.0, 0.0, fw, fh),
            0.0,
        );
        let hovered = self.pointer.is_some();
        let edge = if hovered {
            theme::ACCENT
        } else {
            theme::ACCENT.alpha(0.55)
        };
        c.stroke_rrect(
            RectF::new(0.0, 0.0, fw, fh).inflate(-0.5 * scale),
            0.0,
            scale.max(1.0),
            edge,
        );
        if hovered {
            let r = Self::close_rect(fw, scale);
            let bg = if self.over_close(fw, scale) {
                theme::DANGER
            } else {
                Color::rgba(0x14, 0x15, 0x18, 200)
            };
            c.fill_circle(r.center(), r.w / 2.0, bg);
            icons::draw(
                &mut c,
                icons::Icon::Close,
                r.center(),
                r.w * 0.62,
                Color::WHITE,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin() -> Pin {
        Pin::new(Image {
            w: 120,
            h: 80,
            rgba: [200, 200, 200, 255].repeat(120 * 80),
        })
    }

    #[test]
    fn zoom_is_clamped_and_snaps_to_full_size() {
        let mut p = pin();
        assert_eq!(p.size(), (120, 80));
        assert!(p.zoom_by(3.0));
        assert_eq!(p.size(), (160, 106));
        // Coming back down passes 100% and stops there.
        assert!(p.zoom_by(-5.0));
        assert_eq!(p.zoom, 1.0);
        for _ in 0..100 {
            p.zoom_by(5.0);
        }
        assert_eq!(p.zoom, MAX_ZOOM);
        for _ in 0..100 {
            p.zoom_by(-5.0);
        }
        assert!((p.zoom - MIN_ZOOM).abs() < 1e-6 || p.zoom == 1.0);
    }

    #[test]
    fn renders_opaque_with_the_snip_inside() {
        let mut p = pin();
        p.pointer = Some(Pt::new(115.0, 10.0));
        let (w, h) = p.size();
        let mut buf = vec![0u32; (w * h) as usize];
        p.render(&mut buf, w, h, 1.0);
        assert!(buf.iter().all(|px| px >> 24 == 0xFF));
        assert_eq!(buf[(40 * w + 40) as usize], 0xFFC8_C8C8);
        assert!(p.over_close(w as f32, 1.0));
    }
}
