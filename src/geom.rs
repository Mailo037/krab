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
    fn segment_distance() {
        let d = dist_to_segment(Pt::new(5.0, 5.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-4);
        let d = dist_to_segment(Pt::new(-3.0, 4.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-4);
    }
}
