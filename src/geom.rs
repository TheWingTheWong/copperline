//! Exact 2D geometry for clearance checks.
//!
//! Every copper object is a *core* (a point, a segment or a simple polygon) inflated by
//! a radius. That one representation covers circles (point + r), tracks and oval pads
//! (segment + r), rectangles, trapezoids and chamfered pads (polygon + 0) and rounded
//! rectangles (inner polygon + corner radius) exactly, so the clearance between any two
//! objects is `dist(core_a, core_b) - r_a - r_b` with no sampling error.

use std::ops::{Add, Mul, Sub};

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct P {
    pub x: f64,
    pub y: f64,
}

pub const fn p(x: f64, y: f64) -> P {
    P { x, y }
}

impl Add for P {
    type Output = P;
    fn add(self, o: P) -> P {
        p(self.x + o.x, self.y + o.y)
    }
}
impl Sub for P {
    type Output = P;
    fn sub(self, o: P) -> P {
        p(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f64> for P {
    type Output = P;
    fn mul(self, k: f64) -> P {
        p(self.x * k, self.y * k)
    }
}

impl P {
    pub fn dot(self, o: P) -> f64 {
        self.x * o.x + self.y * o.y
    }
    pub fn cross(self, o: P) -> f64 {
        self.x * o.y - self.y * o.x
    }
    pub fn len(self) -> f64 {
        self.x.hypot(self.y)
    }
    pub fn dist(self, o: P) -> f64 {
        (self - o).len()
    }
    /// Rotate by `deg` degrees the way KiCad does on screen (y axis points down,
    /// positive angles turn counter-clockwise as seen by the user).
    pub fn rot_kicad(self, deg: f64) -> P {
        if deg == 0.0 {
            return self;
        }
        let (s, c) = (-deg).to_radians().sin_cos();
        p(self.x * c - self.y * s, self.x * s + self.y * c)
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BBox {
    pub min: P,
    pub max: P,
}

impl Default for BBox {
    fn default() -> BBox {
        BBox::empty()
    }
}

impl BBox {
    pub fn empty() -> BBox {
        BBox { min: p(f64::INFINITY, f64::INFINITY), max: p(f64::NEG_INFINITY, f64::NEG_INFINITY) }
    }
    pub fn of_points(pts: &[P]) -> BBox {
        let mut b = BBox::empty();
        for &q in pts {
            b.add(q);
        }
        b
    }
    pub fn add(&mut self, q: P) {
        self.min.x = self.min.x.min(q.x);
        self.min.y = self.min.y.min(q.y);
        self.max.x = self.max.x.max(q.x);
        self.max.y = self.max.y.max(q.y);
    }
    pub fn union(&self, o: &BBox) -> BBox {
        BBox {
            min: p(self.min.x.min(o.min.x), self.min.y.min(o.min.y)),
            max: p(self.max.x.max(o.max.x), self.max.y.max(o.max.y)),
        }
    }
    pub fn inflate(&self, r: f64) -> BBox {
        BBox { min: p(self.min.x - r, self.min.y - r), max: p(self.max.x + r, self.max.y + r) }
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }
    pub fn overlaps(&self, o: &BBox) -> bool {
        self.min.x <= o.max.x && o.min.x <= self.max.x && self.min.y <= o.max.y && o.min.y <= self.max.y
    }
    /// Lower bound on the distance between any point of `self` and any point of `o`.
    pub fn gap(&self, o: &BBox) -> f64 {
        let dx = (o.min.x - self.max.x).max(self.min.x - o.max.x).max(0.0);
        let dy = (o.min.y - self.max.y).max(self.min.y - o.max.y).max(0.0);
        dx.hypot(dy)
    }
}

/// The skeleton of a shape; the shape is all points within `radius` of it.
#[derive(Debug, Clone, PartialEq)]
pub enum Core {
    Point(P),
    Seg(P, P),
    /// Simple polygon, either winding, implicitly closed. Treated as filled.
    Poly(Vec<P>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub core: Core,
    pub radius: f64,
}

impl Shape {
    pub fn circle(c: P, r: f64) -> Shape {
        Shape { core: Core::Point(c), radius: r }
    }
    pub fn capsule(a: P, b: P, r: f64) -> Shape {
        if a == b { Shape::circle(a, r) } else { Shape { core: Core::Seg(a, b), radius: r } }
    }
    pub fn poly(pts: Vec<P>, r: f64) -> Shape {
        Shape { core: Core::Poly(pts), radius: r }
    }
    pub fn bbox(&self) -> BBox {
        let b = match &self.core {
            Core::Point(q) => BBox::of_points(&[*q]),
            Core::Seg(a, b) => BBox::of_points(&[*a, *b]),
            Core::Poly(pts) => BBox::of_points(pts),
        };
        b.inflate(self.radius)
    }
    /// Signed gap between the two shapes' boundaries (negative when they overlap).
    pub fn gap(&self, o: &Shape) -> f64 {
        core_dist(&self.core, &o.core) - self.radius - o.radius
    }
    /// Gap from a point to the shape (negative inside).
    pub fn point_gap(&self, q: P) -> f64 {
        core_dist(&Core::Point(q), &self.core) - self.radius
    }
    pub fn translate(&self, d: P) -> Shape {
        let core = match &self.core {
            Core::Point(q) => Core::Point(*q + d),
            Core::Seg(a, b) => Core::Seg(*a + d, *b + d),
            Core::Poly(pts) => Core::Poly(pts.iter().map(|&q| q + d).collect()),
        };
        Shape { core, radius: self.radius }
    }
}

pub fn point_seg_dist(q: P, a: P, b: P) -> f64 {
    let ab = b - a;
    let l2 = ab.dot(ab);
    if l2 == 0.0 {
        return q.dist(a);
    }
    let t = ((q - a).dot(ab) / l2).clamp(0.0, 1.0);
    q.dist(a + ab * t)
}

fn orient(a: P, b: P, c: P) -> f64 {
    (b - a).cross(c - a)
}

fn on_seg(a: P, b: P, q: P) -> bool {
    q.x >= a.x.min(b.x) && q.x <= a.x.max(b.x) && q.y >= a.y.min(b.y) && q.y <= a.y.max(b.y)
}

pub fn segs_intersect(a: P, b: P, c: P, d: P) -> bool {
    let d1 = orient(c, d, a);
    let d2 = orient(c, d, b);
    let d3 = orient(a, b, c);
    let d4 = orient(a, b, d);
    if ((d1 > 0.0 && d2 < 0.0) || (d1 < 0.0 && d2 > 0.0)) && ((d3 > 0.0 && d4 < 0.0) || (d3 < 0.0 && d4 > 0.0)) {
        return true;
    }
    (d1 == 0.0 && on_seg(c, d, a))
        || (d2 == 0.0 && on_seg(c, d, b))
        || (d3 == 0.0 && on_seg(a, b, c))
        || (d4 == 0.0 && on_seg(a, b, d))
}

pub fn seg_seg_dist(a: P, b: P, c: P, d: P) -> f64 {
    if segs_intersect(a, b, c, d) {
        return 0.0;
    }
    point_seg_dist(a, c, d)
        .min(point_seg_dist(b, c, d))
        .min(point_seg_dist(c, a, b))
        .min(point_seg_dist(d, a, b))
}

/// Even-odd point-in-polygon test (boundary counts as inside via the distance checks).
pub fn point_in_poly(q: P, pts: &[P]) -> bool {
    let mut inside = false;
    let n = pts.len();
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (pi, pj) = (pts[i], pts[j]);
        if (pi.y > q.y) != (pj.y > q.y) {
            let x = pj.x + (q.y - pj.y) * (pi.x - pj.x) / (pi.y - pj.y);
            if q.x < x {
                inside = !inside;
            }
        }
        j = i;
    }
    inside
}

fn poly_edges(pts: &[P]) -> impl Iterator<Item = (P, P)> + '_ {
    let n = pts.len();
    (0..n).map(move |i| (pts[i], pts[(i + 1) % n]))
}

fn point_poly_dist(q: P, pts: &[P]) -> f64 {
    if pts.len() >= 3 && point_in_poly(q, pts) {
        return 0.0;
    }
    poly_edges(pts).map(|(a, b)| point_seg_dist(q, a, b)).fold(f64::INFINITY, f64::min)
}

fn seg_poly_dist(a: P, b: P, pts: &[P]) -> f64 {
    if pts.len() >= 3 && (point_in_poly(a, pts) || point_in_poly(b, pts)) {
        return 0.0;
    }
    poly_edges(pts).map(|(c, d)| seg_seg_dist(a, b, c, d)).fold(f64::INFINITY, f64::min)
}

fn poly_poly_dist(x: &[P], y: &[P]) -> f64 {
    if (x.len() >= 3 && y.first().is_some_and(|&q| point_in_poly(q, x)))
        || (y.len() >= 3 && x.first().is_some_and(|&q| point_in_poly(q, y)))
    {
        return 0.0;
    }
    let mut best = f64::INFINITY;
    for (a, b) in poly_edges(x) {
        for (c, d) in poly_edges(y) {
            best = best.min(seg_seg_dist(a, b, c, d));
            if best == 0.0 {
                return 0.0;
            }
        }
    }
    best
}

pub fn core_dist(u: &Core, v: &Core) -> f64 {
    use Core::*;
    match (u, v) {
        (Point(a), Point(b)) => a.dist(*b),
        (Point(q), Seg(a, b)) | (Seg(a, b), Point(q)) => point_seg_dist(*q, *a, *b),
        (Point(q), Poly(ps)) | (Poly(ps), Point(q)) => point_poly_dist(*q, ps),
        (Seg(a, b), Seg(c, d)) => seg_seg_dist(*a, *b, *c, *d),
        (Seg(a, b), Poly(ps)) | (Poly(ps), Seg(a, b)) => seg_poly_dist(*a, *b, ps),
        (Poly(x), Poly(y)) => poly_poly_dist(x, y),
    }
}

/// Approximate an arc through `start`, `mid`, `end` with chords whose sagitta is at most `tol`.
/// Returns the points including both ends.
pub fn arc_points(start: P, mid: P, end: P, tol: f64) -> Vec<P> {
    let Some((c, r)) = circumcircle(start, mid, end) else {
        return vec![start, end];
    };
    let a0 = (start.y - c.y).atan2(start.x - c.x);
    let am = (mid.y - c.y).atan2(mid.x - c.x);
    let a1 = (end.y - c.y).atan2(end.x - c.x);
    // Sweep from a0 to a1 passing through am.
    let norm = |a: f64| a.rem_euclid(std::f64::consts::TAU);
    let ccw_to_end = norm(a1 - a0);
    let ccw_to_mid = norm(am - a0);
    let sweep = if ccw_to_mid <= ccw_to_end { ccw_to_end } else { ccw_to_end - std::f64::consts::TAU };
    let step = 2.0 * (1.0 - tol.min(r) / r).acos();
    let n = ((sweep.abs() / step.max(1e-6)).ceil() as usize).clamp(1, 720);
    (0..=n)
        .map(|i| {
            let a = a0 + sweep * i as f64 / n as f64;
            p(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect()
}

pub fn circumcircle(a: P, b: P, c: P) -> Option<(P, f64)> {
    let d = 2.0 * (a.x * (b.y - c.y) + b.x * (c.y - a.y) + c.x * (a.y - b.y));
    if d.abs() < 1e-12 {
        return None;
    }
    let (a2, b2, c2) = (a.dot(a), b.dot(b), c.dot(c));
    let ux = (a2 * (b.y - c.y) + b2 * (c.y - a.y) + c2 * (a.y - b.y)) / d;
    let uy = (a2 * (c.x - b.x) + b2 * (a.x - c.x) + c2 * (b.x - a.x)) / d;
    let center = p(ux, uy);
    Some((center, center.dist(a)))
}

/// Regular polygon that *contains* the circle of radius `r` around `c` (circumscribed),
/// with the given number of sides. Used where a polygonal outline must be conservative.
pub fn circle_poly_outer(c: P, r: f64, sides: usize) -> Vec<P> {
    let rr = r / (std::f64::consts::PI / sides as f64).cos();
    (0..sides)
        .map(|i| {
            let a = std::f64::consts::TAU * i as f64 / sides as f64;
            p(c.x + rr * a.cos(), c.y + rr * a.sin())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    /// Dense sampling of a shape's boundary-inclusive region for brute-force distances.
    fn sample(shape: &Shape, rng: &mut Rng, n: usize) -> Vec<P> {
        let bb = shape.bbox();
        let mut out = Vec::new();
        while out.len() < n {
            let q = p(rng.range(bb.min.x, bb.max.x), rng.range(bb.min.y, bb.max.y));
            if shape.point_gap(q) <= 0.0 {
                out.push(q);
            }
        }
        out
    }

    fn random_shape(rng: &mut Rng) -> Shape {
        let c = p(rng.range(-3.0, 3.0), rng.range(-3.0, 3.0));
        let r = if rng.chance(0.3) { 0.0 } else { rng.range(0.05, 0.8) };
        match rng.below(3) {
            0 => Shape::circle(c, r.max(0.05)),
            1 => Shape::capsule(c, c + p(rng.range(-2.0, 2.0), rng.range(-2.0, 2.0)), r.max(0.05)),
            _ => {
                // Random convex-ish quad: a rotated rectangle.
                let (w, h, a) = (rng.range(0.1, 2.0), rng.range(0.1, 2.0), rng.range(0.0, 360.0));
                let pts = [p(-w, -h), p(w, -h), p(w, h), p(-w, h)].iter().map(|&q| q.rot_kicad(a) + c).collect();
                Shape::poly(pts, r)
            }
        }
    }

    #[test]
    fn gap_matches_brute_force_sampling() {
        let mut rng = Rng::new(7);
        for _ in 0..300 {
            let (a, b) = (random_shape(&mut rng), random_shape(&mut rng));
            let exact = a.gap(&b);
            let sa = sample(&a, &mut rng, 400);
            // Sampled points are inside A, so the brute-force gap from them to B is an upper bound.
            let brute = sa.iter().map(|&q| b.point_gap(q)).fold(f64::INFINITY, f64::min);
            if exact > 0.0 {
                assert!(brute >= exact - 1e-9, "sampled point closer than exact gap: {brute} < {exact}");
                assert!(brute - exact < 0.25, "exact gap far below sampled gap: {exact} vs {brute}");
            } else {
                // Overlapping shapes: exact gap negative or zero, so some sample should be near/inside B.
                assert!(brute <= 0.2, "shapes overlap but no sample near B: {brute}");
            }
        }
    }

    #[test]
    fn segment_distances() {
        assert_eq!(seg_seg_dist(p(0., 0.), p(2., 0.), p(1., -1.), p(1., 1.)), 0.0);
        assert!((seg_seg_dist(p(0., 0.), p(2., 0.), p(0., 1.), p(2., 1.)) - 1.0).abs() < 1e-12);
        assert!((point_seg_dist(p(3., 4.), p(0., 0.), p(0., 0.)) - 5.0).abs() < 1e-12);
    }

    #[test]
    fn rounded_rect_gap_is_exact() {
        // 2x1 roundrect with corner radius 0.25: inner rect 1.5x0.5 inflated by 0.25.
        let rr = Shape::poly(vec![p(-0.75, -0.25), p(0.75, -0.25), p(0.75, 0.25), p(-0.75, 0.25)], 0.25);
        assert!((rr.point_gap(p(2.0, 0.0)) - 1.0).abs() < 1e-12);
        let corner = p(0.75, 0.25) + p(1.0, 1.0) * (1.0 / 2f64.sqrt());
        assert!((rr.point_gap(corner) - 0.75).abs() < 1e-12);
    }

    #[test]
    fn kicad_rotation_is_counter_clockwise_on_screen() {
        // y points down; +90 deg turns +x into -y (up on screen).
        let r = p(1.0, 0.0).rot_kicad(90.0);
        assert!((r.x).abs() < 1e-12 && (r.y + 1.0).abs() < 1e-12);
    }

    #[test]
    fn arc_points_stay_on_circle_and_respect_tolerance() {
        let pts = arc_points(p(1., 0.), p(0., 1.), p(-1., 0.), 0.001);
        assert!(pts.len() > 10);
        for q in &pts {
            assert!((q.len() - 1.0).abs() < 1e-9);
        }
        assert!(pts.iter().all(|q| q.y >= -1e-9), "arc went the wrong way round");
    }
}
