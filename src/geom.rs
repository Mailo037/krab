//! Minimal 2D geometry helpers used across the app.

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    pub fn dist(self, o: Pt) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn lerp(self, o: Pt, t: f32) -> Pt {
        Pt::new(self.x + (o.x - self.x) * t, self.y + (o.y - self.y) * t)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct RectF {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl RectF {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }
    /// Normalized rectangle spanning two arbitrary corner points.
    pub fn from_points(a: Pt, b: Pt) -> Self {
        let x = a.x.min(b.x);
        let y = a.y.min(b.y);
        Self::new(x, y, (a.x - b.x).abs(), (a.y - b.y).abs())
    }
    pub fn right(&self) -> f32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }
    pub fn center(&self) -> Pt {
        Pt::new(self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.x && p.y >= self.y && p.x < self.right() && p.y < self.bottom()
    }
    pub fn inflate(&self, d: f32) -> Self {
        Self::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }
    pub fn intersect(&self, o: &RectF) -> RectF {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        RectF::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
    }
    pub fn union(&self, o: &RectF) -> RectF {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        let r = self.right().max(o.right());
        let b = self.bottom().max(o.bottom());
        RectF::new(x, y, r - x, b - y)
    }
    /// Snap to whole pixels.
    pub fn round(&self) -> RectI {
        let x = self.x.round() as i32;
        let y = self.y.round() as i32;
        let r = self.right().round() as i32;
        let b = self.bottom().round() as i32;
        RectI::new(x, y, (r - x).max(0), (b - y).max(0))
    }
    /// The smallest pixel rect that covers this one completely.
    pub fn round_out(&self) -> RectI {
        let x = self.x.floor() as i32;
        let y = self.y.floor() as i32;
        let r = self.right().ceil() as i32;
        let b = self.bottom().ceil() as i32;
        RectI::new(x, y, (r - x).max(0), (b - y).max(0))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }
    pub fn right(&self) -> i32 {
        self.x + self.w
    }
    pub fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn intersect(&self, o: &RectI) -> RectI {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        RectI::new(x, y, (r - x).max(0), (b - y).max(0))
    }
    pub fn to_f(&self) -> RectF {
        RectF::new(self.x as f32, self.y as f32, self.w as f32, self.h as f32)
    }
    pub fn translate(&self, dx: i32, dy: i32) -> RectI {
        RectI::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    pub fn union(&self, o: &RectI) -> RectI {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        let r = self.right().max(o.right());
        let b = self.bottom().max(o.bottom());
        RectI::new(x, y, r - x, b - y)
    }
    pub fn inflate(&self, d: i32) -> RectI {
        RectI::new(self.x - d, self.y - d, self.w + 2 * d, self.h + 2 * d)
    }
    pub fn area(&self) -> i64 {
        if self.is_empty() {
            0
        } else {
            self.w as i64 * self.h as i64
        }
    }
    /// The parts of `self` not covered by `o`, as up to four rects.
    pub fn subtract(&self, o: &RectI) -> Vec<RectI> {
        let i = self.intersect(o);
        if i.is_empty() {
            return if self.is_empty() {
                Vec::new()
            } else {
                vec![*self]
            };
        }
        let mut out = Vec::with_capacity(4);
        // Full-width bands above and below, then the sides of the middle band.
        let parts = [
            RectI::new(self.x, self.y, self.w, i.y - self.y),
            RectI::new(self.x, i.bottom(), self.w, self.bottom() - i.bottom()),
            RectI::new(self.x, i.y, i.x - self.x, i.h),
            RectI::new(i.right(), i.y, self.right() - i.right(), i.h),
        ];
        out.extend(parts.into_iter().filter(|r| !r.is_empty()));
        out
    }
}

/// Coalesce damage rects into at most `max` rects covering all of them,
/// clipped to `bounds`. Rects are first snapped to a coarse grid, so the
/// result over-covers a little but stays cheap to compute and to repaint.
pub fn merge_rects(rects: &[RectI], bounds: RectI, max: usize) -> Vec<RectI> {
    const TILE: i32 = 32;
    let rects: Vec<RectI> = rects
        .iter()
        .map(|r| r.intersect(&bounds))
        .filter(|r| !r.is_empty())
        .collect();
    if rects.is_empty() || bounds.is_empty() {
        return Vec::new();
    }
    let cols = ((bounds.w + TILE - 1) / TILE) as usize;
    let rows = ((bounds.h + TILE - 1) / TILE) as usize;
    let mut grid = vec![false; cols * rows];
    for r in &rects {
        let c0 = ((r.x - bounds.x) / TILE) as usize;
        let c1 = ((r.right() - bounds.x - 1) / TILE) as usize;
        let r0 = ((r.y - bounds.y) / TILE) as usize;
        let r1 = ((r.bottom() - bounds.y - 1) / TILE) as usize;
        for row in r0..=r1.min(rows - 1) {
            grid[row * cols + c0..=row * cols + c1.min(cols - 1)].fill(true);
        }
    }
    // Horizontal runs per tile row, grown downwards while the next row has
    // the same run.
    let mut open: Vec<(usize, usize, usize, usize)> = Vec::new(); // c0, c1, r0, r1
    let mut out: Vec<RectI> = Vec::new();
    let tile_rect = |c0: usize, c1: usize, r0: usize, r1: usize| {
        RectI::new(
            bounds.x + c0 as i32 * TILE,
            bounds.y + r0 as i32 * TILE,
            (c1 - c0 + 1) as i32 * TILE,
            (r1 - r0 + 1) as i32 * TILE,
        )
        .intersect(&bounds)
    };
    for row in 0..=rows {
        let mut runs = Vec::new();
        if row < rows {
            let line = &grid[row * cols..(row + 1) * cols];
            let mut c = 0;
            while c < cols {
                if line[c] {
                    let start = c;
                    while c < cols && line[c] {
                        c += 1;
                    }
                    runs.push((start, c - 1));
                } else {
                    c += 1;
                }
            }
        }
        let mut next_open = Vec::new();
        for o in open.drain(..) {
            if let Some(pos) = runs.iter().position(|r| (r.0, r.1) == (o.0, o.1)) {
                runs.remove(pos);
                next_open.push((o.0, o.1, o.2, row));
            } else {
                out.push(tile_rect(o.0, o.1, o.2, o.3));
            }
        }
        for (c0, c1) in runs {
            next_open.push((c0, c1, row, row));
        }
        open = next_open;
    }
    // Greedily join the pair that wastes the least area until few are left.
    while out.len() > max.max(1) {
        let mut best = (0, 1, i64::MAX);
        for i in 0..out.len() {
            for j in i + 1..out.len() {
                let u = out[i].union(&out[j]);
                let waste = u.area() - out[i].area() - out[j].area();
                if waste < best.2 {
                    best = (i, j, waste);
                }
            }
        }
        let (i, j, _) = best;
        let u = out[i].union(&out[j]);
        out.swap_remove(j);
        out[i] = u;
    }
    out
}

/// Distance from point `p` to the segment `a`-`b`.
pub fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 < 1e-6 {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(Pt::new(a.x + t * dx, a.y + t * dy))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_points_normalizes() {
        let r = RectF::from_points(Pt::new(10.0, 20.0), Pt::new(4.0, 2.0));
        assert_eq!(r, RectF::new(4.0, 2.0, 6.0, 18.0));
    }

    #[test]
    fn intersect_clamps_to_empty() {
        let a = RectI::new(0, 0, 10, 10);
        let b = RectI::new(20, 20, 5, 5);
        assert!(a.intersect(&b).is_empty());
        assert_eq!(
            a.intersect(&RectI::new(5, 5, 10, 10)),
            RectI::new(5, 5, 5, 5)
        );
    }

    #[test]
    fn subtract_leaves_exactly_the_uncovered_part() {
        let a = RectI::new(0, 0, 100, 80);
        let b = RectI::new(30, 20, 200, 30);
        let parts = a.subtract(&b);
        let area: i64 = parts.iter().map(|r| r.area()).sum();
        assert_eq!(area, a.area() - a.intersect(&b).area());
        assert!(parts.iter().all(|p| p.intersect(&b).is_empty()));
        assert_eq!(a.subtract(&a), Vec::<RectI>::new());
        assert_eq!(a.subtract(&RectI::new(500, 500, 5, 5)), vec![a]);
    }

    #[test]
    fn merged_damage_covers_every_input_and_respects_the_limit() {
        let bounds = RectI::new(0, 0, 1920, 1080);
        let rects: Vec<RectI> = (0..60)
            .map(|i| RectI::new((i * 97) % 1800, (i * 53) % 1000, 20 + i % 40, 10 + i % 30))
            .collect();
        let merged = merge_rects(&rects, bounds, 8);
        assert!(merged.len() <= 8);
        for r in &rects {
            for (x, y) in [(r.x, r.y), (r.right() - 1, r.bottom() - 1)] {
                assert!(merged.iter().any(|m| m.contains(x, y)), "{r:?} not covered");
            }
        }
        assert!(merged.iter().all(|m| m.intersect(&bounds) == *m));
        assert!(merge_rects(&[], bounds, 8).is_empty());
    }

    #[test]
    fn segment_distance() {
        let d = dist_to_segment(Pt::new(5.0, 5.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-4);
        let d = dist_to_segment(Pt::new(-3.0, 4.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-4);
    }
}
