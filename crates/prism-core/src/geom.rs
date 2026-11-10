//! Small geometry types. Image space is integer pixels of the captured virtual desktop, with
//! (0, 0) at its top-left corner; shapes use `f32` points in the same space.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Default, Serialize, Deserialize)]
pub struct Pt {
    pub x: f32,
    pub y: f32,
}

impl Pt {
    pub const fn new(x: f32, y: f32) -> Pt {
        Pt { x, y }
    }
    pub fn dist(self, o: Pt) -> f32 {
        ((self.x - o.x).powi(2) + (self.y - o.y).powi(2)).sqrt()
    }
    pub fn add(self, dx: f32, dy: f32) -> Pt {
        Pt::new(self.x + dx, self.y + dy)
    }
}

/// Distance from `p` to the segment `a`–`b`.
pub fn dist_to_segment(p: Pt, a: Pt, b: Pt) -> f32 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return p.dist(a);
    }
    let t = (((p.x - a.x) * dx + (p.y - a.y) * dy) / len2).clamp(0.0, 1.0);
    p.dist(Pt::new(a.x + t * dx, a.y + t * dy))
}

/// Integer rectangle, `x`/`y` inclusive, `w`/`h` never negative once normalised.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct IRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl IRect {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> IRect {
        IRect { x, y, w, h }
    }
    /// The rectangle spanned by two corner points, in either order. Both corners are included,
    /// so dragging from (10, 10) to (10, 10) gives a 1×1 rectangle.
    pub fn spanning(ax: i32, ay: i32, bx: i32, by: i32) -> IRect {
        let (x0, x1) = (ax.min(bx), ax.max(bx));
        let (y0, y1) = (ay.min(by), ay.max(by));
        IRect::new(x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }
    pub fn from_ltrb(l: i32, t: i32, r: i32, b: i32) -> IRect {
        IRect::new(l, t, (r - l).max(0), (b - t).max(0))
    }
    pub const fn right(&self) -> i32 {
        self.x + self.w
    }
    pub const fn bottom(&self) -> i32 {
        self.y + self.h
    }
    pub const fn is_empty(&self) -> bool {
        self.w <= 0 || self.h <= 0
    }
    pub const fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.right() && y < self.bottom()
    }
    pub fn intersect(&self, o: &IRect) -> IRect {
        IRect::from_ltrb(self.x.max(o.x), self.y.max(o.y), self.right().min(o.right()), self.bottom().min(o.bottom()))
    }
    pub fn union(&self, o: &IRect) -> IRect {
        if self.is_empty() {
            return *o;
        }
        if o.is_empty() {
            return *self;
        }
        IRect::from_ltrb(self.x.min(o.x), self.y.min(o.y), self.right().max(o.right()), self.bottom().max(o.bottom()))
    }
    pub fn inflate(&self, d: i32) -> IRect {
        IRect::new(self.x - d, self.y - d, self.w + 2 * d, self.h + 2 * d)
    }
    pub fn offset(&self, dx: i32, dy: i32) -> IRect {
        IRect::new(self.x + dx, self.y + dy, self.w, self.h)
    }
    /// Moved by the smallest amount that puts it inside `bounds`. A rectangle larger than
    /// `bounds` keeps its size and is aligned to the top-left.
    pub fn clamp_inside(&self, bounds: &IRect) -> IRect {
        let x = self.x.min(bounds.right() - self.w).max(bounds.x);
        let y = self.y.min(bounds.bottom() - self.h).max(bounds.y);
        IRect::new(x, y, self.w, self.h)
    }
}

/// Floating-point bounding box used for hit testing and dirty regions of shapes.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct FRect {
    pub l: f32,
    pub t: f32,
    pub r: f32,
    pub b: f32,
}

impl FRect {
    pub fn spanning(a: Pt, b: Pt) -> FRect {
        FRect { l: a.x.min(b.x), t: a.y.min(b.y), r: a.x.max(b.x), b: a.y.max(b.y) }
    }
    pub fn of_points(pts: &[Pt]) -> FRect {
        let mut r = FRect { l: f32::MAX, t: f32::MAX, r: f32::MIN, b: f32::MIN };
        for p in pts {
            r.l = r.l.min(p.x);
            r.t = r.t.min(p.y);
            r.r = r.r.max(p.x);
            r.b = r.b.max(p.y);
        }
        if pts.is_empty() { FRect::default() } else { r }
    }
    pub fn inflate(&self, d: f32) -> FRect {
        FRect { l: self.l - d, t: self.t - d, r: self.r + d, b: self.b + d }
    }
    pub fn offset(&self, dx: f32, dy: f32) -> FRect {
        FRect { l: self.l + dx, t: self.t + dy, r: self.r + dx, b: self.b + dy }
    }
    /// The smallest rectangle holding both.
    pub fn union(&self, o: &FRect) -> FRect {
        FRect { l: self.l.min(o.l), t: self.t.min(o.t), r: self.r.max(o.r), b: self.b.max(o.b) }
    }
    pub fn contains(&self, p: Pt) -> bool {
        p.x >= self.l && p.x <= self.r && p.y >= self.t && p.y <= self.b
    }
    pub fn w(&self) -> f32 {
        self.r - self.l
    }
    pub fn h(&self) -> f32 {
        self.b - self.t
    }
    /// The smallest integer rectangle covering this one.
    pub fn to_irect(&self) -> IRect {
        IRect::from_ltrb(self.l.floor() as i32, self.t.floor() as i32, self.r.ceil() as i32 + 1, self.b.ceil() as i32 + 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanning_is_inclusive_and_order_free() {
        assert_eq!(IRect::spanning(10, 10, 10, 10), IRect::new(10, 10, 1, 1));
        assert_eq!(IRect::spanning(20, 5, 10, 15), IRect::new(10, 5, 11, 11));
    }

    #[test]
    fn intersect_and_union() {
        let a = IRect::new(0, 0, 10, 10);
        let b = IRect::new(5, 5, 10, 10);
        assert_eq!(a.intersect(&b), IRect::new(5, 5, 5, 5));
        assert_eq!(a.union(&b), IRect::new(0, 0, 15, 15));
        assert!(a.intersect(&IRect::new(20, 20, 2, 2)).is_empty());
        assert_eq!(IRect::default().union(&b), b);
    }

    #[test]
    fn clamp_inside_moves_minimally() {
        let bounds = IRect::new(0, 0, 100, 100);
        assert_eq!(IRect::new(95, -3, 10, 10).clamp_inside(&bounds), IRect::new(90, 0, 10, 10));
        assert_eq!(IRect::new(20, 20, 10, 10).clamp_inside(&bounds), IRect::new(20, 20, 10, 10));
    }

    #[test]
    fn segment_distance() {
        let d = dist_to_segment(Pt::new(5.0, 3.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0));
        assert!((d - 3.0).abs() < 1e-5);
        let d = dist_to_segment(Pt::new(-4.0, 3.0), Pt::new(0.0, 0.0), Pt::new(10.0, 0.0));
        assert!((d - 5.0).abs() < 1e-5);
    }
}
