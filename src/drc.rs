//! Exact design-rule check for the copper the router adds.
//!
//! KiCad's DRC is the referee; this checker exists so the router never ships copper it
//! knows to be illegal. It checks every new track and via against all other copper
//! (clearance), holes (hole clearance, hole-to-hole), the board edge and rule-area
//! keepouts, using the exact core+radius distances from `geom`.

use crate::board::Board;
use crate::geom::{BBox, P, Shape};
use crate::rules::Rules;

/// Safety margin added to every clearance (mm). KiCad compares integer nanometres, and we
/// write coordinates rounded to 1 nm, so a few nm of slack avoids exact-boundary flips.
pub const EPS: f64 = 0.0005;
/// Float tolerance for clearance comparisons: a gap equal to the requirement is legal
/// (KiCad flags `actual < required`, in integer nanometres).
pub const TOL: f64 = 1e-7;

#[derive(Debug, Clone)]
pub struct Track {
    pub net: u32,
    pub layer: usize,
    pub a: P,
    pub b: P,
    pub width: f64,
}

impl Track {
    pub fn shape(&self) -> Shape {
        Shape::capsule(self.a, self.b, self.width / 2.0)
    }
}

#[derive(Debug, Clone)]
pub struct NewVia {
    pub net: u32,
    pub pos: P,
    pub diameter: f64,
    pub drill: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Clearance,
    HoleClearance,
    HoleToHole,
    EdgeClearance,
    Keepout,
    OutsideBoard,
}

#[derive(Debug, Clone)]
pub struct Violation {
    pub kind: Kind,
    /// Net of the new item at fault (and the other net, if any).
    pub net: u32,
    pub other: u32,
    pub at: P,
    pub gap: f64,
    pub required: f64,
}

/// Static obstacle set for queries: board copper by layer, holes, edges.
pub struct Obstacles<'a> {
    pub board: &'a Board,
    pub rules: &'a Rules,
    /// (net, clearance of that item, shape) per layer.
    pub copper: Vec<Vec<(u32, f64, Shape)>>,
    /// (net or 0 for NPTH, shape)
    pub holes: Vec<(u32, Shape)>,
}

impl<'a> Obstacles<'a> {
    pub fn new(board: &'a Board, rules: &'a Rules) -> Obstacles<'a> {
        let mut copper = vec![Vec::new(); board.layers.len()];
        for pd in &board.pads {
            let c = item_clearance(board, rules, pd.net, pd.clearance);
            for &l in &pd.layers {
                for s in &pd.shapes {
                    copper[l].push((pd.net, c, s.clone()));
                }
            }
        }
        for f in &board.fixed {
            copper[f.layer].push((f.net, item_clearance(board, rules, f.net, None), f.shape.clone()));
        }
        let mut holes = Vec::new();
        for pd in &board.pads {
            if let Some(h) = &pd.hole {
                holes.push((if pd.plated { pd.net } else { 0 }, h.clone()));
            }
        }
        for v in &board.vias {
            holes.push((v.net, Shape::circle(v.pos, v.drill / 2.0)));
        }
        Obstacles { board, rules, copper, holes }
    }
}

/// Clearance an item of `net` asks for: its net class clearance, a local override, or the
/// board minimum, whichever is largest.
pub fn item_clearance(board: &Board, rules: &Rules, net: u32, local: Option<f64>) -> f64 {
    let class = rules.class(rules.class_of(board.net_name(net)));
    class.clearance.max(local.unwrap_or(0.0)).max(rules.min_clearance)
}

fn poly_hit(shape: &Shape, outline: &[P]) -> bool {
    let poly = Shape::poly(outline.to_vec(), 0.0);
    shape.gap(&poly) < 0.0
}

/// Check new tracks and vias. Returns all violations found.
pub fn check(obs: &Obstacles, tracks: &[Track], vias: &[NewVia]) -> Vec<Violation> {
    let b = obs.board;
    let r = obs.rules;
    let mut out = Vec::new();
    let clear = |net: u32| item_clearance(b, r, net, None);
    let edge_c = r.min_copper_edge_clearance;

    // New items as (net, layer, shape, clearance, is_via_index)
    struct Item {
        net: u32,
        layer: usize,
        shape: Shape,
        bbox: BBox,
    }
    let mut items: Vec<Item> = Vec::new();
    for t in tracks {
        let s = t.shape();
        items.push(Item { net: t.net, layer: t.layer, bbox: s.bbox(), shape: s });
    }
    for v in vias {
        for l in 0..b.layers.len() {
            let s = Shape::circle(v.pos, v.diameter / 2.0);
            items.push(Item { net: v.net, layer: l, bbox: s.bbox(), shape: s });
        }
    }

    for it in &items {
        let c_it = clear(it.net);
        // Against fixed copper.
        for (net, c_o, s) in &obs.copper[it.layer] {
            if *net == it.net && *net != 0 {
                continue;
            }
            let req = c_it.max(*c_o);
            if it.bbox.gap(&s.bbox()) >= req + EPS {
                continue;
            }
            let g = it.shape.gap(s);
            if g < req - TOL {
                out.push(Violation { kind: Kind::Clearance, net: it.net, other: *net, at: first_point(&it.shape), gap: g, required: req });
            }
        }
        // Against holes of other nets.
        for (net, h) in &obs.holes {
            if *net == it.net && *net != 0 {
                continue;
            }
            let req = r.min_hole_clearance;
            if it.bbox.gap(&h.bbox()) >= req + EPS {
                continue;
            }
            let g = it.shape.gap(h);
            if g < req - TOL {
                out.push(Violation { kind: Kind::HoleClearance, net: it.net, other: *net, at: first_point(&it.shape), gap: g, required: req });
            }
        }
        // Board edge.
        let g = b.shape_edge_gap(&it.shape);
        if g < edge_c - TOL {
            out.push(Violation { kind: Kind::EdgeClearance, net: it.net, other: 0, at: first_point(&it.shape), gap: g, required: edge_c });
        }
        if !b.edges.is_empty() && !b.inside(first_point(&it.shape)) {
            out.push(Violation { kind: Kind::OutsideBoard, net: it.net, other: 0, at: first_point(&it.shape), gap: 0.0, required: 0.0 });
        }
    }
    // Keepouts.
    for t in tracks {
        let s = t.shape();
        for k in &b.keepouts {
            if k.no_tracks && k.layers.contains(&t.layer) && poly_hit(&s, &k.outline) {
                out.push(Violation { kind: Kind::Keepout, net: t.net, other: 0, at: t.a, gap: 0.0, required: 0.0 });
            }
        }
    }
    for v in vias {
        let s = Shape::circle(v.pos, v.diameter / 2.0);
        for k in &b.keepouts {
            if k.no_vias && poly_hit(&s, &k.outline) {
                out.push(Violation { kind: Kind::Keepout, net: v.net, other: 0, at: v.pos, gap: 0.0, required: 0.0 });
            }
        }
        // Via hole against every other hole, and against new vias.
        let hole = Shape::circle(v.pos, v.drill / 2.0);
        for (_, h) in &obs.holes {
            let g = hole.gap(h);
            if g < r.min_hole_to_hole - TOL {
                out.push(Violation { kind: Kind::HoleToHole, net: v.net, other: 0, at: v.pos, gap: g, required: r.min_hole_to_hole });
            }
        }
    }
    // New items against each other (different nets), bucketed by a coarse grid.
    let cell = 1.0f64;
    let mut buckets: std::collections::HashMap<(i64, i64, usize), Vec<usize>> = Default::default();
    for (i, it) in items.iter().enumerate() {
        let bb = it.bbox;
        for x in (bb.min.x / cell).floor() as i64..=(bb.max.x / cell).floor() as i64 {
            for y in (bb.min.y / cell).floor() as i64..=(bb.max.y / cell).floor() as i64 {
                buckets.entry((x, y, it.layer)).or_default().push(i);
            }
        }
    }
    let mut seen = std::collections::HashSet::new();
    for list in buckets.values() {
        for (ai, &i) in list.iter().enumerate() {
            for &j in &list[ai + 1..] {
                let (a, bb) = (&items[i], &items[j]);
                if a.net == bb.net {
                    continue;
                }
                let key = (i.min(j), i.max(j));
                if !seen.insert(key) {
                    continue;
                }
                let req = clear(a.net).max(clear(bb.net));
                if a.bbox.gap(&bb.bbox) >= req + EPS {
                    continue;
                }
                let g = a.shape.gap(&bb.shape);
                if g < req - TOL {
                    out.push(Violation { kind: Kind::Clearance, net: a.net, other: bb.net, at: first_point(&a.shape), gap: g, required: req });
                }
            }
        }
    }
    for (i, v) in vias.iter().enumerate() {
        for w in &vias[i + 1..] {
            let g = v.pos.dist(w.pos) - v.drill / 2.0 - w.drill / 2.0;
            if g < r.min_hole_to_hole - TOL {
                out.push(Violation { kind: Kind::HoleToHole, net: v.net, other: w.net, at: v.pos, gap: g, required: r.min_hole_to_hole });
            }
        }
    }
    out
}

fn first_point(s: &Shape) -> P {
    s.core_points()[0]
}
