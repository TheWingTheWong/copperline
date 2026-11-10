//! Tiny anti-aliased rasteriser for routing animations: every shape is drawn from its exact
//! signed distance, so a frame needs nothing but the geometry kernel. Output is binary PPM.

use crate::board::Board;
use crate::drc::{NewVia, Track};
use crate::geom::{Core, P, Shape, p, point_in_poly, point_seg_dist};

pub struct Image {
    pub w: usize,
    pub h: usize,
    pub px: Vec<[f32; 3]>,
    x0: f64,
    y0: f64,
    scale: f64,
}

fn rgb(hex: u32) -> [f32; 3] {
    [((hex >> 16) & 255) as f32 / 255.0, ((hex >> 8) & 255) as f32 / 255.0, (hex & 255) as f32 / 255.0]
}

impl Image {
    pub fn for_board(b: &Board, width: usize) -> Image {
        let bb = b.bbox.inflate(1.0);
        let scale = width as f64 / (bb.max.x - bb.min.x);
        let h = ((bb.max.y - bb.min.y) * scale).ceil() as usize;
        let h = h + h % 2; // even sizes keep video encoders happy
        Image { w: width, h, px: vec![rgb(0x10151c); width * h], x0: bb.min.x, y0: bb.min.y, scale }
    }

    pub fn fill(&mut self, s: &Shape, color: u32, alpha: f32) {
        let c = rgb(color);
        let bb = s.bbox();
        let px = 1.0 / self.scale;
        let xa = (((bb.min.x - self.x0) * self.scale).floor().max(0.0)) as usize;
        let ya = (((bb.min.y - self.y0) * self.scale).floor().max(0.0)) as usize;
        let xb = ((((bb.max.x - self.x0) * self.scale).ceil()) as usize).min(self.w.saturating_sub(1));
        let yb = ((((bb.max.y - self.y0) * self.scale).ceil()) as usize).min(self.h.saturating_sub(1));
        for y in ya..=yb {
            for x in xa..=xb {
                let q = p(self.x0 + (x as f64 + 0.5) * px, self.y0 + (y as f64 + 0.5) * px);
                let d = signed_gap(s, q) / px;
                let cov = (0.5 - d).clamp(0.0, 1.0) as f32 * alpha;
                if cov > 0.0 {
                    let o = &mut self.px[y * self.w + x];
                    for k in 0..3 {
                        o[k] = o[k] * (1.0 - cov) + c[k] * cov;
                    }
                }
            }
        }
    }

    pub fn ppm(&self) -> Vec<u8> {
        let mut out = format!("P6\n{} {}\n255\n", self.w, self.h).into_bytes();
        for c in &self.px {
            for k in 0..3 {
                out.push((c[k].clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        out
    }
}

/// Signed distance to the shape's boundary (negative inside), so polygon interiors get
/// full coverage and their edges are anti-aliased.
pub fn signed_gap(s: &Shape, q: P) -> f64 {
    if let Core::Poly(pts) = &s.core {
        if pts.len() >= 3 && point_in_poly(q, pts) {
            let d = (0..pts.len()).map(|i| point_seg_dist(q, pts[i], pts[(i + 1) % pts.len()])).fold(f64::INFINITY, f64::min);
            return -d - s.radius;
        }
    }
    s.point_gap(q)
}

const LAYER: [u32; 4] = [0xd9483b, 0x3b7dd8, 0xe0a030, 0x40a070];

/// One frame: outline, routed copper (bottom layer first), pads, vias; `hot` tracks
/// (nets still in conflict) are drawn in white.
pub fn frame(b: &Board, tracks: &[Track], vias: &[NewVia], hot: &[u32], width: usize) -> Image {
    let mut img = Image::for_board(b, width);
    let line = 0.6 / img.scale.max(1e-9) * 2.0;
    for &(a, e) in &b.edges {
        img.fill(&Shape::capsule(a, e, line.max(0.05)), 0xc8c26a, 1.0);
    }
    let nl = b.layers.len();
    for l in (0..nl).rev() {
        let col = if l == 0 { LAYER[0] } else if l + 1 == nl { LAYER[1] } else { LAYER[2 + l % 2] };
        for f in b.fixed.iter().filter(|f| f.layer == l) {
            img.fill(&f.shape, col, 0.9);
        }
        for t in tracks.iter().filter(|t| t.layer == l) {
            let c = if hot.contains(&t.net) { 0xf4f4f4 } else { col };
            img.fill(&t.shape(), c, 0.9);
        }
    }
    for pd in &b.pads {
        let c = if pd.layers.contains(&0) { 0xe8b04a } else { 0x7aa7e0 };
        for s in &pd.shapes {
            img.fill(s, c, 1.0);
        }
        if let Some(h) = &pd.hole {
            img.fill(h, 0x10151c, 1.0);
        }
    }
    for v in &b.vias {
        img.fill(&Shape::circle(v.pos, v.diameter / 2.0), 0xcfcfcf, 1.0);
        img.fill(&Shape::circle(v.pos, v.drill / 2.0), 0x10151c, 1.0);
    }
    for v in vias {
        img.fill(&Shape::circle(v.pos, v.diameter / 2.0), 0xcfcfcf, 1.0);
        img.fill(&Shape::circle(v.pos, v.drill / 2.0), 0x10151c, 1.0);
    }
    img
}
