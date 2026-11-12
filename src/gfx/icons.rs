//! Krab's icon set: original, soft line icons defined on a 24x24 grid.
//!
//! Icons are described with a tiny path language - a subset of SVG path data
//! plus a few shapes that make rounded geometry easy to write:
//!
//! | command               | meaning                                              |
//! |-----------------------|------------------------------------------------------|
//! | `M x y`               | move to                                              |
//! | `L x y`               | line to (further pairs are more line-tos)            |
//! | `Q cx cy x y`         | quadratic curve                                      |
//! | `C c1x c1y c2x c2y x y` | cubic curve                                        |
//! | `Z`                   | close the current figure                             |
//! | `O cx cy r`           | circle                                               |
//! | `R x y w h r`         | rectangle with corner radius `r`                     |
//! | `P r x y x y ...`     | open polyline, every inner corner rounded by `r`     |
//! | `G r x y x y ...`     | closed polygon, every corner rounded by `r`          |
//!
//! Sources are parsed once on first use and drawn as vectors at any scale.
//! `docs/ICONS.md` explains the style rules and how to add an icon.

use super::{push_rrect, Canvas, Color};
use crate::geom::{Pt, RectF};
use std::sync::OnceLock;
use tiny_skia::{BlendMode, FillRule, LineCap, LineJoin, Path, PathBuilder, Stroke, Transform};

/// Stroke width on the 24px grid.
pub const STROKE: f32 = 1.9;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(usize)]
pub enum Icon {
    Pointer,
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
    Undo,
    Redo,
    TextScan,
    External,
    Save,
    Copy,
    Close,
    Check,
    ModeRect,
    ModeFreeform,
    ModeWindow,
    ModeFull,
    Timer,
    Camera,
    Video,
    Stop,
    Dot,
    Folder,
}

const COUNT: usize = Icon::Folder as usize + 1;

/// (stroked geometry, filled geometry)
pub fn source(icon: Icon) -> (&'static str, &'static str) {
    match icon {
        Icon::Pointer => ("G 1.6 6 3.8 6 18 10 14.6 12.5 20 15.2 18.8 12.6 13.4 17.6 13", ""),
        Icon::Pen => (
            "M6.1 14.8 L15.4 5.4 Q17 3.9 18.6 5.4 Q20.1 7 18.6 8.6 L9.2 17.9 L5.7 19.1 \
             Q4.5 19.5 4.9 18.3 Z M13.6 7.3 L16.7 10.4",
            "",
        ),
        Icon::Highlighter => (
            "G 1.8 9.2 13.4 15.4 4.8 19.6 7.9 13.4 16.5 P 1.2 9.2 13.4 8 17.2 10.6 19.1 13.4 16.5 \
             M4.5 20.5 L11 20.5",
            "",
        ),
        Icon::Line => ("M5 19 L19 5", ""),
        Icon::Arrow => ("M5 19 L18.4 5.6 P 2.6 10.5 5 19 5 19 13.5", ""),
        Icon::Rect => ("R 4 5.5 16 13 4", ""),
        Icon::Ellipse => ("O 12 12 8", ""),
        Icon::Text => (
            "P 2 5.5 8 5.5 5 18.5 5 18.5 8 M12 5 L12 19 M9.5 19 L14.5 19",
            "",
        ),
        Icon::Pixelate => (
            "R 13.2 4.5 6.3 6.3 2.2 R 4.5 13.2 6.3 6.3 2.2",
            "R 3.8 3.8 7.7 7.7 2.6 R 12.5 12.5 7.7 7.7 2.6",
        ),
        Icon::Eraser => (
            "G 2 14.6 3.8 21 10.2 12.2 19 8.3 19 3.8 14.5 M9.2 9.2 L15.6 15.6 M12.2 19 L20 19",
            "",
        ),
        Icon::Pipette => (
            "G 2.2 15.8 11.4 20.2 7 17 3.8 12.6 8.2 M10.4 8.4 L15.6 13.6 M13 11 L5.6 18.4 L4.6 19.4",
            "",
        ),
        Icon::Undo => (
            "P 2.2 8.5 14 4 9.5 8.5 5 M4 9.5 L14.5 9.5 C17.5 9.5 20 11.7 20 14.5 \
             C20 17.3 17.5 19.5 14.5 19.5 L11 19.5",
            "",
        ),
        Icon::Redo => (
            "P 2.2 15.5 14 20 9.5 15.5 5 M20 9.5 L9.5 9.5 C6.5 9.5 4 11.7 4 14.5 \
             C4 17.3 6.5 19.5 9.5 19.5 L13 19.5",
            "",
        ),
        Icon::TextScan => (
            "P 3.2 4 9 4 4.5 8.5 4.5 P 3.2 15.5 4.5 20 4.5 20 9 \
             P 3.2 20 15 20 19.5 15.5 19.5 P 3.2 8.5 19.5 4 19.5 4 15 \
             M8 9.5 L16 9.5 M8 14.5 L13.5 14.5",
            "",
        ),
        Icon::External => (
            "P 3.2 18 13.5 18 20 4 20 4 6 10.5 6 P 2.4 14.5 4 20 4 20 9.5 M19.2 4.8 L11 13",
            "",
        ),
        Icon::Save => (
            "M12 4 L12 14.6 P 2.4 7.5 10.5 12 15 16.5 10.5 M5 19.5 L19 19.5",
            "",
        ),
        Icon::Copy => (
            "R 9 9 11 11 3.2 P 2.6 5.5 15 4 15 4 4 15 4 15 5.5",
            "",
        ),
        Icon::Close => ("M6.5 6.5 L17.5 17.5 M17.5 6.5 L6.5 17.5", ""),
        Icon::Check => ("P 1.6 5 12.5 10 17.5 19 7", ""),
        Icon::ModeRect => (
            "P 2.6 4 9 4 6 7.5 6 P 2.6 16.5 6 20 6 20 9 P 2.6 20 15 20 18 16.5 18 \
             P 2.6 7.5 18 4 18 4 15 M10.5 6 L13.5 6 M10.5 18 L13.5 18 \
             M4 11.4 L4 12.6 M20 11.4 L20 12.6",
            "",
        ),
        Icon::ModeFreeform => (
            "M12.5 4.5 C17 4.5 20.5 7 20.5 10.3 C20.5 13.8 16.5 16 12 16 C7.5 16 3.5 14 3.5 10.5 \
             C3.5 7 7.5 4.5 12.5 4.5 M8 15.3 C8.6 17.6 7.6 19.4 5 20.2",
            "",
        ),
        Icon::ModeWindow => (
            "R 3.5 5 17 14 3.6 M3.5 9.6 L20.5 9.6",
            "O 6.7 7.3 0.8 O 9.2 7.3 0.8",
        ),
        Icon::ModeFull => (
            "R 3.5 4.5 17 12 3.2 M8.5 20 L15.5 20 M12 16.5 L12 20",
            "",
        ),
        Icon::Timer => (
            "O 12 13.5 7 P 1.4 12 10 12 13.5 14.3 15.4 M10 3.5 L14 3.5 M18.2 6.8 L19.4 5.6",
            "",
        ),
        Icon::Camera => (
            "G 2.4 3.5 7 7.6 7 9.4 4.8 14.6 4.8 16.4 7 20.5 7 20.5 19 3.5 19 O 12 12.9 3.3",
            "",
        ),
        Icon::Video => (
            "R 3 6.5 12.5 11 3.6 G 1.6 15.5 10.6 21 7.6 21 16.4 15.5 13.4",
            "",
        ),
        Icon::Stop => ("", "R 6.5 6.5 11 11 3.6"),
        Icon::Dot => ("", "O 12 12 5.5"),
        Icon::Folder => (
            "G 2.4 3.5 5 9.4 5 11.5 7.5 20.5 7.5 20.5 19 3.5 19 M3.5 10 L20.5 10",
            "",
        ),
    }
}

/// Append a polyline through `pts` whose corners are rounded by `radius`.
///
/// Each corner is replaced by a quadratic curve that starts and ends `radius`
/// away from the corner along its two edges. The radius shrinks on short
/// edges: an edge shared by two corners gives each of them at most half its
/// length, an edge ending in a free end point gives its whole length.
pub fn push_rounded(pb: &mut PathBuilder, pts: &[(f32, f32)], radius: f32, closed: bool) {
    let n = pts.len();
    if n < 2 {
        return;
    }
    let p = |i: usize| Pt::new(pts[i % n].0, pts[i % n].1);
    let dist = |a: Pt, b: Pt| ((b.x - a.x).powi(2) + (b.y - a.y).powi(2)).sqrt();
    let toward = |from: Pt, to: Pt, t: f32| {
        let d = dist(from, to).max(1e-6);
        Pt::new(
            from.x + (to.x - from.x) * t / d,
            from.y + (to.y - from.y) * t / d,
        )
    };
    // Rounded corner at vertex `i`, entering from `a` and leaving to `b`.
    let corner = |pb: &mut PathBuilder, a: Pt, v: Pt, b: Pt, share_a: f32, share_b: f32| {
        let t = radius
            .min(dist(a, v) * share_a)
            .min(dist(v, b) * share_b)
            .max(0.0);
        let p1 = toward(v, a, t);
        let p2 = toward(v, b, t);
        pb.line_to(p1.x, p1.y);
        pb.quad_to(v.x, v.y, p2.x, p2.y);
    };

    if closed {
        if n < 3 {
            return;
        }
        // Start halfway along the last edge so every vertex is a corner.
        let (a, b) = (p(n - 1), p(0));
        pb.move_to((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        for i in 0..n {
            corner(pb, p(i + n - 1), p(i), p(i + 1), 0.5, 0.5);
        }
        pb.close();
    } else {
        pb.move_to(pts[0].0, pts[0].1);
        for i in 1..n - 1 {
            let share_a = if i == 1 { 1.0 } else { 0.5 };
            let share_b = if i == n - 2 { 1.0 } else { 0.5 };
            corner(pb, p(i - 1), p(i), p(i + 1), share_a, share_b);
        }
        pb.line_to(pts[n - 1].0, pts[n - 1].1);
    }
}

pub fn parse(src: &str) -> Option<Path> {
    if src.trim().is_empty() {
        return None;
    }
    let mut pb = PathBuilder::new();
    // Allow commands to hug their first argument ("M6 3.5").
    let spaced: String = src
        .chars()
        .flat_map(|ch| {
            if ch.is_ascii_alphabetic() {
                vec![' ', ch, ' ']
            } else {
                vec![ch]
            }
        })
        .collect();
    let mut tokens = spaced.split_whitespace().peekable();
    let is_cmd =
        |t: &str| t.len() == 1 && t.chars().next().is_some_and(|c| c.is_ascii_alphabetic());
    let mut cmd = ' ';
    while let Some(tok) = tokens.peek().copied() {
        if is_cmd(tok) {
            cmd = tok.chars().next().unwrap();
            tokens.next();
            if cmd == 'Z' {
                pb.close();
            }
            continue;
        }
        let mut num = || -> f32 { tokens.next().and_then(|t| t.parse().ok()).unwrap_or(0.0) };
        match cmd {
            'M' => {
                let (x, y) = (num(), num());
                pb.move_to(x, y);
                cmd = 'L'; // subsequent pairs are implicit line-tos
            }
            'L' => {
                let (x, y) = (num(), num());
                pb.line_to(x, y);
            }
            'Q' => {
                let (a, b, c, d) = (num(), num(), num(), num());
                pb.quad_to(a, b, c, d);
            }
            'C' => {
                let (a, b, c, d, e, f) = (num(), num(), num(), num(), num(), num());
                pb.cubic_to(a, b, c, d, e, f);
            }
            'O' => {
                let (cx, cy, r) = (num(), num(), num());
                pb.push_circle(cx, cy, r);
            }
            'R' => {
                let (x, y, w, h, r) = (num(), num(), num(), num(), num());
                push_rrect(&mut pb, RectF::new(x, y, w, h), r);
            }
            'P' | 'G' => {
                let radius = num();
                let mut values = Vec::new();
                while let Some(t) = tokens.peek().copied() {
                    if is_cmd(t) {
                        break;
                    }
                    values.push(t.parse::<f32>().unwrap_or(0.0));
                    tokens.next();
                }
                let pts: Vec<(f32, f32)> = values.chunks_exact(2).map(|c| (c[0], c[1])).collect();
                push_rounded(&mut pb, &pts, radius, cmd == 'G');
            }
            _ => {
                tokens.next();
            }
        }
    }
    pb.finish()
}

fn defs() -> &'static Vec<(Option<Path>, Option<Path>)> {
    static DEFS: OnceLock<Vec<(Option<Path>, Option<Path>)>> = OnceLock::new();
    DEFS.get_or_init(|| {
        ALL.iter()
            .map(|i| {
                let (s, f) = source(*i);
                (parse(s), parse(f))
            })
            .collect()
    })
}

pub const ALL: [Icon; COUNT] = [
    Icon::Pointer,
    Icon::Pen,
    Icon::Highlighter,
    Icon::Line,
    Icon::Arrow,
    Icon::Rect,
    Icon::Ellipse,
    Icon::Text,
    Icon::Pixelate,
    Icon::Eraser,
    Icon::Pipette,
    Icon::Undo,
    Icon::Redo,
    Icon::TextScan,
    Icon::External,
    Icon::Save,
    Icon::Copy,
    Icon::Close,
    Icon::Check,
    Icon::ModeRect,
    Icon::ModeFreeform,
    Icon::ModeWindow,
    Icon::ModeFull,
    Icon::Timer,
    Icon::Camera,
    Icon::Video,
    Icon::Stop,
    Icon::Dot,
    Icon::Folder,
];

/// Draw `icon` centered on `center`, fitted to a `size` x `size` box.
pub fn draw(c: &mut Canvas, icon: Icon, center: Pt, size: f32, color: Color) {
    let (stroke_path, fill_path) = &defs()[icon as usize];
    let k = size / 24.0;
    let ts =
        Transform::from_scale(k, k).post_translate(center.x - size / 2.0, center.y - size / 2.0);
    if let Some(p) = fill_path {
        c.fill_path_ex(p, color, FillRule::Winding, ts);
    }
    if let Some(p) = stroke_path {
        let stroke = Stroke {
            // Keep strokes from getting hairline-thin on 1x displays.
            width: STROKE.max(1.4 / k),
            line_cap: LineCap::Round,
            line_join: LineJoin::Round,
            ..Stroke::default()
        };
        c.stroke_path_ex(p, &stroke, color, BlendMode::SourceOver, ts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_parses_to_geometry() {
        assert_eq!(ALL.len(), COUNT);
        for (i, icon) in ALL.iter().enumerate() {
            assert_eq!(*icon as usize, i, "ALL must be in declaration order");
            let (s, f) = &defs()[i];
            assert!(s.is_some() || f.is_some(), "{icon:?} has no geometry");
        }
    }

    #[test]
    fn icons_stay_inside_their_box() {
        for (i, icon) in ALL.iter().enumerate() {
            let (s, f) = &defs()[i];
            for p in [s, f].into_iter().flatten() {
                let b = p.bounds();
                assert!(
                    b.left() >= 2.0 && b.top() >= 2.0 && b.right() <= 22.0 && b.bottom() <= 22.0,
                    "{icon:?} leaves the 24px grid safe area: {b:?}"
                );
            }
        }
    }

    #[test]
    fn rounded_corners_never_overshoot_short_edges() {
        // A 2-unit square with a radius of 5 must stay a 2-unit square.
        let p = parse("G 5 10 10 12 10 12 12 10 12").unwrap();
        let b = p.bounds();
        assert!((b.left() - 10.0).abs() < 0.01 && (b.right() - 12.0).abs() < 0.01);
        assert!((b.top() - 10.0).abs() < 0.01 && (b.bottom() - 12.0).abs() < 0.01);
    }

    #[test]
    fn open_polyline_keeps_its_end_points() {
        let p = parse("P 3 4 4 12 4 12 12").unwrap();
        let pts: Vec<_> = p.points().to_vec();
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        assert_eq!((first.x, first.y), (4.0, 4.0));
        assert_eq!((last.x, last.y), (12.0, 12.0));
    }
}
