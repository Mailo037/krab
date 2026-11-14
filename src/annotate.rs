//! Annotation objects: what the markup tools create.

use crate::geom::{dist_to_segment, Pt, RectF};
use crate::gfx::{text, Canvas, Color};
use tiny_skia::{BlendMode, LineCap, LineJoin, Path, PathBuilder, Rect, Stroke, Transform};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Tool {
    Select,
    Pen,
    Highlighter,
    Line,
    Arrow,
    Rect,
    Ellipse,
    Text,
    /// A speech bubble pointing at something.
    Callout,
    /// Numbered markers: 1, 2, 3...
    Step,
    /// A dimension line showing its length in pixels.
    Measure,
    Pixelate,
    Blur,
    Eraser,
    Pipette,
}

impl Tool {
    /// Tools whose result is tinted with the current colour and size.
    pub fn has_style(self) -> bool {
        matches!(
            self,
            Tool::Pen
                | Tool::Highlighter
                | Tool::Line
                | Tool::Arrow
                | Tool::Rect
                | Tool::Ellipse
                | Tool::Text
                | Tool::Callout
                | Tool::Step
                | Tool::Measure
        )
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Stroke {
        pts: Vec<Pt>,
        highlighter: bool,
    },
    Line {
        a: Pt,
        b: Pt,
        arrow: bool,
    },
    Rect(RectF),
    Ellipse(RectF),
    /// `pos` is the top-left corner of the first line; `size` the em size in px.
    Text {
        pos: Pt,
        text: String,
        size: f32,
    },
    Pixelate(RectF),
    Blur(RectF),
    /// A numbered marker of radius `r`.
    Step {
        at: Pt,
        n: u32,
        r: f32,
    },
    /// A bubble whose text starts at `pos` and whose tail points at `tip`.
    Callout {
        tip: Pt,
        pos: Pt,
        text: String,
        size: f32,
    },
    /// A dimension line; `size` is the label's em size.
    Measure {
        a: Pt,
        b: Pt,
        size: f32,
    },
    /// A solid box (used to redact).
    Fill(RectF),
}

/// Text colour that reads well on `bg`.
fn ink_on(bg: Color) -> Color {
    if bg.luma() > 170.0 {
        Color::rgb(0x1C, 0x1C, 0x1E)
    } else {
        Color::WHITE
    }
}

/// The bubble of a callout whose text starts at `pos`.
pub fn callout_box(pos: Pt, text: &str, size: f32) -> RectF {
    let (tw, th) = text_size(if text.is_empty() { " " } else { text }, size);
    let pad = size * 0.55;
    RectF::new(
        pos.x - pad,
        pos.y - pad * 0.6,
        tw.max(size * 0.5) + 2.0 * pad,
        th + 1.2 * pad,
    )
}

/// "240 px", plus the horizontal and vertical extent when slanted.
pub fn measure_label(a: Pt, b: Pt) -> String {
    let (dx, dy) = ((b.x - a.x).abs().round(), (b.y - a.y).abs().round());
    let len = a.dist(b).round();
    if dx >= 1.0 && dy >= 1.0 {
        format!("{len} px  \u{00B7}  {dx} \u{00D7} {dy}")
    } else {
        format!("{len} px")
    }
}

/// Where the label of a dimension line goes.
fn measure_label_rect(a: Pt, b: Pt, size: f32) -> RectF {
    let label = measure_label(a, b);
    let tw = text::ui().measure(&label, size);
    let (w, h) = (tw + size * 1.2, size * 1.9);
    let mid = a.lerp(b, 0.5);
    let len = a.dist(b).max(0.01);
    // Beside the line, on its upper / left side.
    let (mut nx, mut ny) = (-(b.y - a.y) / len, (b.x - a.x) / len);
    if ny > 0.0 || (ny == 0.0 && nx > 0.0) {
        nx = -nx;
        ny = -ny;
    }
    // Far enough along the normal that the whole label clears the line.
    let off = (nx.abs() * w + ny.abs() * h) / 2.0 + size * 0.5;
    RectF::new(mid.x + nx * off - w / 2.0, mid.y + ny * off - h / 2.0, w, h)
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub shape: Shape,
    pub color: Color,
    /// Stroke width in pixels.
    pub width: f32,
}

/// A handle that reshapes a markup object once it is picked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    Nw,
    N,
    Ne,
    E,
    Se,
    S,
    Sw,
    W,
    /// The first end point of a line.
    A,
    /// The second end point of a line, or the tip of a callout.
    B,
}

/// `from` with the corner or edge `grip` dragged to `p`. With `keep_ratio`
/// a corner scales the box evenly, anchored at the opposite corner.
fn regrip(from: RectF, grip: Grip, p: Pt, keep_ratio: bool) -> RectF {
    let (x0, y0, x1, y1) = (from.x, from.y, from.right(), from.bottom());
    let corner = match grip {
        Grip::Nw => Some(((x0, y0), (x1, y1))),
        Grip::Ne => Some(((x1, y0), (x0, y1))),
        Grip::Se => Some(((x1, y1), (x0, y0))),
        Grip::Sw => Some(((x0, y1), (x1, y0))),
        _ => None,
    };
    if let (true, Some(((cx, cy), (ax, ay)))) = (keep_ratio, corner) {
        let (vx, vy) = (cx - ax, cy - ay);
        let len2 = vx * vx + vy * vy;
        if len2 > 0.0 {
            let k = (((p.x - ax) * vx + (p.y - ay) * vy) / len2).max(0.05);
            return RectF::from_points(Pt::new(ax, ay), Pt::new(ax + vx * k, ay + vy * k));
        }
    }
    let (mut x0, mut y0, mut x1, mut y1) = (x0, y0, x1, y1);
    match grip {
        Grip::N | Grip::Ne | Grip::Nw => y0 = p.y,
        Grip::S | Grip::Se | Grip::Sw => y1 = p.y,
        _ => {}
    }
    match grip {
        Grip::W | Grip::Nw | Grip::Sw => x0 = p.x,
        Grip::E | Grip::Ne | Grip::Se => x1 = p.x,
        _ => {}
    }
    RectF::from_points(Pt::new(x0, y0), Pt::new(x1, y1))
}

fn shift(p: &mut Pt, dx: f32, dy: f32) {
    p.x += dx;
    p.y += dy;
}

/// The frozen screenshot annotations are drawn over (needed by pixelate).
pub struct Source<'a> {
    pub px: &'a [u32],
    pub w: u32,
    pub h: u32,
}

fn smooth_path(pts: &[Pt]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let first = *pts.first()?;
    pb.move_to(first.x, first.y);
    match pts.len() {
        1 => pb.line_to(first.x + 0.01, first.y),
        2 => pb.line_to(pts[1].x, pts[1].y),
        n => {
            // Quadratic midpoint smoothing turns jittery mouse samples into curves.
            for i in 1..n - 1 {
                let mid = pts[i].lerp(pts[i + 1], 0.5);
                pb.quad_to(pts[i].x, pts[i].y, mid.x, mid.y);
            }
            pb.line_to(pts[n - 1].x, pts[n - 1].y);
        }
    }
    pb.finish()
}

fn highlighter_width(width: f32) -> f32 {
    width * 3.4 + 6.0
}

impl Annotation {
    /// `scale` is the monitor's UI scale factor.
    pub fn draw(&self, c: &mut Canvas, src: &Source, scale: f32) {
        match &self.shape {
            Shape::Stroke { pts, highlighter } => {
                let Some(path) = smooth_path(pts) else { return };
                if *highlighter {
                    let stroke = Stroke {
                        width: highlighter_width(self.width),
                        line_cap: LineCap::Round,
                        line_join: LineJoin::Round,
                        ..Stroke::default()
                    };
                    c.stroke_path_ex(
                        &path,
                        &stroke,
                        self.color.alpha(0.42),
                        BlendMode::SourceOver,
                        Transform::identity(),
                    );
                } else {
                    c.stroke_path(&path, self.width, self.color);
                }
            }
            Shape::Line { a, b, arrow } => {
                if !*arrow {
                    c.line(*a, *b, self.width, self.color);
                    return;
                }
                let len = a.dist(*b);
                if len < 0.5 {
                    return;
                }
                let head = (self.width * 3.2 + 8.0 * scale).min(len);
                let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
                let base = Pt::new(b.x - ux * head, b.y - uy * head);
                let half = head * 0.42;
                c.line(
                    *a,
                    Pt::new(b.x - ux * head * 0.85, b.y - uy * head * 0.85),
                    self.width,
                    self.color,
                );
                let mut pb = PathBuilder::new();
                pb.move_to(b.x, b.y);
                pb.line_to(base.x - uy * half, base.y + ux * half);
                pb.line_to(base.x + uy * half, base.y - ux * half);
                pb.close();
                if let Some(p) = pb.finish() {
                    c.fill_path(&p, self.color);
                    // A thin round-joined outline softens the tip.
                    c.stroke_path(&p, (self.width * 0.35).max(1.0), self.color);
                }
            }
            Shape::Rect(r) => {
                let radius = (self.width * 0.6).min(r.w.min(r.h) / 2.0);
                c.stroke_rrect(*r, radius, self.width, self.color);
            }
            Shape::Ellipse(r) => {
                if let Some(rect) = Rect::from_xywh(r.x, r.y, r.w.max(0.1), r.h.max(0.1)) {
                    let mut pb = PathBuilder::new();
                    pb.push_oval(rect);
                    if let Some(p) = pb.finish() {
                        c.stroke_path(&p, self.width, self.color);
                    }
                }
            }
            Shape::Text { pos, text, size } => {
                let face = text::ui();
                let m = face.metrics(*size);
                for (i, line) in text.split('\n').enumerate() {
                    face.draw(
                        c,
                        line,
                        pos.x,
                        pos.y + m.ascent + i as f32 * m.line_height,
                        *size,
                        self.color,
                    );
                }
            }
            Shape::Pixelate(r) => {
                c.pixelate(
                    src.px,
                    src.w,
                    src.h,
                    r.round(),
                    (9.0 * scale).round() as i32,
                );
            }
            Shape::Blur(r) => {
                c.blur(
                    src.px,
                    src.w,
                    src.h,
                    r.round(),
                    (7.0 * scale).round() as i32,
                );
            }
            Shape::Fill(r) => c.fill_rect(*r, self.color),
            Shape::Step { at, n, r } => {
                c.fill_circle(*at, *r, self.color);
                c.stroke_circle(
                    *at,
                    *r,
                    (r * 0.11).max(1.0),
                    Color::rgba(255, 255, 255, 230),
                );
                let label = n.to_string();
                let em = r * if label.len() > 1 { 0.92 } else { 1.12 };
                text::ui().draw_centered(c, &label, at.x, at.y, em, ink_on(self.color));
            }
            Shape::Callout {
                tip,
                pos,
                text: body,
                size,
            } => {
                let b = callout_box(*pos, body, *size);
                if !b.contains(*tip) {
                    let mid = b.center();
                    let len = mid.dist(*tip).max(0.01);
                    let (nx, ny) = (-(tip.y - mid.y) / len, (tip.x - mid.x) / len);
                    let half = (b.w.min(b.h) * 0.22).max(4.0);
                    let mut pb = PathBuilder::new();
                    pb.move_to(mid.x + nx * half, mid.y + ny * half);
                    pb.line_to(tip.x, tip.y);
                    pb.line_to(mid.x - nx * half, mid.y - ny * half);
                    pb.close();
                    if let Some(p) = pb.finish() {
                        c.fill_path(&p, self.color);
                    }
                }
                c.fill_rrect(b, size * 0.5, self.color);
                let face = text::ui();
                let m = face.metrics(*size);
                for (i, line) in body.split('\n').enumerate() {
                    face.draw(
                        c,
                        line,
                        pos.x,
                        pos.y + m.ascent + i as f32 * m.line_height,
                        *size,
                        ink_on(self.color),
                    );
                }
            }
            Shape::Measure { a, b, size } => {
                let len = a.dist(*b);
                if len < 0.5 {
                    return;
                }
                let w = self.width.clamp(1.0, 3.0 * scale);
                let (ux, uy) = ((b.x - a.x) / len, (b.y - a.y) / len);
                let tick = 5.0 * scale + w;
                c.line(*a, *b, w, self.color);
                for e in [a, b] {
                    c.line(
                        Pt::new(e.x - uy * tick, e.y + ux * tick),
                        Pt::new(e.x + uy * tick, e.y - ux * tick),
                        w,
                        self.color,
                    );
                }
                let r = measure_label_rect(*a, *b, *size);
                c.fill_rrect(r, r.h / 2.0, Color::rgba(0x14, 0x15, 0x18, 225));
                text::ui().draw_centered(
                    c,
                    &measure_label(*a, *b),
                    r.center().x,
                    r.center().y,
                    *size,
                    Color::WHITE,
                );
            }
        }
    }

    pub fn bounds(&self) -> RectF {
        match &self.shape {
            Shape::Stroke { pts, highlighter } => {
                let pad = if *highlighter {
                    highlighter_width(self.width)
                } else {
                    self.width
                } / 2.0;
                let mut r = RectF::new(pts[0].x, pts[0].y, 0.0, 0.0);
                for p in pts {
                    r = r.union(&RectF::new(p.x, p.y, 0.0, 0.0));
                }
                r.inflate(pad)
            }
            Shape::Line { a, b, .. } => RectF::from_points(*a, *b).inflate(self.width * 2.0),
            Shape::Rect(r) | Shape::Ellipse(r) => r.inflate(self.width / 2.0),
            Shape::Pixelate(r) | Shape::Blur(r) | Shape::Fill(r) => *r,
            Shape::Step { at, r, .. } => {
                RectF::new(at.x - r, at.y - r, 2.0 * r, 2.0 * r).inflate(1.0)
            }
            Shape::Callout {
                tip,
                pos,
                text,
                size,
            } => callout_box(*pos, text, *size).union(&RectF::new(tip.x, tip.y, 0.0, 0.0)),
            Shape::Measure { a, b, size } => RectF::from_points(*a, *b)
                .inflate(self.width * 2.0 + 8.0)
                .union(&measure_label_rect(*a, *b, *size)),
            Shape::Text { pos, text, size } => {
                let (w, h) = text_size(text, *size);
                RectF::new(pos.x, pos.y, w, h)
            }
        }
    }

    /// The box the object is resized by.
    pub fn frame(&self) -> RectF {
        match &self.shape {
            Shape::Stroke { pts, .. } => {
                let mut r = RectF::new(pts[0].x, pts[0].y, 0.0, 0.0);
                for p in pts {
                    r = r.union(&RectF::new(p.x, p.y, 0.0, 0.0));
                }
                r
            }
            Shape::Line { a, b, .. } | Shape::Measure { a, b, .. } => RectF::from_points(*a, *b),
            Shape::Rect(r)
            | Shape::Ellipse(r)
            | Shape::Pixelate(r)
            | Shape::Blur(r)
            | Shape::Fill(r) => *r,
            Shape::Text { pos, text, size } => {
                let (w, h) = text_size(text, *size);
                RectF::new(pos.x, pos.y, w, h)
            }
            Shape::Step { at, r, .. } => RectF::new(at.x - r, at.y - r, 2.0 * r, 2.0 * r),
            Shape::Callout {
                pos, text, size, ..
            } => callout_box(*pos, text, *size),
        }
    }

    /// The end points of an object reshaped by them instead of a box.
    pub fn ends(&self) -> Option<(Pt, Pt)> {
        match &self.shape {
            Shape::Line { a, b, .. } | Shape::Measure { a, b, .. } => Some((*a, *b)),
            Shape::Stroke { pts, .. } if pts.len() == 2 => Some((pts[0], pts[1])),
            _ => None,
        }
    }

    /// Whether a colour and size apply (pixelate, blur and redaction boxes
    /// have neither).
    pub fn has_style(&self) -> bool {
        !matches!(
            self.shape,
            Shape::Pixelate(_) | Shape::Blur(_) | Shape::Fill(_)
        )
    }

    /// The handles of a picked object and where they sit.
    pub fn grips(&self) -> Vec<(Grip, Pt)> {
        if let Some((a, b)) = self.ends() {
            return vec![(Grip::A, a), (Grip::B, b)];
        }
        let f = self.frame();
        let (cx, cy) = (f.x + f.w / 2.0, f.y + f.h / 2.0);
        let mut grips = vec![
            (Grip::Nw, Pt::new(f.x, f.y)),
            (Grip::Ne, Pt::new(f.right(), f.y)),
            (Grip::Se, Pt::new(f.right(), f.bottom())),
            (Grip::Sw, Pt::new(f.x, f.bottom())),
        ];
        match &self.shape {
            // Text and markers only scale evenly: corners are enough.
            Shape::Text { .. } | Shape::Step { .. } => {}
            Shape::Callout { tip, .. } => grips.push((Grip::B, *tip)),
            _ => grips.extend([
                (Grip::N, Pt::new(cx, f.y)),
                (Grip::E, Pt::new(f.right(), cy)),
                (Grip::S, Pt::new(cx, f.bottom())),
                (Grip::W, Pt::new(f.x, cy)),
            ]),
        }
        grips
    }

    /// Move the whole object.
    pub fn translate(&mut self, dx: f32, dy: f32) {
        match &mut self.shape {
            Shape::Stroke { pts, .. } => pts.iter_mut().for_each(|p| shift(p, dx, dy)),
            Shape::Line { a, b, .. } | Shape::Measure { a, b, .. } => {
                shift(a, dx, dy);
                shift(b, dx, dy);
            }
            Shape::Rect(r)
            | Shape::Ellipse(r)
            | Shape::Pixelate(r)
            | Shape::Blur(r)
            | Shape::Fill(r) => {
                r.x += dx;
                r.y += dy;
            }
            Shape::Text { pos, .. } => shift(pos, dx, dy),
            Shape::Step { at, .. } => shift(at, dx, dy),
            Shape::Callout { tip, pos, .. } => {
                shift(tip, dx, dy);
                shift(pos, dx, dy);
            }
        }
    }

    /// This object with `grip` dragged to `p`. With `keep_ratio` a corner
    /// scales the object evenly (text, markers and callouts always do).
    pub fn with_grip(&self, grip: Grip, p: Pt, keep_ratio: bool) -> Annotation {
        let mut out = self.clone();
        match (&mut out.shape, grip) {
            (Shape::Line { a, .. } | Shape::Measure { a, .. }, Grip::A) => *a = p,
            (Shape::Line { b, .. } | Shape::Measure { b, .. }, Grip::B) => *b = p,
            (Shape::Stroke { pts, .. }, Grip::A) if pts.len() == 2 => pts[0] = p,
            (Shape::Stroke { pts, .. }, Grip::B) if pts.len() == 2 => pts[1] = p,
            (Shape::Callout { tip, .. }, Grip::B) => *tip = p,
            (_, Grip::A | Grip::B) => {}
            _ => {
                let from = self.frame();
                let even = matches!(
                    self.shape,
                    Shape::Text { .. } | Shape::Step { .. } | Shape::Callout { .. }
                );
                out.refit(from, regrip(from, grip, p, keep_ratio || even));
            }
        }
        out
    }

    /// Stretch the object from the box `from` to the box `to`.
    fn refit(&mut self, from: RectF, to: RectF) {
        let sx = if from.w > 0.0 { to.w / from.w } else { 1.0 };
        let sy = if from.h > 0.0 { to.h / from.h } else { 1.0 };
        let map = |q: Pt| Pt::new(to.x + (q.x - from.x) * sx, to.y + (q.y - from.y) * sy);
        match &mut self.shape {
            Shape::Stroke { pts, .. } => pts.iter_mut().for_each(|q| *q = map(*q)),
            Shape::Line { a, b, .. } | Shape::Measure { a, b, .. } => {
                *a = map(*a);
                *b = map(*b);
            }
            Shape::Rect(r)
            | Shape::Ellipse(r)
            | Shape::Pixelate(r)
            | Shape::Blur(r)
            | Shape::Fill(r) => *r = to,
            Shape::Text { pos, size, .. } => {
                *size = (*size * sy).max(4.0);
                *pos = Pt::new(to.x, to.y);
            }
            Shape::Step { at, r, .. } => {
                *r = (to.w.min(to.h) / 2.0).max(3.0);
                *at = to.center();
            }
            // The bubble scales; the tail keeps pointing where it did.
            Shape::Callout { pos, size, .. } => {
                let s = (*size * sy).max(4.0);
                *size = s;
                let pad = s * 0.55;
                *pos = Pt::new(to.x + pad, to.y + pad * 0.6);
            }
        }
    }

    /// Whether `p` touches the visible ink (used by the eraser).
    pub fn hit(&self, p: Pt, tol: f32) -> bool {
        match &self.shape {
            Shape::Stroke { pts, highlighter } => {
                let reach = tol
                    + if *highlighter {
                        highlighter_width(self.width)
                    } else {
                        self.width
                    } / 2.0;
                if pts.len() == 1 {
                    return pts[0].dist(p) <= reach;
                }
                pts.windows(2)
                    .any(|s| dist_to_segment(p, s[0], s[1]) <= reach)
            }
            Shape::Line { a, b, .. } => dist_to_segment(p, *a, *b) <= tol + self.width / 2.0,
            Shape::Rect(r) => {
                let reach = tol + self.width / 2.0;
                r.inflate(reach).contains(p) && !r.inflate(-reach).contains(p)
            }
            Shape::Ellipse(r) => {
                let (rx, ry) = (r.w / 2.0, r.h / 2.0);
                if rx < 0.5 || ry < 0.5 {
                    return r.inflate(tol).contains(p);
                }
                let c = r.center();
                let d = (((p.x - c.x) / rx).powi(2) + ((p.y - c.y) / ry).powi(2)).sqrt();
                (d - 1.0).abs() * rx.min(ry) <= tol + self.width / 2.0
            }
            Shape::Text { .. } | Shape::Pixelate(_) | Shape::Blur(_) | Shape::Fill(_) => {
                self.bounds().inflate(tol).contains(p)
            }
            Shape::Step { at, r, .. } => at.dist(p) <= r + tol,
            Shape::Callout {
                tip,
                pos,
                text,
                size,
            } => {
                let b = callout_box(*pos, text, *size);
                b.inflate(tol).contains(p) || dist_to_segment(p, b.center(), *tip) <= tol + 3.0
            }
            Shape::Measure { a, b, size } => {
                dist_to_segment(p, *a, *b) <= tol + self.width
                    || measure_label_rect(*a, *b, *size).contains(p)
            }
        }
    }
}

/// Width and height of a (possibly multi-line) text block.
pub fn text_size(text: &str, size: f32) -> (f32, f32) {
    let face = text::ui();
    let m = face.metrics(size);
    let lines = text.split('\n').count().max(1);
    let w = text
        .split('\n')
        .map(|l| face.measure(l, size))
        .fold(0.0f32, f32::max);
    (w, lines as f32 * m.line_height)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ann(shape: Shape) -> Annotation {
        Annotation {
            shape,
            color: Color::rgb(255, 0, 0),
            width: 4.0,
        }
    }

    #[test]
    fn rect_hit_is_outline_only() {
        let a = ann(Shape::Rect(RectF::new(10.0, 10.0, 100.0, 60.0)));
        assert!(a.hit(Pt::new(10.0, 40.0), 3.0));
        assert!(!a.hit(Pt::new(60.0, 40.0), 3.0));
        assert!(!a.hit(Pt::new(200.0, 40.0), 3.0));
    }

    #[test]
    fn ellipse_hit_follows_the_curve() {
        let a = ann(Shape::Ellipse(RectF::new(0.0, 0.0, 100.0, 50.0)));
        assert!(a.hit(Pt::new(100.0, 25.0), 3.0));
        assert!(a.hit(Pt::new(50.0, 0.0), 3.0));
        assert!(!a.hit(Pt::new(50.0, 25.0), 3.0));
        assert!(!a.hit(Pt::new(0.0, 0.0), 3.0)); // bounding-box corner is empty
    }

    #[test]
    fn arrow_draws_ink_between_endpoints() {
        let (w, h) = (80u32, 40u32);
        let mut buf = vec![0xFF00_0000u32; (w * h) as usize];
        let src_px = buf.clone();
        let src = Source { px: &src_px, w, h };
        let mut c = Canvas::new(&mut buf, w, h);
        ann(Shape::Line {
            a: Pt::new(10.0, 20.0),
            b: Pt::new(70.0, 20.0),
            arrow: true,
        })
        .draw(&mut c, &src, 1.0);
        assert_eq!(buf[(20 * w + 40) as usize], 0xFFFF_0000, "shaft");
        assert_eq!(buf[(20 * w + 66) as usize], 0xFFFF_0000, "head");
        assert_eq!(buf[(5 * w + 40) as usize], 0xFF00_0000, "untouched");
    }

    #[test]
    fn new_shapes_draw_inside_their_bounds() {
        let (w, h) = (300u32, 220u32);
        let shapes = [
            Shape::Step {
                at: Pt::new(150.0, 110.0),
                n: 12,
                r: 16.0,
            },
            Shape::Callout {
                tip: Pt::new(40.0, 190.0),
                pos: Pt::new(120.0, 60.0),
                text: "Look here\nand here".into(),
                size: 16.0,
            },
            Shape::Measure {
                a: Pt::new(40.0, 150.0),
                b: Pt::new(260.0, 90.0),
                size: 12.0,
            },
            Shape::Blur(RectF::new(60.0, 40.0, 120.0, 80.0)),
            Shape::Fill(RectF::new(10.0, 10.0, 30.0, 12.0)),
        ];
        let src_px: Vec<u32> = (0..w * h)
            .map(|i| {
                if (i % w / 4 + i / w / 4) % 2 == 0 {
                    0xFF00_0000
                } else {
                    0xFFFF_FFFF
                }
            })
            .collect();
        for shape in shapes {
            let a = ann(shape);
            let mut buf = src_px.clone();
            let src = Source { px: &src_px, w, h };
            a.draw(&mut Canvas::new(&mut buf, w, h), &src, 1.0);
            let b = a.bounds().inflate(1.5);
            let mut changed = 0;
            for (i, (p, q)) in buf.iter().zip(&src_px).enumerate() {
                if p != q {
                    changed += 1;
                    let (x, y) = ((i as u32 % w) as f32, (i as u32 / w) as f32);
                    assert!(
                        b.contains(Pt::new(x, y)),
                        "{:?} drew outside {b:?} at {x},{y}",
                        a.shape
                    );
                }
            }
            assert!(changed > 50, "{:?} drew almost nothing", a.shape);
            let probe = match &a.shape {
                Shape::Callout {
                    pos, text, size, ..
                } => callout_box(*pos, text, *size).center(),
                Shape::Measure { a, b, .. } => a.lerp(*b, 0.5),
                _ => b.center(),
            };
            assert!(a.hit(probe, 4.0), "{:?} not hit at {probe:?}", a.shape);
        }
    }

    #[test]
    fn measure_label_reads_length_and_extent() {
        assert_eq!(
            measure_label(Pt::new(0.0, 0.0), Pt::new(120.0, 0.0)),
            "120 px"
        );
        assert_eq!(
            measure_label(Pt::new(0.0, 0.0), Pt::new(30.0, 40.0)),
            "50 px  \u{00B7}  30 \u{00D7} 40"
        );
    }

    #[test]
    fn grips_reshape_boxes_lines_and_text() {
        let r = ann(Shape::Rect(RectF::new(10.0, 10.0, 100.0, 50.0)));
        let wider = r.with_grip(Grip::E, Pt::new(150.0, 999.0), false);
        assert_eq!(wider.frame(), RectF::new(10.0, 10.0, 140.0, 50.0));
        // Dragged past the opposite edge, the box flips instead of inverting.
        let flipped = r.with_grip(Grip::W, Pt::new(130.0, 0.0), false);
        assert_eq!(flipped.frame(), RectF::new(110.0, 10.0, 20.0, 50.0));
        // Keeping the ratio, a corner scales from the opposite corner.
        let even = r.with_grip(Grip::Se, Pt::new(210.0, 110.0), true);
        assert_eq!(even.frame(), RectF::new(10.0, 10.0, 200.0, 100.0));

        let l = ann(Shape::Line {
            a: Pt::new(0.0, 0.0),
            b: Pt::new(10.0, 0.0),
            arrow: true,
        });
        assert_eq!(l.grips().len(), 2);
        let moved = l.with_grip(Grip::B, Pt::new(20.0, 20.0), false);
        assert_eq!(moved.ends(), Some((Pt::new(0.0, 0.0), Pt::new(20.0, 20.0))));

        let t = ann(Shape::Text {
            pos: Pt::new(0.0, 0.0),
            text: "Krab".into(),
            size: 20.0,
        });
        let f = t.frame();
        let big = t.with_grip(Grip::Se, Pt::new(f.w * 2.0, f.h * 2.0), false);
        match big.shape {
            Shape::Text { size, pos, .. } => {
                assert!((size - 40.0).abs() < 0.01);
                assert_eq!(pos, Pt::new(0.0, 0.0));
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn translate_moves_every_point() {
        let mut c = ann(Shape::Callout {
            tip: Pt::new(5.0, 5.0),
            pos: Pt::new(50.0, 50.0),
            text: "Hi".into(),
            size: 14.0,
        });
        let before = c.bounds();
        c.translate(10.0, -4.0);
        let after = c.bounds();
        assert_eq!((after.x - before.x, after.y - before.y), (10.0, -4.0));
    }

    #[test]
    fn multiline_text_bounds_grow() {
        let (w1, h1) = text_size("Krab", 20.0);
        let (w2, h2) = text_size("Krab\nKrab grabs", 20.0);
        assert!(w2 > w1 && (h2 - 2.0 * h1).abs() < 0.01);
    }
}
