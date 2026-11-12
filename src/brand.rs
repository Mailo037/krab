//! The Krab mark, drawn as vectors so it scales from a 16px tray icon to a
//! banner without shipping any bitmap assets.

use crate::geom::{Pt, RectF};
use crate::gfx::icons::push_rounded;
use crate::gfx::{premul_argb_to_rgba, theme, Canvas, Color};
use tiny_skia::{
    BlendMode, FillRule, LineCap, LineJoin, Path, PathBuilder, Rect, Stroke, Transform,
};

const SHELL: Color = theme::ACCENT;
const SHELL_DARK: Color = theme::ACCENT_DEEP;
const INK: Color = Color::rgb(0x1B, 0x1C, 0x20);

/// A claw: a disc with a wedge cut out, opening towards `open_deg`
/// (degrees, clockwise from +x in screen space).
fn claw(cx: f32, cy: f32, r: f32, open_deg: f32, gap_deg: f32) -> Option<Path> {
    let mut pb = PathBuilder::new();
    let start = (open_deg + gap_deg / 2.0).to_radians();
    let sweep = (360.0 - gap_deg).to_radians();
    pb.move_to(
        cx - r * 0.18 * open_deg.to_radians().cos(),
        cy - r * 0.18 * open_deg.to_radians().sin(),
    );
    const STEPS: usize = 40;
    for i in 0..=STEPS {
        let a = start + sweep * i as f32 / STEPS as f32;
        pb.line_to(cx + r * a.cos(), cy + r * a.sin());
    }
    pb.close();
    pb.finish()
}

fn seg(pts: &[(f32, f32)]) -> Option<Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0, pts[0].1);
    for p in &pts[1..] {
        pb.line_to(p.0, p.1);
    }
    pb.finish()
}

/// A continuous-curvature rounded square (superellipse, n = 5): rounder than
/// a plain rounded rectangle, the shape app icons use on modern desktops.
fn squircle(r: RectF) -> Option<Path> {
    let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
    let (ax, ay) = (r.w / 2.0, r.h / 2.0);
    let mut pb = PathBuilder::new();
    const STEPS: usize = 160;
    for i in 0..STEPS {
        let t = i as f32 / STEPS as f32 * std::f32::consts::TAU;
        let (s, c) = t.sin_cos();
        let x = cx + ax * c.signum() * c.abs().powf(2.0 / 5.0);
        let y = cy + ay * s.signum() * s.abs().powf(2.0 / 5.0);
        if i == 0 {
            pb.move_to(x, y);
        } else {
            pb.line_to(x, y);
        }
    }
    pb.close();
    pb.finish()
}

/// Draw the mark into `r`. With `badge` the crab sits on a dark rounded tile
/// (app icon); without it only the crab is drawn (tray icon, inline logo).
pub fn draw_mark(c: &mut Canvas, r: RectF, badge: bool) {
    let size = r.w.min(r.h);
    let (x0, y0) = (r.x + (r.w - size) / 2.0, r.y + (r.h - size) / 2.0);

    if badge {
        let tile = RectF::new(x0, y0, size, size);
        if let Some(p) = squircle(tile) {
            c.fill_path_gradient(
                &p,
                Pt::new(x0, y0),
                Color::rgb(0x2E, 0x30, 0x3A),
                Pt::new(x0 + size, y0 + size),
                Color::rgb(0x14, 0x15, 0x19),
            );
        }
    }

    // The crab is authored in a 100x100 space; shrink it a little on the badge.
    let (k, off) = if badge {
        (size / 100.0 * 0.80, size * 0.10)
    } else {
        (size / 100.0, 0.0)
    };
    let ts = Transform::from_scale(k, k)
        .post_translate(x0 + off, y0 + off + if badge { size * 0.03 } else { 0.0 });

    let stroke = |w: f32| Stroke {
        width: w,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    let line = |c: &mut Canvas, pts: &[(f32, f32)], w: f32, col: Color| {
        if let Some(p) = seg(pts) {
            c.stroke_path_ex(&p, &stroke(w), col, BlendMode::SourceOver, ts);
        }
    };

    if badge {
        // Selection corners: the "grab", with soft rounded bends.
        let (m, l) = (4.0, 15.0);
        let col = Color::rgba(255, 255, 255, 235);
        let far = 100.0 - m;
        for pts in [
            [(m, m + l), (m, m), (m + l, m)],
            [(far - l, m), (far, m), (far, m + l)],
            [(far, far - l), (far, far), (far - l, far)],
            [(m + l, far), (m, far), (m, far - l)],
        ] {
            let mut pb = PathBuilder::new();
            push_rounded(&mut pb, &pts, 7.0, false);
            if let Some(p) = pb.finish() {
                c.stroke_path_ex(&p, &stroke(4.6), col, BlendMode::SourceOver, ts);
            }
        }
    }

    // Legs.
    for (a, b) in [
        ((30.0, 70.0), (17.5, 75.0)),
        ((33.0, 77.0), (22.0, 85.5)),
        ((39.5, 81.5), (33.0, 91.5)),
    ] {
        line(c, &[a, b], 5.2, SHELL_DARK);
        line(
            c,
            &[(100.0 - a.0, a.1), (100.0 - b.0, b.1)],
            5.2,
            SHELL_DARK,
        );
    }
    // Arms.
    line(
        c,
        &[(29.0, 58.0), (20.0, 50.0), (21.0, 40.0)],
        6.5,
        SHELL_DARK,
    );
    line(
        c,
        &[(71.0, 58.0), (80.0, 50.0), (79.0, 40.0)],
        6.5,
        SHELL_DARK,
    );
    // Claws, raised and open towards each other.
    // Fill plus a round-joined outline in the same colour softens the
    // pincer tips and the notch between them.
    for p in [
        claw(21.0, 29.0, 12.0, -52.0, 62.0),
        claw(79.0, 29.0, 12.0, -128.0, 62.0),
    ]
    .into_iter()
    .flatten()
    {
        c.fill_path_ex(&p, SHELL, FillRule::Winding, ts);
        c.stroke_path_ex(&p, &stroke(3.6), SHELL, BlendMode::SourceOver, ts);
    }
    // Eye stalks.
    line(c, &[(42.5, 48.0), (40.5, 37.0)], 4.6, SHELL_DARK);
    line(c, &[(57.5, 48.0), (59.5, 37.0)], 4.6, SHELL_DARK);
    // Shell.
    if let Some(rect) = Rect::from_ltrb(22.0, 44.0, 78.0, 84.0) {
        let mut pb = PathBuilder::new();
        pb.push_oval(rect);
        if let Some(p) = pb.finish() {
            c.fill_path_ex(&p, SHELL, FillRule::Winding, ts);
        }
    }
    // Shell highlight.
    if let Some(p) = seg(&[(33.0, 55.0), (39.0, 50.5), (46.0, 48.5)]) {
        c.stroke_path_ex(
            &p,
            &stroke(3.2),
            Color::rgba(255, 255, 255, 70),
            BlendMode::SourceOver,
            ts,
        );
    }
    // Eyes.
    for (ex, px) in [(40.2, 41.2), (59.8, 58.8)] {
        if let Some(p) = PathBuilder::from_circle(ex, 34.0, 6.6) {
            c.fill_path_ex(&p, Color::WHITE, FillRule::Winding, ts);
        }
        if let Some(p) = PathBuilder::from_circle(px, 34.4, 3.1) {
            c.fill_path_ex(&p, INK, FillRule::Winding, ts);
        }
    }
    // Smile.
    let mut pb = PathBuilder::new();
    pb.move_to(43.5, 67.0);
    pb.quad_to(50.0, 73.5, 56.5, 67.0);
    if let Some(p) = pb.finish() {
        c.stroke_path_ex(&p, &stroke(3.0), INK, BlendMode::SourceOver, ts);
    }
}

/// Render the mark to straight-alpha RGBA (for tray and window icons).
pub fn mark_rgba(size: u32, badge: bool) -> Vec<u8> {
    let mut buf = vec![0u32; (size * size) as usize];
    let mut c = Canvas::new(&mut buf, size, size);
    draw_mark(
        &mut c,
        RectF::new(0.0, 0.0, size as f32, size as f32),
        badge,
    );
    premul_argb_to_rgba(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mark_renders_with_transparency_and_ink() {
        let px = mark_rgba(64, false);
        assert_eq!(px.len(), 64 * 64 * 4);
        let opaque = px.chunks_exact(4).filter(|p| p[3] == 255).count();
        let clear = px.chunks_exact(4).filter(|p| p[3] == 0).count();
        assert!(opaque > 500, "crab body should be opaque");
        assert!(clear > 500, "corners should stay transparent");
    }
}
