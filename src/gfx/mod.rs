//! Software rendering primitives.
//!
//! Everything is drawn on the CPU straight into a `0xAARRGGBB` `u32` buffer -
//! the exact format `softbuffer` presents - so there is no GPU context to spin
//! up and no intermediate copies. tiny-skia does the anti-aliased vector work;
//! it expects RGBA byte order, so colours are handed to it with red and blue
//! swapped (see [`Canvas::paint`]).

pub mod icons;
pub mod text;
pub mod theme;

use crate::geom::{Pt, RectF, RectI};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Paint, Path, PathBuilder, PixmapMut, Rect, Stroke,
    Transform,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const WHITE: Color = Color::rgb(255, 255, 255);
    pub const BLACK: Color = Color::rgb(0, 0, 0);

    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b, a: 255 }
    }
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
    /// `0xRRGGBB`
    pub const fn hex(v: u32) -> Self {
        Self::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
    }
    /// Scale the alpha channel by `f` (0..=1).
    pub fn alpha(self, f: f32) -> Self {
        Self {
            a: (self.a as f32 * f.clamp(0.0, 1.0)).round() as u8,
            ..self
        }
    }
    pub fn to_u32(self) -> u32 {
        0xFF00_0000 | (self.r as u32) << 16 | (self.g as u32) << 8 | self.b as u32
    }
    pub fn from_u32(p: u32) -> Self {
        Self::rgb((p >> 16) as u8, (p >> 8) as u8, p as u8)
    }
    pub fn to_hex_string(self) -> String {
        format!("#{:02X}{:02X}{:02X}", self.r, self.g, self.b)
    }
    pub fn parse(s: &str) -> Option<Color> {
        let s = s.trim().trim_start_matches('#');
        if s.len() != 6 {
            return None;
        }
        u32::from_str_radix(s, 16).ok().map(Color::hex)
    }
    /// Perceived brightness, 0..=255.
    pub fn luma(self) -> f32 {
        0.299 * self.r as f32 + 0.587 * self.g as f32 + 0.114 * self.b as f32
    }
}

/// A drawing surface over a borrowed pixel buffer.
pub struct Canvas<'a> {
    pub buf: &'a mut [u32],
    pub w: u32,
    pub h: u32,
    /// Translation applied to everything drawn (used when rendering a crop).
    pub ox: f32,
    pub oy: f32,
    /// Measuring canvas: nothing is painted, every primitive only records
    /// the device pixels it would touch (used to find what must be redrawn).
    rec: Option<&'a mut Vec<RectI>>,
}

impl<'a> Canvas<'a> {
    pub fn new(buf: &'a mut [u32], w: u32, h: u32) -> Self {
        debug_assert_eq!(buf.len(), (w * h) as usize);
        Self {
            buf,
            w,
            h,
            ox: 0.0,
            oy: 0.0,
            rec: None,
        }
    }

    /// A canvas of size `w` x `h` that records, instead of painting, a
    /// conservative bounding rect of everything drawn on it into `rec`.
    pub fn measure(w: u32, h: u32, rec: &'a mut Vec<RectI>) -> Self {
        Self {
            buf: &mut [],
            w,
            h,
            ox: 0.0,
            oy: 0.0,
            rec: Some(rec),
        }
    }

    pub fn is_measuring(&self) -> bool {
        self.rec.is_some()
    }

    /// Record device pixels (already offset). Returns true when measuring.
    pub(crate) fn record_px(&mut self, r: RectI) -> bool {
        match &mut self.rec {
            Some(rec) => {
                let r = r.intersect(&RectI::new(0, 0, self.w as i32, self.h as i32));
                if !r.is_empty() {
                    rec.push(r);
                }
                true
            }
            None => false,
        }
    }

    /// Record a canvas-space rect (before the offset), padded for
    /// anti-aliasing. Returns true when measuring.
    fn record(&mut self, r: RectF, pad: f32) -> bool {
        if self.rec.is_none() {
            return false;
        }
        let r = RectF::new(r.x + self.ox, r.y + self.oy, r.w, r.h).inflate(pad + 1.5);
        self.record_px(r.round_out())
    }

    /// Record the device-space footprint of `path` under `ts` (which
    /// already includes the offset), grown by `pad` on every side. Long
    /// strokes are recorded segment by segment, so the outline of a large
    /// rectangle does not mark its whole inside as changed.
    fn record_path(&mut self, path: &Path, ts: Transform, pad: f32, segmentwise: bool) -> bool {
        if self.rec.is_none() {
            return false;
        }
        let map = |x: f32, y: f32| (ts.sx * x + ts.kx * y + ts.tx, ts.ky * x + ts.sy * y + ts.ty);
        let bounds_of = |pts: &[(f32, f32)]| {
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for &(x, y) in pts {
                let (x, y) = map(x, y);
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x);
                y1 = y1.max(y);
            }
            RectF::new(x0, y0, x1 - x0, y1 - y0)
                .inflate(pad + 1.5)
                .round_out()
        };
        let b = path.bounds();
        let whole = bounds_of(&[
            (b.left(), b.top()),
            (b.right(), b.top()),
            (b.left(), b.bottom()),
            (b.right(), b.bottom()),
        ]);
        if !segmentwise || whole.w.min(whole.h) < 96 {
            return self.record_px(whole);
        }
        use tiny_skia::PathSegment;
        let mut last = (0.0, 0.0);
        let mut start = (0.0, 0.0);
        for seg in path.segments() {
            let r = match seg {
                PathSegment::MoveTo(p) => {
                    last = (p.x, p.y);
                    start = last;
                    continue;
                }
                PathSegment::LineTo(p) => bounds_of(&[last, (p.x, p.y)]),
                PathSegment::QuadTo(c, p) => bounds_of(&[last, (c.x, c.y), (p.x, p.y)]),
                PathSegment::CubicTo(c1, c2, p) => {
                    bounds_of(&[last, (c1.x, c1.y), (c2.x, c2.y), (p.x, p.y)])
                }
                PathSegment::Close => bounds_of(&[last, start]),
            };
            last = match seg {
                PathSegment::LineTo(p)
                | PathSegment::QuadTo(_, p)
                | PathSegment::CubicTo(_, _, p) => (p.x, p.y),
                _ => start,
            };
            self.record_px(r);
        }
        true
    }

    pub fn with_offset(mut self, ox: f32, oy: f32) -> Self {
        self.ox = ox;
        self.oy = oy;
        self
    }

    fn paint(c: Color) -> Paint<'static> {
        let mut p = Paint::default();
        // Buffer bytes are B,G,R,A - swap so tiny-skia writes the right channels.
        p.set_color_rgba8(c.b, c.g, c.r, c.a);
        p.anti_alias = true;
        p
    }

    fn ts(&self) -> Transform {
        Transform::from_translate(self.ox, self.oy)
    }

    fn pixmap(&mut self) -> Option<PixmapMut<'_>> {
        PixmapMut::from_bytes(bytemuck::cast_slice_mut(self.buf), self.w, self.h)
    }

    pub fn fill_path(&mut self, path: &Path, c: Color) {
        self.fill_path_ex(path, c, FillRule::Winding, Transform::identity());
    }

    pub fn fill_path_ex(&mut self, path: &Path, c: Color, rule: FillRule, extra: Transform) {
        let ts = extra.post_concat(self.ts());
        if self.record_path(path, ts, 0.0, false) {
            return;
        }
        let paint = Self::paint(c);
        if let Some(mut pm) = self.pixmap() {
            pm.fill_path(path, &paint, rule, ts, None);
        }
    }

    /// Stroke with round caps and joins.
    pub fn stroke_path(&mut self, path: &Path, width: f32, c: Color) {
        let stroke = Stroke {
            width,
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        self.stroke_path_ex(
            path,
            &stroke,
            c,
            BlendMode::SourceOver,
            Transform::identity(),
        );
    }

    pub fn stroke_path_ex(
        &mut self,
        path: &Path,
        stroke: &Stroke,
        c: Color,
        blend: BlendMode,
        extra: Transform,
    ) {
        let ts = extra.post_concat(self.ts());
        let k = (ts.sx.abs() + ts.kx.abs()).max(ts.ky.abs() + ts.sy.abs());
        if self.record_path(path, ts, stroke.width * k * 0.5 + 1.0, true) {
            return;
        }
        let mut paint = Self::paint(c);
        paint.blend_mode = blend;
        if let Some(mut pm) = self.pixmap() {
            pm.stroke_path(path, &paint, stroke, ts, None);
        }
    }

    /// Fill with a two-stop linear gradient running from `a` to `b`.
    pub fn fill_path_gradient(&mut self, path: &Path, a: Pt, ca: Color, b: Pt, cb: Color) {
        if self.record_path(path, self.ts(), 0.0, false) {
            return;
        }
        let stop = |t: f32, c: Color| {
            tiny_skia::GradientStop::new(t, tiny_skia::Color::from_rgba8(c.b, c.g, c.r, c.a))
        };
        let Some(shader) = tiny_skia::LinearGradient::new(
            tiny_skia::Point::from_xy(a.x, a.y),
            tiny_skia::Point::from_xy(b.x, b.y),
            vec![stop(0.0, ca), stop(1.0, cb)],
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let ts = self.ts();
        if let Some(mut pm) = self.pixmap() {
            pm.fill_path(path, &paint, FillRule::Winding, ts, None);
        }
    }

    /// Fill with a radial gradient from `inner` at `center` to `outer` at `radius`.
    pub fn fill_path_radial(
        &mut self,
        path: &Path,
        center: Pt,
        radius: f32,
        inner: Color,
        outer: Color,
    ) {
        if self.record_path(path, self.ts(), 0.0, false) {
            return;
        }
        let stop = |t: f32, c: Color| {
            tiny_skia::GradientStop::new(t, tiny_skia::Color::from_rgba8(c.b, c.g, c.r, c.a))
        };
        let p = tiny_skia::Point::from_xy(center.x, center.y);
        let Some(shader) = tiny_skia::RadialGradient::new(
            p,
            p,
            radius,
            vec![stop(0.0, inner), stop(1.0, outer)],
            tiny_skia::SpreadMode::Pad,
            Transform::identity(),
        ) else {
            return;
        };
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let ts = self.ts();
        if let Some(mut pm) = self.pixmap() {
            pm.fill_path(path, &paint, FillRule::Winding, ts, None);
        }
    }

    /// Draw another `0xAARRGGBB` buffer scaled into `dst` with rounded corners.
    pub fn draw_image(&mut self, px: &[u32], iw: u32, ih: u32, dst: RectF, radius: f32) {
        if self.record(dst, 0.0) {
            return;
        }
        let Some(src) = tiny_skia::PixmapRef::from_bytes(bytemuck::cast_slice(px), iw, ih) else {
            return;
        };
        let Some(path) = rrect_path(dst, radius) else {
            return;
        };
        let (sx, sy) = (dst.w / iw as f32, dst.h / ih as f32);
        let shader = tiny_skia::Pattern::new(
            src,
            tiny_skia::SpreadMode::Pad,
            tiny_skia::FilterQuality::Bilinear,
            1.0,
            Transform::from_scale(sx, sy).post_translate(dst.x, dst.y),
        );
        let paint = Paint {
            shader,
            anti_alias: true,
            ..Paint::default()
        };
        let ts = self.ts();
        if let Some(mut pm) = self.pixmap() {
            pm.fill_path(&path, &paint, FillRule::Winding, ts, None);
        }
    }

    pub fn fill_rect(&mut self, r: RectF, c: Color) {
        if r.is_empty() || self.record(r, 0.0) {
            return;
        }
        if c.a == 255 {
            // Opaque, pixel-aligned fills skip the rasterizer entirely.
            let ri = RectF::new(r.x + self.ox, r.y + self.oy, r.w, r.h).round();
            self.fill_rect_px(ri, c.to_u32());
            return;
        }
        if let Some(rect) = Rect::from_xywh(r.x, r.y, r.w, r.h) {
            let paint = Self::paint(c);
            let ts = self.ts();
            if let Some(mut pm) = self.pixmap() {
                pm.fill_rect(rect, &paint, ts, None);
            }
        }
    }

    /// Fill device pixels directly (no offset, no blending).
    pub fn fill_rect_px(&mut self, r: RectI, px: u32) {
        let r = r.intersect(&RectI::new(0, 0, self.w as i32, self.h as i32));
        if r.is_empty() || self.record_px(r) {
            return;
        }
        let w = self.w as usize;
        for y in r.y..r.bottom() {
            let row = y as usize * w;
            self.buf[row + r.x as usize..row + r.right() as usize].fill(px);
        }
    }

    pub fn fill_rrect(&mut self, r: RectF, radius: f32, c: Color) {
        if let Some(p) = rrect_path(r, radius) {
            self.fill_path(&p, c);
        }
    }

    pub fn stroke_rrect(&mut self, r: RectF, radius: f32, width: f32, c: Color) {
        if let Some(p) = rrect_path(r, radius) {
            self.stroke_path(&p, width, c);
        }
    }

    pub fn fill_circle(&mut self, center: Pt, radius: f32, c: Color) {
        if let Some(p) = PathBuilder::from_circle(center.x, center.y, radius.max(0.01)) {
            self.fill_path(&p, c);
        }
    }

    pub fn stroke_circle(&mut self, center: Pt, radius: f32, width: f32, c: Color) {
        if let Some(p) = PathBuilder::from_circle(center.x, center.y, radius.max(0.01)) {
            self.stroke_path(&p, width, c);
        }
    }

    pub fn line(&mut self, a: Pt, b: Pt, width: f32, c: Color) {
        let mut pb = PathBuilder::new();
        pb.move_to(a.x, a.y);
        pb.line_to(b.x, b.y);
        if let Some(p) = pb.finish() {
            self.stroke_path(&p, width, c);
        }
    }

    /// Soft drop shadow approximated with many stacked translucent shapes.
    ///
    /// The layers are rendered once per size into a cached alpha sprite, so
    /// a shadow costs a single blend pass per frame instead of 24 path fills
    /// (which made the toolbars the most expensive part of a 4K frame).
    pub fn shadow(&mut self, r: RectF, radius: f32, spread: f32, strength: f32) {
        if r.is_empty() {
            return;
        }
        let outer = RectF::new(
            r.x - spread,
            r.y - spread + spread * 0.3,
            r.w + 2.0 * spread,
            r.h + 2.0 * spread,
        );
        if self.record(outer, 1.0) {
            return;
        }
        // Whole device pixels go into the blit position; the fraction is
        // part of the sprite, so positions stay sub-pixel exact.
        let (dx, dy) = (r.x + self.ox, r.y + self.oy);
        let (ix, iy) = (dx.floor(), dy.floor());
        let sprite = shadow_sprite(
            RectF::new(dx - ix, dy - iy, r.w, r.h),
            radius,
            spread,
            strength,
        );
        let (cw, ch) = (self.w as i32, self.h as i32);
        let (x0, y0) = (ix as i32 + sprite.x, iy as i32 + sprite.y);
        for row in 0..sprite.h {
            let y = y0 + row;
            if y < 0 || y >= ch {
                continue;
            }
            let xa = x0.max(0);
            let xb = (x0 + sprite.w).min(cw);
            if xa >= xb {
                continue;
            }
            let src =
                &sprite.a[(row * sprite.w + xa - x0) as usize..(row * sprite.w + xb - x0) as usize];
            let dst = &mut self.buf[(y * cw + xa) as usize..(y * cw + xb) as usize];
            for (d, &a) in dst.iter_mut().zip(src) {
                if a == 0 {
                    continue;
                }
                // Premultiplied black over the destination.
                let inv = 255 - a as u32;
                let ch = |shift: u32| (((*d >> shift) & 0xFF) * inv + 127) / 255;
                let da = (*d >> 24) & 0xFF;
                *d = (a as u32 + (da * inv + 127) / 255).min(255) << 24
                    | ch(16) << 16
                    | ch(8) << 8
                    | ch(0);
            }
        }
    }

    /// Floating panel: shadow and fill, no outline.
    pub fn panel(&mut self, r: RectF, radius: f32, s: f32) {
        self.shadow(r, radius, 14.0 * s, 0.55);
        self.fill_rrect(r, radius, theme::PANEL);
    }

    /// Mosaic the area `r` (in canvas space) using pixels from `src`, which
    /// shares this canvas' coordinate space before the offset is applied.
    pub fn pixelate(&mut self, src: &[u32], sw: u32, sh: u32, r: RectI, block: i32) {
        let block = block.max(2);
        let r = r.intersect(&RectI::new(0, 0, sw as i32, sh as i32));
        if r.is_empty() {
            return;
        }
        if self.record(r.to_f(), 0.0) {
            return;
        }
        let (ox, oy) = (self.ox.round() as i32, self.oy.round() as i32);
        // Anchor the grid globally so blocks stay put while the area is resized.
        let mut by = r.y - r.y.rem_euclid(block);
        while by < r.bottom() {
            let mut bx = r.x - r.x.rem_euclid(block);
            while bx < r.right() {
                let cell = RectI::new(bx, by, block, block)
                    .intersect(&RectI::new(0, 0, sw as i32, sh as i32));
                if !cell.is_empty() {
                    let (mut sr, mut sg, mut sb, mut n) = (0u32, 0u32, 0u32, 0u32);
                    for y in cell.y..cell.bottom() {
                        let row = y as usize * sw as usize;
                        for p in &src[row + cell.x as usize..row + cell.right() as usize] {
                            sr += (p >> 16) & 0xFF;
                            sg += (p >> 8) & 0xFF;
                            sb += p & 0xFF;
                            n += 1;
                        }
                    }
                    let px = 0xFF00_0000 | (sr / n) << 16 | (sg / n) << 8 | (sb / n);
                    let vis = cell.intersect(&r).translate(ox, oy);
                    self.fill_rect_px(vis, px);
                }
                bx += block;
            }
            by += block;
        }
    }

    /// Draw an opaque RGBA image scaled to fit `dst` (box-sampled, for thumbnails).
    pub fn blit_rgba_fit(&mut self, rgba: &[u8], iw: u32, ih: u32, dst: RectF) {
        if iw == 0 || ih == 0 || dst.is_empty() || self.record(dst, 0.0) {
            return;
        }
        let scale = (dst.w / iw as f32).min(dst.h / ih as f32);
        let (tw, th) = ((iw as f32 * scale).max(1.0), (ih as f32 * scale).max(1.0));
        let x0 = (dst.x + (dst.w - tw) / 2.0 + self.ox).round() as i32;
        let y0 = (dst.y + (dst.h - th) / 2.0 + self.oy).round() as i32;
        let (tw, th) = (tw.round() as i32, th.round() as i32);
        for ty in 0..th {
            let y = y0 + ty;
            if y < 0 || y >= self.h as i32 {
                continue;
            }
            let sy0 = (ty as f32 / scale) as u32;
            let sy1 = (((ty + 1) as f32 / scale) as u32)
                .clamp(sy0 + 1, ih.max(sy0 + 1))
                .min(ih);
            for tx in 0..tw {
                let x = x0 + tx;
                if x < 0 || x >= self.w as i32 {
                    continue;
                }
                let sx0 = (tx as f32 / scale) as u32;
                let sx1 = (((tx + 1) as f32 / scale) as u32)
                    .clamp(sx0 + 1, iw.max(sx0 + 1))
                    .min(iw);
                // Sample at most a 4x4 grid inside the source footprint.
                let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
                let stepy = ((sy1 - sy0) / 4).max(1);
                let stepx = ((sx1 - sx0) / 4).max(1);
                let mut sy = sy0.min(ih - 1);
                while sy < sy1.max(sy0 + 1).min(ih) {
                    let mut sx = sx0.min(iw - 1);
                    while sx < sx1.max(sx0 + 1).min(iw) {
                        let i = ((sy * iw + sx) * 4) as usize;
                        r += rgba[i] as u32;
                        g += rgba[i + 1] as u32;
                        b += rgba[i + 2] as u32;
                        n += 1;
                        sx += stepx;
                    }
                    sy += stepy;
                }
                let n = n.max(1);
                self.buf[(y as u32 * self.w + x as u32) as usize] =
                    0xFF00_0000 | (r / n) << 16 | (g / n) << 8 | (b / n);
            }
        }
    }
}

/// A pre-rendered shadow: coverage of the stacked layers, relative to the
/// whole-pixel position of the shadowed rect.
struct ShadowSprite {
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    a: Vec<u8>,
}

/// The shadow for `r` (whose position only carries the sub-pixel fraction),
/// rendered on first use and cached by size.
fn shadow_sprite(r: RectF, radius: f32, spread: f32, strength: f32) -> Arc<ShadowSprite> {
    type Key = [i32; 7];
    static CACHE: OnceLock<Mutex<HashMap<Key, Arc<ShadowSprite>>>> = OnceLock::new();
    let q = |v: f32| (v * 16.0).round() as i32;
    let key = [
        q(r.x),
        q(r.y),
        q(r.w),
        q(r.h),
        q(radius),
        q(spread),
        q(strength * 100.0),
    ];
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(s) = cache.lock().unwrap().get(&key) {
        return s.clone();
    }
    let outer = RectF::new(
        r.x - spread,
        r.y - spread + spread * 0.3,
        r.w + 2.0 * spread,
        r.h + 2.0 * spread,
    )
    .inflate(2.0)
    .round_out();
    let mut px = vec![0u32; (outer.w.max(1) * outer.h.max(1)) as usize];
    {
        let mut c = Canvas::new(&mut px, outer.w.max(1) as u32, outer.h.max(1) as u32)
            .with_offset(-outer.x as f32, -outer.y as f32);
        // Many faint layers: each one is below the threshold where a band
        // would be visible, together they fall off smoothly.
        const STEPS: usize = 24;
        for i in 0..STEPS {
            let t = (i as f32 + 0.5) / STEPS as f32;
            let d = spread * t;
            let rr = RectF::new(
                r.x - d,
                r.y - d + spread * 0.3,
                r.w + 2.0 * d,
                r.h + 2.0 * d,
            );
            let a = strength * 0.075 * (1.0 - t).powf(1.5);
            c.fill_rrect(
                rr,
                radius + d,
                Color::rgba(0, 0, 0, (a * 255.0).round().min(255.0) as u8),
            );
        }
    }
    let sprite = Arc::new(ShadowSprite {
        x: outer.x,
        y: outer.y,
        w: outer.w.max(1),
        h: outer.h.max(1),
        a: px.iter().map(|p| (p >> 24) as u8).collect(),
    });
    let mut map = cache.lock().unwrap();
    // Tooltips and toasts come in many sizes; keep the cache small.
    if map.len() >= 96 {
        map.clear();
    }
    map.insert(key, sprite.clone());
    sprite
}

/// Rounded rectangle path (cubic corner approximation).
pub fn rrect_path(r: RectF, radius: f32) -> Option<Path> {
    if r.w <= 0.0 || r.h <= 0.0 {
        return None;
    }
    let rad = radius.min(r.w / 2.0).min(r.h / 2.0);
    if rad <= 0.05 {
        return Some(PathBuilder::from_rect(Rect::from_xywh(r.x, r.y, r.w, r.h)?));
    }
    let mut pb = PathBuilder::new();
    push_rrect(&mut pb, r, rad);
    pb.finish()
}

pub fn push_rrect(pb: &mut PathBuilder, r: RectF, rad: f32) {
    let rad = rad.min(r.w / 2.0).min(r.h / 2.0).max(0.0);
    let k = rad * 0.447_715_25; // 1 - 0.5522847
    let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
    pb.move_to(x0 + rad, y0);
    pb.line_to(x1 - rad, y0);
    pb.cubic_to(x1 - k, y0, x1, y0 + k, x1, y0 + rad);
    pb.line_to(x1, y1 - rad);
    pb.cubic_to(x1, y1 - k, x1 - k, y1, x1 - rad, y1);
    pb.line_to(x0 + rad, y1);
    pb.cubic_to(x0 + k, y1, x0, y1 - k, x0, y1 - rad);
    pb.line_to(x0, y0 + rad);
    pb.cubic_to(x0, y0 + k, x0 + k, y0, x0 + rad, y0);
    pb.close();
}

/// Convert a `0xAARRGGBB` buffer (alpha ignored) to tightly packed RGBA bytes.
pub fn argb_to_rgba(px: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for p in px {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, 255]);
    }
    out
}

/// Convert a buffer rendered on a transparent background (premultiplied) to
/// straight-alpha RGBA bytes.
pub fn premul_argb_to_rgba(px: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(px.len() * 4);
    for p in px {
        let a = (p >> 24) & 0xFF;
        let un = |c: u32| -> u8 {
            if a == 0 {
                0
            } else {
                ((c * 255 + a / 2) / a).min(255) as u8
            }
        };
        out.extend_from_slice(&[
            un((p >> 16) & 0xFF),
            un((p >> 8) & 0xFF),
            un(p & 0xFF),
            a as u8,
        ]);
    }
    out
}

/// Convert RGBA bytes to an opaque `0xFFRRGGBB` buffer.
pub fn rgba_to_argb(rgba: &[u8]) -> Vec<u32> {
    rgba.chunks_exact(4)
        .map(|c| 0xFF00_0000 | (c[0] as u32) << 16 | (c[1] as u32) << 8 | c[2] as u32)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_roundtrip() {
        let c = Color::parse("#F46623").unwrap();
        assert_eq!(c, Color::rgb(0xF4, 0x66, 0x23));
        assert_eq!(c.to_hex_string(), "#F46623");
        assert_eq!(Color::from_u32(c.to_u32()), c);
        assert!(Color::parse("nope").is_none());
    }

    #[test]
    fn opaque_fill_respects_offset_and_bounds() {
        let mut buf = vec![0u32; 16];
        let mut c = Canvas::new(&mut buf, 4, 4).with_offset(-1.0, -1.0);
        c.fill_rect(RectF::new(0.0, 0.0, 3.0, 3.0), Color::WHITE);
        // Only the 2x2 block that survives the -1,-1 shift is painted.
        let painted = buf.iter().filter(|p| **p == 0xFFFF_FFFF).count();
        assert_eq!(painted, 4);
        assert_eq!(buf[0], 0xFFFF_FFFF);
        assert_eq!(buf[2], 0);
    }

    #[test]
    fn channel_order_matches_buffer_format() {
        let mut buf = vec![0xFF00_0000u32; 64];
        let mut c = Canvas::new(&mut buf, 8, 8);
        c.fill_rrect(RectF::new(0.0, 0.0, 8.0, 8.0), 0.0, Color::rgb(255, 0, 0));
        assert_eq!(buf[8 * 4 + 4], 0xFFFF_0000);
    }

    #[test]
    fn pixelate_averages_blocks() {
        let src: Vec<u32> = (0..16)
            .map(|i| if i % 2 == 0 { 0xFF00_0000 } else { 0xFFFF_FFFF })
            .collect();
        let mut buf = src.clone();
        let mut c = Canvas::new(&mut buf, 4, 4);
        c.pixelate(&src, 4, 4, RectI::new(0, 0, 4, 4), 2);
        assert!(buf.iter().all(|p| *p == 0xFF7F_7F7F));
    }
}
