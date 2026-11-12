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
    Pixelate,
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
}

#[derive(Clone, Debug, PartialEq)]
pub struct Annotation {
    pub shape: Shape,
    pub color: Color,
    /// Stroke width in pixels.
    pub width: f32,
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
            Shape::Pixelate(r) => *r,
            Shape::Text { pos, text, size } => {
                let (w, h) = text_size(text, *size);
                RectF::new(pos.x, pos.y, w, h)
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
            Shape::Text { .. } | Shape::Pixelate(_) => self.bounds().inflate(tol).contains(p),
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
    fn multiline_text_bounds_grow() {
        let (w1, h1) = text_size("Krab", 20.0);
        let (w2, h2) = text_size("Krab\nKrab grabs", 20.0);
        assert!(w2 > w1 && (h2 - 2.0 * h1).abs() < 0.01);
    }
}
