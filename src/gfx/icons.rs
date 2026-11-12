//! Krab's icon set: original line icons defined on a 24x24 grid.
//!
//! Icons are described with a tiny path language (a subset of SVG path data
//! plus `O cx cy r` for circles and `R x y w h radius` for rounded rects),
//! parsed once on first use and drawn as vectors at any scale.

use super::{push_rrect, Canvas, Color};
use crate::geom::{Pt, RectF};
use std::sync::OnceLock;
use tiny_skia::{BlendMode, FillRule, LineCap, LineJoin, Path, PathBuilder, Stroke, Transform};

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
fn source(icon: Icon) -> (&'static str, &'static str) {
    match icon {
        Icon::Pointer => ("M6 3.5 L6 18 L10 14.4 L12.6 20.2 L15 19.1 L12.4 13.4 L17.6 13 Z", ""),
        Icon::Pen => (
            "M4 20 L5 15.6 L15.8 4.8 Q17 3.6 18.2 4.8 L19.2 5.8 Q20.4 7 19.2 8.2 L8.4 19 Z M13.8 6.8 L17.2 10.2",
            "",
        ),
        Icon::Highlighter => (
            "M9.2 13.4 L15.4 4.8 L19.6 7.9 L13.4 16.5 Z M9.2 13.4 L8 17.2 L10.6 19.1 L13.4 16.5 M4 21 L11.5 21",
            "",
        ),
        Icon::Line => ("M5 19 L19 5", ""),
        Icon::Arrow => ("M5 19 L19 5 M10.5 5 L19 5 L19 13.5", ""),
        Icon::Rect => ("R 4 5.5 16 13 2.5", ""),
        Icon::Ellipse => ("O 12 12 8", ""),
        Icon::Text => ("M5.5 7.5 L5.5 5 L18.5 5 L18.5 7.5 M12 5 L12 19 M9.5 19 L14.5 19", ""),
        Icon::Pixelate => ("R 13.2 4.5 6.3 6.3 1.2 R 4.5 13.2 6.3 6.3 1.2", "R 3.8 3.8 7.7 7.7 1.6 R 12.5 12.5 7.7 7.7 1.6"),
        Icon::Eraser => (
            "M13.6 4.6 Q14.6 3.6 15.6 4.6 L20.4 9.4 Q21.4 10.4 20.4 11.4 L12.3 19.5 L8 19.5 L4.4 15.9 Q3.4 14.9 4.4 13.9 Z M9 9.2 L15.8 16 M12.3 19.5 L20.5 19.5",
            "",
        ),
        Icon::Pipette => (
            "M13.2 10.8 L6 18 L4.5 19.5 M11.2 8.2 L15.8 12.8 M14.6 9.4 L17.2 6.8 Q18.6 5.4 17.2 4 M14.6 9.4 L12.4 7.2 M17.2 6.8 Q18.6 5.4 20 6.8 Q21.4 8.2 20 9.6 L17.4 12.2",
            "",
        ),
        Icon::Undo => (
            "M8.5 14 L4 9.5 L8.5 5 M4 9.5 L14.5 9.5 C17.5 9.5 20 11.7 20 14.5 C20 17.3 17.5 19.5 14.5 19.5 L11 19.5",
            "",
        ),
        Icon::Redo => (
            "M15.5 14 L20 9.5 L15.5 5 M20 9.5 L9.5 9.5 C6.5 9.5 4 11.7 4 14.5 C4 17.3 6.5 19.5 9.5 19.5 L13 19.5",
            "",
        ),
        Icon::TextScan => (
            "M4 8.5 L4 6.5 Q4 4.5 6 4.5 L8 4.5 M16 4.5 L18 4.5 Q20 4.5 20 6.5 L20 8.5 M20 15.5 L20 17.5 Q20 19.5 18 19.5 L16 19.5 M8 19.5 L6 19.5 Q4 19.5 4 17.5 L4 15.5 M8 9.5 L16 9.5 M8 14.5 L13.5 14.5",
            "",
        ),
        Icon::External => (
            "M18 13.5 L18 18 Q18 20 16 20 L6 20 Q4 20 4 18 L4 8 Q4 6 6 6 L10.5 6 M14.5 4 L20 4 L20 9.5 M20 4 L11 13",
            "",
        ),
        Icon::Save => ("M12 4 L12 15 M7.5 10.5 L12 15 L16.5 10.5 M5 19.5 L19 19.5", ""),
        Icon::Copy => ("R 9 9 11 11 2.2 M5.5 15 Q4 15 4 13.5 L4 6 Q4 4 6 4 L13.5 4 Q15 4 15 5.5", ""),
        Icon::Close => ("M6.5 6.5 L17.5 17.5 M17.5 6.5 L6.5 17.5", ""),
        Icon::Check => ("M5 12.5 L10 17.5 L19 7", ""),
        Icon::ModeRect => (
            "M4 9 L4 7.5 Q4 6 5.5 6 L7.5 6 M16.5 6 L18.5 6 Q20 6 20 7.5 L20 9 M20 15 L20 16.5 Q20 18 18.5 18 L16.5 18 M7.5 18 L5.5 18 Q4 18 4 16.5 L4 15 M10.5 6 L13.5 6 M10.5 18 L13.5 18 M4 11.3 L4 12.7 M20 11.3 L20 12.7",
            "",
        ),
        Icon::ModeFreeform => (
            "M12.5 4.5 C17 4.5 20.5 7 20.5 10.3 C20.5 13.8 16.5 16 12 16 C7.5 16 3.5 14 3.5 10.5 C3.5 7 7.5 4.5 12.5 4.5 M8 15.3 C8.6 17.6 7.6 19.4 5 20.2",
            "",
        ),
        Icon::ModeWindow => ("R 3.5 5 17 14 2.5 M3.5 9.5 L20.5 9.5", "O 6.4 7.3 0.7 O 8.9 7.3 0.7"),
        Icon::ModeFull => ("R 3.5 4.5 17 12 2 M8.5 20 L15.5 20 M12 16.5 L12 20", ""),
        Icon::Timer => ("O 12 13.5 7 M12 10 L12 13.5 L14.3 15.4 M9.8 3.5 L14.2 3.5", ""),
        Icon::Camera => (
            "M3.5 9 Q3.5 7 5.5 7 L7.6 7 L9.2 4.8 L14.8 4.8 L16.4 7 L18.5 7 Q20.5 7 20.5 9 L20.5 17 Q20.5 19 18.5 19 L5.5 19 Q3.5 19 3.5 17 Z O 12 13 3.3",
            "",
        ),
        Icon::Video => ("R 3 6.5 12.5 11 2.5 M15.5 10.8 L21 7.8 L21 16.2 L15.5 13.2", ""),
        Icon::Stop => ("", "R 6.5 6.5 11 11 2.5"),
        Icon::Dot => ("", "O 12 12 5.5"),
        Icon::Folder => (
            "M3.5 7 Q3.5 5 5.5 5 L9.3 5 L11.5 7.5 L18.5 7.5 Q20.5 7.5 20.5 9.5 L20.5 17 Q20.5 19 18.5 19 L5.5 19 Q3.5 19 3.5 17 Z",
            "",
        ),
    }
}

fn parse(src: &str) -> Option<Path> {
    if src.is_empty() {
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
    let mut cmd = ' ';
    while let Some(tok) = tokens.peek().copied() {
        if tok.len() == 1 && tok.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) {
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
            width: (1.7f32).max(1.35 / k),
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
}
