//! Steadying freehand strokes.
//!
//! A light mouse or a trackpad wobbles, and a stroke made of every point the pointer passes
//! keeps every wobble. Here each point of the stroke is an average of the pointer's path around
//! it, a few pixels either way, weighted towards the middle (a Gaussian along the path). Wobbles
//! much shorter than that average out, while turns and loops the hand meant are much longer
//! and keep their shape. The average looks both ways, so nothing lags behind or cuts corners.
//!
//! The stroke's two ends stay exactly where the pointer was: past each end, the path is taken
//! to carry on as the mirror image of its last stretch, which also keeps a straight end straight.
//! Ahead of the pointer there's nothing to average yet, so the newest few pixels of a stroke
//! settle as it goes on; everything further back is final and never changes again.

use crate::geom::Pt;

/// Freehand points closer together than this add nothing but drawing work. With smoothing,
/// the pointer's path is measured out in steps of this.
pub const MIN_STEP: f32 = 1.5;

pub struct Steady {
    /// Sample weights for 0, 1, 2… steps away; empty without smoothing.
    kernel: Vec<f32>,
    /// The stroke's points before this one, left as they are (Shift drew them straight).
    base: usize,
    /// The pointer's path since `base`, one point every `MIN_STEP` along it.
    path: Vec<Pt>,
    /// How far the pointer went from the last of `path` to `last`.
    run: f32,
    /// Where the pointer is.
    last: Pt,
    /// How many stroke points after `base` are final.
    settled: usize,
}

impl Steady {
    /// A stroke starting at `start`, averaged over `sigma` pixels either way. 0 follows the
    /// pointer exactly.
    pub fn new(start: Pt, sigma: f32) -> Steady {
        let kernel = if sigma > 0.0 {
            let reach = (3.0 * sigma / MIN_STEP).ceil() as usize;
            (0..=reach).map(|i| (-(i as f32 * MIN_STEP / sigma).powi(2) / 2.0).exp()).collect()
        } else {
            Vec::new()
        };
        Steady { kernel, base: 0, path: vec![start], run: 0.0, last: start, settled: 1 }
    }

    /// Stroke points, counting from the first, that will never change again.
    pub fn settled(&self) -> usize {
        self.base + self.settled
    }

    /// The pointer moved to `p`: bring the stroke `pts` up to date.
    pub fn follow(&mut self, pts: &mut Vec<Pt>, p: Pt) {
        if self.kernel.is_empty() {
            if pts.last().is_none_or(|l| l.dist(p) >= MIN_STEP) {
                pts.push(p);
            }
            self.settled = pts.len() - self.base;
            return;
        }
        self.walk(p);
        // The samples to average: the path, then the pointer itself unless it's on the last.
        let n = self.path.len();
        let with_tip = self.path[n - 1].dist(p) > 0.01;
        let len = n + with_tip as usize;
        let at = |k: usize| if k < n { self.path[k] } else { p };
        let reach = (self.kernel.len() - 1).min(len - 1);
        pts.truncate(self.base + self.settled);
        // The ends average to themselves; the last is put in as it is, so rounding can't move it.
        for i in self.settled..len - 1 {
            let (mut x, mut y, mut sum) = (0.0, 0.0, 0.0);
            for j in -(reach as isize)..=reach as isize {
                let k = i as isize + j;
                // Past an end, the mirror image of the path through that end.
                let q = if k < 0 {
                    mirror(at(0), at(-k as usize))
                } else if k as usize >= len {
                    mirror(at(len - 1), at(2 * (len - 1) - k as usize))
                } else {
                    at(k as usize)
                };
                let w = self.kernel[j.unsigned_abs()];
                (x, y, sum) = (x + q.x * w, y + q.y * w, sum + w);
            }
            pts.push(Pt::new(x / sum, y / sum));
        }
        if len > self.settled {
            pts.push(at(len - 1));
        }
        // A point is final once everything it averages is on the path and short of its end.
        self.settled = self.settled.max(n.saturating_sub(self.kernel.len()));
    }

    /// Measure the pointer's path out to `p`, adding a point every `MIN_STEP`.
    fn walk(&mut self, p: Pt) {
        let d = self.last.dist(p);
        let mut along = MIN_STEP - self.run;
        while along <= d {
            let t = along / d;
            self.path.push(Pt::new(self.last.x + (p.x - self.last.x) * t, self.last.y + (p.y - self.last.y) * t));
            along += MIN_STEP;
        }
        self.run = d - (along - MIN_STEP);
        self.last = p;
    }

    /// The stroke was changed from outside, as Shift does when it makes it straight: keep it
    /// as it is and carry on from its last point.
    pub fn restart(&mut self, pts: &[Pt]) {
        let Some(&end) = pts.last() else { return };
        self.base = pts.len() - 1;
        self.path = vec![end];
        self.run = 0.0;
        self.last = end;
        self.settled = 1;
    }
}

/// `p` mirrored through `c`.
fn mirror(c: Pt, p: Pt) -> Pt {
    Pt::new(2.0 * c.x - p.x, 2.0 * c.y - p.y)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(sigma: f32, path: &[Pt]) -> (Vec<Pt>, Steady) {
        let mut pts = vec![path[0]];
        let mut s = Steady::new(path[0], sigma);
        for p in &path[1..] {
            s.follow(&mut pts, *p);
        }
        (pts, s)
    }

    /// Along a line, zigzagging `amp` pixels either side of it every 2 pixels.
    fn zigzag(amp: f32) -> Vec<Pt> {
        (0..=200).map(|i| Pt::new(i as f32 * 2.0, if i % 2 == 0 { amp } else { -amp })).collect()
    }

    #[test]
    fn without_smoothing_the_stroke_follows_the_pointer() {
        let path = zigzag(3.0);
        assert_eq!(draw(0.0, &path).0, path);
        // Points closer than a step are dropped.
        let (pts, _) = draw(0.0, &[Pt::new(0.0, 0.0), Pt::new(1.0, 0.0), Pt::new(2.0, 0.0)]);
        assert_eq!(pts, [Pt::new(0.0, 0.0), Pt::new(2.0, 0.0)]);
    }

    #[test]
    fn wobbles_are_ironed_out() {
        let (pts, _) = draw(4.0, &zigzag(3.0));
        // The ends stay on the pointer, wobble and all; a little way in, the stroke stays close
        // to the line the hand meant.
        let inside = pts.iter().filter(|p| (8.0..392.0).contains(&p.x));
        let worst = inside.map(|p| p.y.abs()).fold(0.0, f32::max);
        assert!(worst < 0.5, "wobbles up to {worst} px got through");
    }

    #[test]
    fn the_stroke_runs_from_where_it_started_to_the_pointer() {
        let path = zigzag(3.0);
        let (pts, _) = draw(4.0, &path);
        assert_eq!((pts[0], pts.last()), (path[0], path.last()));
        // A pointer barely moved still draws.
        let (pts, _) = draw(4.0, &[Pt::new(0.0, 0.0), Pt::new(1.0, 0.0)]);
        assert_eq!(pts, [Pt::new(0.0, 0.0), Pt::new(1.0, 0.0)]);
    }

    #[test]
    fn straight_lines_stay_straight_and_keep_their_length() {
        let path: Vec<Pt> = (0..=40).map(|i| Pt::new(i as f32 * 3.0, i as f32 * 1.5)).collect();
        let (pts, _) = draw(6.0, &path);
        for p in &pts {
            assert!((p.y - p.x / 2.0).abs() < 1e-3, "{p:?} is off the line");
        }
        assert_eq!(pts.last(), path.last());
    }

    #[test]
    fn settled_points_never_change() {
        let mut path = zigzag(3.0);
        path.extend((0..100).map(|i| Pt::new(400.0 - i as f32, 3.0 * i as f32)));
        let mut pts = vec![path[0]];
        let mut s = Steady::new(path[0], 5.0);
        let mut seen: Vec<Pt> = Vec::new();
        for p in &path[1..] {
            s.follow(&mut pts, *p);
            let settled = s.settled();
            assert!(settled >= seen.len() && settled <= pts.len());
            assert_eq!(pts[..seen.len()], seen[..]);
            seen = pts[..settled].to_vec();
        }
        assert!(seen.len() > pts.len() - 20, "only {} of {} points settled", seen.len(), pts.len());
    }

    #[test]
    fn restart_keeps_what_was_drawn_and_carries_on() {
        let mut pts = vec![Pt::new(0.0, 0.0)];
        let mut s = Steady::new(pts[0], 4.0);
        s.follow(&mut pts, Pt::new(4.0, 3.0));
        // Shift made it a straight stroke to (50, 0).
        pts.truncate(1);
        pts.push(Pt::new(50.0, 0.0));
        s.restart(&pts);
        assert_eq!(s.settled(), 2);
        s.follow(&mut pts, Pt::new(80.0, 0.0));
        assert_eq!(pts[..2], [Pt::new(0.0, 0.0), Pt::new(50.0, 0.0)]);
        assert_eq!(pts.last(), Some(&Pt::new(80.0, 0.0)));
        assert!(pts[2..].windows(2).all(|w| w[1].x > w[0].x && w[1].y.abs() < 1e-3));
    }
}
