//! Grid router with negotiated congestion (PathFinder).
//!
//! The board is rasterised into one grid per copper layer. For every net class a static
//! mask records, per cell, which nets may put a track centerline there: the cell is free,
//! owned by exactly one net (it is inside that net's clearance halo, so only that net may
//! use it), or blocked. Masks are built from exact distances to the real copper shapes,
//! inflated by the class's half track width, the larger clearance and a grid margin.
//!
//! Nets are routed as approximate Steiner trees by repeated multi-source A*. Overlap
//! between different nets is allowed but priced: every routed net stamps a halo into an
//! occupancy grid, and the cost of a cell grows with its present occupancy and with a
//! history term that accumulates across iterations, until no two nets overlap
//! (McMurchie and Ebeling, 1995). Pads are reached through short exact "stubs" from the
//! pad anchor to a nearby grid node, so pads that sit off the grid or between
//! fine-pitch neighbours stay reachable. The result is checked with the exact DRC; nets
//! that still violate are rerouted with hard blocking or left unrouted.

use std::collections::{BinaryHeap, HashSet};
use std::cmp::Ordering;

use crate::board::Board;
use crate::drc::{self, NewVia, Obstacles, Track};
use crate::geom::{BBox, P, Shape, p};
use crate::rules::Rules;

const FREE: u32 = u32::MAX;
const MULTI: u32 = u32::MAX - 1;
const SQRT2: f32 = std::f32::consts::SQRT_2;

#[derive(Debug, Clone)]
pub struct Params {
    /// Grid pitch in mm; chosen from the design rules when `None`.
    pub pitch: Option<f64>,
    pub max_iters: usize,
    /// Cost of a via, in grid steps.
    pub via_cost: f32,
    pub pres_fac: f32,
    pub pres_growth: f32,
    pub hist_fac: f32,
    /// Extra cost factor for moves against a layer's preferred direction.
    pub dir_penalty: f32,
    pub turn_cost: f32,
    /// Cap on grid cells per layer (the pitch grows to stay under it).
    pub max_cells: usize,
    /// Record per-iteration snapshots (for animation).
    pub snapshots: bool,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            pitch: None,
            max_iters: 60,
            via_cost: 30.0,
            pres_fac: 0.6,
            pres_growth: 1.6,
            hist_fac: 0.4,
            dir_penalty: 0.15,
            turn_cost: 0.4,
            max_cells: 3_000_000,
            snapshots: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Class {
    w: f64,
    cl: f64,
    vd: f64,
    vdrill: f64,
}

#[derive(Debug, Clone, Copy)]
pub struct Grid {
    pub o: P,
    pub g: f64,
    pub nx: usize,
    pub ny: usize,
    pub nl: usize,
}

impl Grid {
    pub fn cells(&self) -> usize {
        self.nx * self.ny
    }
    pub fn center(&self, x: usize, y: usize) -> P {
        p(self.o.x + (x as f64 + 0.5) * self.g, self.o.y + (y as f64 + 0.5) * self.g)
    }
    fn xy(&self, c: usize) -> (usize, usize) {
        (c % self.nx, c / self.nx)
    }
    pub fn cell_center(&self, c: usize) -> P {
        let (x, y) = self.xy(c);
        self.center(x, y)
    }
    /// Cell index range (inclusive) covering a box, clamped to the grid.
    fn range(&self, b: &BBox) -> Option<(usize, usize, usize, usize)> {
        let f = |v: f64, o: f64, n: usize| -> (i64, usize) { (((v - o) / self.g - 0.5).floor() as i64, n) };
        let (x0, _) = f(b.min.x, self.o.x, self.nx);
        let (y0, _) = f(b.min.y, self.o.y, self.ny);
        let x1 = ((b.max.x - self.o.x) / self.g - 0.5).ceil() as i64;
        let y1 = ((b.max.y - self.o.y) / self.g - 0.5).ceil() as i64;
        let (x0, y0) = (x0.max(0), y0.max(0));
        let (x1, y1) = (x1.min(self.nx as i64 - 1), y1.min(self.ny as i64 - 1));
        if x0 > x1 || y0 > y1 {
            return None;
        }
        Some((x0 as usize, x1 as usize, y0 as usize, y1 as usize))
    }
    fn nearest(&self, q: P) -> Option<usize> {
        let x = ((q.x - self.o.x) / self.g - 0.5).round();
        let y = ((q.y - self.o.y) / self.g - 0.5).round();
        if x < 0.0 || y < 0.0 || x >= self.nx as f64 || y >= self.ny as f64 {
            return None;
        }
        Some(y as usize * self.nx + x as usize)
    }
}

fn own(mask: &mut [u32], i: usize, net: u32) {
    let o = mask[i];
    mask[i] = if net == 0 || o == MULTI {
        MULTI
    } else if o == FREE || o == net {
        net
    } else {
        MULTI
    };
}

#[inline]
fn passable(owner: u32, net: u32) -> bool {
    owner == FREE || owner == net
}

/// Exact clearance queries against the fixed board items (bucketed).
struct StaticIndex<'a> {
    obs: &'a Obstacles<'a>,
    bucket: f64,
    o: P,
    bx: usize,
    by: usize,
    copper: Vec<Vec<Vec<u32>>>,
    holes: Vec<Vec<u32>>,
    edges: Vec<Vec<u32>>,
}

impl<'a> StaticIndex<'a> {
    fn new(obs: &'a Obstacles<'a>, bb: BBox) -> StaticIndex<'a> {
        let bucket = 1.0;
        let o = bb.min;
        let bx = (((bb.max.x - bb.min.x) / bucket).ceil() as usize).max(1);
        let by = (((bb.max.y - bb.min.y) / bucket).ceil() as usize).max(1);
        let nl = obs.copper.len();
        let mut s = StaticIndex { obs, bucket, o, bx, by, copper: vec![vec![Vec::new(); bx * by]; nl], holes: vec![Vec::new(); bx * by], edges: vec![Vec::new(); bx * by] };
        for l in 0..nl {
            for (i, (_, _, sh)) in obs.copper[l].iter().enumerate() {
                for b in s.buckets(&sh.bbox().inflate(1.5)) {
                    s.copper[l][b].push(i as u32);
                }
            }
        }
        for (i, (_, h)) in obs.holes.iter().enumerate() {
            for b in s.buckets(&h.bbox().inflate(1.5)) {
                s.holes[b].push(i as u32);
            }
        }
        for (i, &(a, e)) in obs.board.edges.iter().enumerate() {
            for b in s.buckets(&BBox::of_points(&[a, e]).inflate(1.5)) {
                s.edges[b].push(i as u32);
            }
        }
        s
    }
    fn buckets(&self, b: &BBox) -> Vec<usize> {
        let x0 = (((b.min.x - self.o.x) / self.bucket).floor().max(0.0) as usize).min(self.bx - 1);
        let x1 = (((b.max.x - self.o.x) / self.bucket).floor().max(0.0) as usize).min(self.bx - 1);
        let y0 = (((b.min.y - self.o.y) / self.bucket).floor().max(0.0) as usize).min(self.by - 1);
        let y1 = (((b.max.y - self.o.y) / self.bucket).floor().max(0.0) as usize).min(self.by - 1);
        let mut v = Vec::new();
        for y in y0..=y1 {
            for x in x0..=x1 {
                v.push(y * self.bx + x);
            }
        }
        v
    }
    /// True when a track segment of `net` (width `w`, clearance `cl`) is legal against
    /// every fixed item, the holes and the board edge.
    fn seg_clear(&self, net: u32, layer: usize, a: P, b: P, w: f64, cl: f64) -> bool {
        let s = Shape::capsule(a, b, w / 2.0);
        let bks = self.buckets(&BBox::of_points(&[a, b]));
        let mut ids: Vec<u32> = bks.iter().flat_map(|&k| self.copper[layer][k].iter().copied()).collect();
        ids.sort_unstable();
        ids.dedup();
        let mut hids: Vec<u32> = bks.iter().flat_map(|&k| self.holes[k].iter().copied()).collect();
        hids.sort_unstable();
        hids.dedup();
        let mut eids: Vec<u32> = bks.iter().flat_map(|&k| self.edges[k].iter().copied()).collect();
        eids.sort_unstable();
        eids.dedup();
        let r = self.obs.rules;
        for &i in &ids {
            let (n, c_o, sh) = &self.obs.copper[layer][i as usize];
            if *n == net && net != 0 {
                continue;
            }
            if s.gap(sh) < cl.max(*c_o) + drc::EPS {
                return false;
            }
        }
        for &i in &hids {
            let (n, h) = &self.obs.holes[i as usize];
            if *n == net && net != 0 {
                continue;
            }
            if s.gap(h) < r.min_hole_clearance + drc::EPS {
                return false;
            }
        }
        for &i in &eids {
            let (ea, eb) = self.obs.board.edges[i as usize];
            if s.gap(&Shape::capsule(ea, eb, 0.0)) < r.min_copper_edge_clearance + drc::EPS {
                return false;
            }
        }
        for k in &self.obs.board.keepouts {
            if k.no_tracks && k.layers.contains(&layer) && s.gap(&Shape::poly(k.outline.clone(), 0.0)) < 0.0 {
                return false;
            }
        }
        true
    }
}

/// A way onto a pad: a grid node plus the exact stub from the pad anchor to it.
#[derive(Debug, Clone)]
struct Access {
    state: usize,
    /// Pad anchor the stub starts from; `None` when the node is inside the pad copper.
    anchor: Option<P>,
    cost: f32,
    /// Cells under the stub (for congestion).
    cells: Vec<u32>,
}

#[derive(Debug, Clone, Default)]
struct Route {
    /// Paths as state sequences; a path starts on the tree (or a pad access) and ends at a
    /// pad access.
    paths: Vec<Vec<usize>>,
    /// Stubs used: (layer, pad anchor, access state); no anchor for in-pad nodes.
    stubs: Vec<(usize, Option<P>, usize)>,
    stamped_t: Vec<u32>,
    stamped_v: Vec<u32>,
    connected: usize,
}

#[derive(Copy, Clone, PartialEq)]
struct HeapItem {
    f: f32,
    s: u32,
}
impl Eq for HeapItem {}
impl Ord for HeapItem {
    fn cmp(&self, o: &Self) -> Ordering {
        o.f.partial_cmp(&self.f).unwrap_or(Ordering::Equal).then_with(|| o.s.cmp(&self.s))
    }
}
impl PartialOrd for HeapItem {
    fn partial_cmp(&self, o: &Self) -> Option<Ordering> {
        Some(self.cmp(o))
    }
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub pitch: f64,
    pub cells: usize,
    pub nets: usize,
    pub nets_complete: usize,
    pub connections: usize,
    pub connections_routed: usize,
    pub iterations: usize,
    pub conflicts_per_iter: Vec<usize>,
    pub drc_dropped: usize,
    pub tracks: usize,
    pub vias: usize,
    pub length_mm: f64,
    pub ms: u128,
}

/// One frame of routing progress: every net's current tracks and vias.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub iteration: usize,
    pub conflicts: usize,
    pub tracks: Vec<Track>,
    pub vias: Vec<NewVia>,
}

pub struct Output {
    pub tracks: Vec<Track>,
    pub vias: Vec<NewVia>,
    pub stats: Stats,
    pub snapshots: Vec<Snapshot>,
}

struct Net {
    id: u32,
    pads: Vec<usize>,
    class: usize,
    /// Accesses per pad (same order as `pads`).
    access: Vec<Vec<Access>>,
    hpwl: f64,
}

pub struct Router<'a> {
    board: &'a Board,
    rules: &'a Rules,
    prm: Params,
    grid: Grid,
    classes: Vec<Class>,
    /// Track masks per class: nl * cells.
    tmask: Vec<Vec<u32>>,
    /// Via masks per class: cells.
    vmask: Vec<Vec<u32>>,
    occ_t: Vec<u16>,
    occ_v: Vec<u16>,
    hist_t: Vec<f32>,
    hist_v: Vec<f32>,
    nets: Vec<Net>,
    routes: Vec<Route>,
    // search scratch
    dist: Vec<f32>,
    parent: Vec<u32>,
    target: Vec<u32>,
    touched: Vec<u32>,
    w_max: f64,
    rv_max: f64,
    cl_max: f64,
}

fn class_of(board: &Board, rules: &Rules, net: u32) -> Class {
    let c = rules.class(rules.class_of(board.net_name(net)));
    let w = c.track_width.max(rules.min_track_width);
    let cl = drc::item_clearance(board, rules, net, None);
    let vd = c.via_diameter.max(rules.min_via_diameter);
    let vdrill = c.via_drill.max(rules.min_through_hole);
    Class { w, cl, vd, vdrill }
}

impl<'a> Router<'a> {
    pub fn new(board: &'a Board, rules: &'a Rules, prm: Params) -> Router<'a> {
        let obs_bb = if board.edges.is_empty() {
            board.pads.iter().fold(BBox::empty(), |b, pd| b.union(&pd.bbox())).inflate(3.0)
        } else {
            board.bbox
        };
        // Classes actually used by routable nets.
        let mut classes: Vec<Class> = Vec::new();
        let mut net_class = Vec::new();
        for (net, pads) in board.net_pads() {
            let c = class_of(board, rules, net);
            let ci = classes.iter().position(|k| k.w == c.w && k.cl == c.cl && k.vd == c.vd && k.vdrill == c.vdrill).unwrap_or_else(|| {
                classes.push(c);
                classes.len() - 1
            });
            net_class.push((net, pads, ci));
        }
        let min_wc = classes.iter().map(|c| c.w + c.cl).fold(f64::INFINITY, f64::min);
        let area = (obs_bb.max.x - obs_bb.min.x) * (obs_bb.max.y - obs_bb.min.y);
        let mut g = prm.pitch.unwrap_or_else(|| (min_wc / 4.0).clamp(0.04, 0.1));
        g = g.max((area / prm.max_cells as f64).sqrt());
        let nx = ((obs_bb.max.x - obs_bb.min.x) / g).ceil() as usize + 1;
        let ny = ((obs_bb.max.y - obs_bb.min.y) / g).ceil() as usize + 1;
        let nl = board.layers.len();
        let grid = Grid { o: obs_bb.min, g, nx, ny, nl };
        let n = grid.cells();
        let w_max = classes.iter().map(|c| c.w).fold(0.0, f64::max);
        let rv_max = classes.iter().map(|c| c.vd / 2.0).fold(0.0, f64::max);
        let cl_max = classes.iter().map(|c| c.cl).fold(0.0, f64::max);
        let mut r = Router {
            board,
            rules,
            prm,
            grid,
            classes: classes.clone(),
            tmask: Vec::new(),
            vmask: Vec::new(),
            occ_t: vec![0; nl * n],
            occ_v: vec![0; nl * n],
            hist_t: vec![0.0; nl * n],
            hist_v: vec![0.0; n],
            nets: Vec::new(),
            routes: Vec::new(),
            dist: vec![f32::INFINITY; nl * n],
            parent: vec![u32::MAX; nl * n],
            target: vec![0; nl * n],
            touched: Vec::new(),
            w_max,
            rv_max,
            cl_max,
        };
        r.build_masks();
        let obs = Obstacles::new(board, rules);
        let idx = StaticIndex::new(&obs, obs_bb.inflate(2.0));
        for (net, pads, ci) in net_class {
            let access: Vec<Vec<Access>> = pads.iter().map(|&pi| r.accesses(&idx, net, pi, ci)).collect();
            if std::env::var_os("COPPERLINE_DEBUG").is_some() {
                for (k, a) in access.iter().enumerate() {
                    let pd = &board.pads[pads[k]];
                    if a.len() < 3 {
                        let ci = r.nets.len().min(0) + net_class_dbg(&r, net);
                        let mut hist = std::collections::BTreeMap::<String, usize>::new();
                        if let Some((x0, x1, y0, y1)) = r.grid.range(&pd.bbox()) {
                            for y in y0..=y1 { for x in x0..=x1 {
                                let q = r.grid.center(x, y);
                                if pd.shapes.iter().any(|sh| sh.point_gap(q) <= 0.0) {
                                    let o = r.tmask[ci][pd.layers[0] * r.grid.cells() + y * r.grid.nx + x];
                                    let key = if o == FREE { "free".into() } else if o == MULTI { "multi".into() } else { format!("net{o}") };
                                    *hist.entry(key).or_default() += 1;
                                }
                            }}
                        }
                        eprintln!("debug: owners inside pad {:?}", hist);
                        eprintln!("debug: pad {}.{} net {} has {} accesses", pd.footprint, pd.number, board.net_name(net), a.len());
                    }
                }
            }
            let bb = pads.iter().fold(BBox::empty(), |b, &pi| b.union(&BBox::of_points(&[board.pads[pi].pos])));
            let hpwl = (bb.max.x - bb.min.x) + (bb.max.y - bb.min.y);
            r.nets.push(Net { id: net, pads, class: ci, access, hpwl });
        }
        r.routes = vec![Route::default(); r.nets.len()];
        r
    }

    pub fn grid(&self) -> Grid {
        self.grid
    }

    fn margin(&self) -> f64 {
        // Any point on a straight or diagonal move is within g/sqrt(2) of a cell centre.
        self.grid.g * 0.7072
    }

    fn stamp(&self, mask: &mut [u32], base: usize, sh: &Shape, r: f64, net: u32) {
        let gr = &self.grid;
        let Some((x0, x1, y0, y1)) = gr.range(&sh.bbox().inflate(r)) else { return };
        for y in y0..=y1 {
            for x in x0..=x1 {
                let i = base + y * gr.nx + x;
                if mask[i] == MULTI {
                    continue;
                }
                if sh.point_gap(gr.center(x, y)) < r {
                    own(mask, i, net);
                }
            }
        }
    }

    fn build_masks(&mut self) {
        let b = self.board;
        let r = self.rules;
        let gr = self.grid;
        let n = gr.cells();
        let m = self.margin();
        let obs = Obstacles::new(b, r);
        // Inside-board raster (even-odd scanline).
        let mut outside = vec![b.edges.is_empty() == false; n];
        if !b.edges.is_empty() {
            for y in 0..gr.ny {
                let qy = gr.center(0, y).y;
                let mut xs: Vec<f64> = Vec::new();
                for &(a, e) in &b.edges {
                    if (a.y > qy) != (e.y > qy) {
                        xs.push(a.x + (qy - a.y) * (e.x - a.x) / (e.y - a.y));
                    }
                }
                xs.sort_by(|u, v| u.partial_cmp(v).unwrap());
                for pair in xs.chunks(2) {
                    if pair.len() < 2 {
                        break;
                    }
                    for x in 0..gr.nx {
                        let qx = gr.center(x, y).x;
                        if qx > pair[0] && qx < pair[1] {
                            outside[y * gr.nx + x] = false;
                        }
                    }
                }
            }
        }
        let classes = self.classes.clone();
        let mut tmasks = Vec::new();
        let mut vmasks = Vec::new();
        for c in &classes {
            let mut tm = vec![FREE; gr.nl * n];
            let mut vm = vec![FREE; n];
            for l in 0..gr.nl {
                for i in 0..n {
                    if outside[i] {
                        tm[l * n + i] = MULTI;
                    }
                }
            }
            for i in 0..n {
                if outside[i] {
                    vm[i] = MULTI;
                }
            }
            let rv = c.vd / 2.0;
            for l in 0..gr.nl {
                for (net, c_o, sh) in &obs.copper[l] {
                    let req = c.cl.max(*c_o);
                    self.stamp(&mut tm, l * n, sh, c.w / 2.0 + req + m, *net);
                    self.stamp(&mut vm, 0, sh, rv + req + m, *net);
                }
            }
            for (net, h) in &obs.holes {
                for l in 0..gr.nl {
                    self.stamp(&mut tm, l * n, h, c.w / 2.0 + r.min_hole_clearance + m, *net);
                }
                self.stamp(&mut vm, 0, h, rv + r.min_hole_clearance + m, *net);
                // Hole to hole applies between any two holes.
                self.stamp(&mut vm, 0, h, c.vdrill / 2.0 + r.min_hole_to_hole + m, 0);
            }
            for &(a, e) in &b.edges {
                let sh = Shape::capsule(a, e, 0.0);
                for l in 0..gr.nl {
                    self.stamp(&mut tm, l * n, &sh, c.w / 2.0 + r.min_copper_edge_clearance + m, 0);
                }
                self.stamp(&mut vm, 0, &sh, rv + r.min_copper_edge_clearance + m, 0);
            }
            for k in &b.keepouts {
                let sh = Shape::poly(k.outline.clone(), 0.0);
                if k.no_tracks {
                    for &l in &k.layers {
                        self.stamp(&mut tm, l * n, &sh, c.w / 2.0 + m, 0);
                    }
                }
                if k.no_vias {
                    self.stamp(&mut vm, 0, &sh, rv + m, 0);
                }
            }
            tmasks.push(tm);
            vmasks.push(vm);
        }
        self.tmask = tmasks;
        self.vmask = vmasks;
    }

    /// Grid nodes from which a pad can be reached by a short exact stub.
    fn accesses(&self, idx: &StaticIndex, net: u32, pi: usize, ci: usize) -> Vec<Access> {
        let pd = &self.board.pads[pi];
        let c = self.classes[ci];
        let gr = &self.grid;
        let n = gr.cells();
        let mut out = Vec::new();
        let anchor = pd.pos;
        for &l in &pd.layers {
            let reach = 0.8 + c.w;
            let bb = pd.bbox().inflate(reach);
            let Some((x0, x1, y0, y1)) = gr.range(&bb) else { continue };
            let mut cand = Vec::new();
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let s = l * n + y * gr.nx + x;
                    if !passable(self.tmask[ci][s], net) {
                        continue;
                    }
                    let q = gr.center(x, y);
                    // Zero-radius polygons report a gap of exactly 0 inside, so test `<= 0`.
                    let inside = pd.shapes.iter().any(|sh| sh.point_gap(q) <= -1e-9 || (sh.radius == 0.0 && sh.point_gap(q) <= 0.0));
                    let d = q.dist(anchor);
                    let gap = pd.gap_to(&Shape::circle(q, 0.0));
                    if !inside && gap > reach {
                        continue;
                    }
                    cand.push((inside, d, gap, s, q));
                }
            }
            // Cells inside the pad first, then the closest to the pad edge.
            cand.sort_by(|a, b| b.0.cmp(&a.0).then(a.2.partial_cmp(&b.2).unwrap()).then(a.1.partial_cmp(&b.1).unwrap()));
            let mut taken = 0;
            for (inside, d, _, s, q) in cand {
                if taken >= 24 {
                    break;
                }
                if inside {
                    out.push(Access { state: s, anchor: None, cost: 0.0, cells: vec![s as u32] });
                    continue;
                }
                if !idx.seg_clear(net, l, anchor, q, c.w, c.cl) {
                    continue;
                }
                let cells = self.raster_seg(l, anchor, q);
                out.push(Access { state: s, anchor: Some(anchor), cost: (d / gr.g) as f32, cells });
                if !inside {
                    taken += 1;
                }
            }
        }
        out
    }

    fn raster_seg(&self, l: usize, a: P, b: P) -> Vec<u32> {
        let gr = &self.grid;
        let steps = ((a.dist(b) / (gr.g * 0.5)).ceil() as usize).max(1);
        let mut v: Vec<u32> = (0..=steps)
            .filter_map(|i| gr.nearest(a + (b - a) * (i as f64 / steps as f64)))
            .map(|c| (l * gr.cells() + c) as u32)
            .collect();
        v.dedup();
        v
    }

    fn net_order(&self) -> Vec<usize> {
        let mut o: Vec<usize> = (0..self.nets.len()).collect();
        o.sort_by(|&a, &b| self.nets[a].hpwl.partial_cmp(&self.nets[b].hpwl).unwrap().then(a.cmp(&b)));
        o
    }

    /// Track/via geometry of one route as (layer, a, b) segments and via positions.
    fn geometry(&self, ni: usize) -> (Vec<(usize, P, P)>, Vec<P>) {
        let gr = &self.grid;
        let n = gr.cells();
        let rt = &self.routes[ni];
        let mut segs = Vec::new();
        let mut vias = Vec::new();
        let mut breaks: HashSet<usize> = HashSet::new();
        for path in &rt.paths {
            if let (Some(&f), Some(&l)) = (path.first(), path.last()) {
                breaks.insert(f);
                breaks.insert(l);
            }
        }
        for path in &rt.paths {
            let mut i = 0;
            while i + 1 < path.len() {
                let (s0, s1) = (path[i], path[i + 1]);
                if s0 / n != s1 / n {
                    vias.push(gr.cell_center(s0 % n));
                    i += 1;
                    continue;
                }
                // Extend a straight run.
                let d = |a: usize, b: usize| {
                    let (ax, ay) = gr.xy(a % n);
                    let (bx, by) = gr.xy(b % n);
                    (bx as i64 - ax as i64, by as i64 - ay as i64)
                };
                let dir = d(s0, s1);
                let mut j = i + 1;
                while j + 1 < path.len() && path[j] / n == path[j + 1] / n && d(path[j], path[j + 1]) == dir && !breaks.contains(&path[j]) {
                    j += 1;
                }
                segs.push((s0 / n, gr.cell_center(s0 % n), gr.cell_center(path[j] % n)));
                i = j;
            }
        }
        for &(l, anchor, s) in &rt.stubs {
            let q = gr.cell_center(s % n);
            let Some(anchor) = anchor else { continue };
            if q.dist(anchor) > 1e-9 {
                segs.push((l, anchor, q));
            }
        }
        vias.sort_by(|a, b| (a.x, a.y).partial_cmp(&(b.x, b.y)).unwrap());
        vias.dedup_by(|a, b| a.dist(*b) < 1e-9);
        (segs, vias)
    }

    fn halo_cells(&self, ni: usize) -> (Vec<u32>, Vec<u32>) {
        let c = self.classes[self.nets[ni].class];
        let gr = &self.grid;
        let n = gr.cells();
        let m = self.margin();
        let (segs, vias) = self.geometry(ni);
        let mut t = Vec::new();
        let mut v = Vec::new();
        let push = |out: &mut Vec<u32>, l: usize, sh: &Shape, r: f64| {
            let Some((x0, x1, y0, y1)) = gr.range(&sh.bbox().inflate(r)) else { return };
            for y in y0..=y1 {
                for x in x0..=x1 {
                    if sh.point_gap(gr.center(x, y)) < r {
                        out.push((l * n + y * gr.nx + x) as u32);
                    }
                }
            }
        };
        let rt = c.w / 2.0 + self.w_max / 2.0 + c.cl.max(self.cl_max) + m;
        let rv = c.w / 2.0 + self.rv_max + c.cl.max(self.cl_max) + m;
        for &(l, a, b) in &segs {
            let sh = Shape::capsule(a, b, 0.0);
            push(&mut t, l, &sh, rt);
            push(&mut v, l, &sh, rv);
        }
        let h2h = c.vdrill / 2.0 + self.rv_max + self.rules.min_hole_to_hole + m;
        for &q in &vias {
            let sh = Shape::circle(q, 0.0);
            for l in 0..gr.nl {
                push(&mut t, l, &sh, c.vd / 2.0 + self.w_max / 2.0 + c.cl.max(self.cl_max) + m);
                push(&mut v, l, &sh, (c.vd / 2.0 + self.rv_max + c.cl.max(self.cl_max) + m).max(h2h));
            }
        }
        t.sort_unstable();
        t.dedup();
        v.sort_unstable();
        v.dedup();
        (t, v)
    }

    fn stamp_route(&mut self, ni: usize) {
        let (t, v) = self.halo_cells(ni);
        for &c in &t {
            self.occ_t[c as usize] += 1;
        }
        for &c in &v {
            self.occ_v[c as usize] += 1;
        }
        self.routes[ni].stamped_t = t;
        self.routes[ni].stamped_v = v;
    }

    fn rip_up(&mut self, ni: usize) {
        let rt = std::mem::take(&mut self.routes[ni]);
        for &c in &rt.stamped_t {
            self.occ_t[c as usize] -= 1;
        }
        for &c in &rt.stamped_v {
            self.occ_v[c as usize] -= 1;
        }
    }

    /// Route one net (its previous route must be ripped up). `strict` forbids any cell
    /// another net occupies.
    fn route_net(&mut self, ni: usize, pres: f32, strict: bool) {
        let net = &self.nets[ni];
        let id = net.id;
        let ci = net.class;
        let npads = net.pads.len();
        let access = net.access.clone();
        let mut route = Route::default();
        let mut in_tree = vec![false; npads];
        // Start from the pad with the most accesses (most likely reachable).
        let Some(start) = (0..npads).filter(|&k| !access[k].is_empty()).max_by_key(|&k| (access[k].len().min(4), std::cmp::Reverse(k))) else {
            self.routes[ni] = route;
            return;
        };
        in_tree[start] = true;
        route.connected = 1;
        let mut tree: Vec<usize> = Vec::new();
        loop {
            let remaining: Vec<usize> = (0..npads).filter(|&k| !in_tree[k] && !access[k].is_empty()).collect();
            if remaining.is_empty() {
                break;
            }
            // Sources: tree nodes (cost 0) and accesses of connected pads.
            let mut sources: Vec<(usize, f32, Option<(usize, usize)>)> = tree.iter().map(|&s| (s, 0.0, None)).collect();
            for k in 0..npads {
                if in_tree[k] {
                    for (ai, a) in access[k].iter().enumerate() {
                        sources.push((a.state, a.cost + self.stub_cong(a, pres, strict), Some((k, ai))));
                    }
                }
            }
            let mut tb = BBox::empty();
            for &k in &remaining {
                for (ai, a) in access[k].iter().enumerate() {
                    let t = &mut self.target[a.state];
                    if *t == 0 {
                        *t = ((k << 16) + ai + 1) as u32;
                    }
                    tb.add(self.grid.cell_center(a.state % self.grid.cells()));
                }
            }
            let found = self.astar(id, ci, &sources, tb, pres, strict, &access);
            for &k in &remaining {
                for a in &access[k] {
                    self.target[a.state] = 0;
                }
            }
            let Some((path, src_stub, (k, ai))) = found else {
                // Mark unreachable pads so we stop trying them.
                for &k in &remaining {
                    in_tree[k] = true;
                }
                break;
            };
            if let Some((sk, sai)) = src_stub {
                let a = &access[sk][sai];
                let l = a.state / self.grid.cells();
                if !route.stubs.iter().any(|st| st.2 == a.state) {
                    route.stubs.push((l, a.anchor, a.state));
                }
            }
            let a = &access[k][ai];
            route.stubs.push((a.state / self.grid.cells(), a.anchor, a.state));
            in_tree[k] = true;
            route.connected += 1;
            tree.extend(path.iter().copied());
            route.paths.push(path);
        }
        self.routes[ni] = route;
    }

    fn stub_cong(&self, a: &Access, pres: f32, strict: bool) -> f32 {
        let mut worst = 0u16;
        for &c in &a.cells {
            worst = worst.max(self.occ_t[c as usize]);
        }
        if strict && worst > 0 {
            return 1e6;
        }
        a.cost * pres * worst as f32 + 5.0 * pres * worst as f32
    }

    #[allow(clippy::too_many_arguments)]
    fn astar(
        &mut self,
        net: u32,
        ci: usize,
        sources: &[(usize, f32, Option<(usize, usize)>)],
        tb: BBox,
        pres: f32,
        strict: bool,
        access: &[Vec<Access>],
    ) -> Option<(Vec<usize>, Option<(usize, usize)>, (usize, usize))> {
        let gr = self.grid;
        let n = gr.cells();
        let nx = gr.nx as i64;
        let ny = gr.ny as i64;
        let tmask = &self.tmask[ci];
        let vmask = &self.vmask[ci];
        let h = |s: usize| -> f32 {
            let q = gr.cell_center(s % n);
            let dx = (tb.min.x - q.x).max(q.x - tb.max.x).max(0.0);
            let dy = (tb.min.y - q.y).max(q.y - tb.max.y).max(0.0);
            let (a, b) = (dx.max(dy), dx.min(dy));
            ((a - b + b * std::f64::consts::SQRT_2) / gr.g) as f32
        };
        let mut heap = BinaryHeap::new();
        let mut src_of: std::collections::HashMap<usize, Option<(usize, usize)>> = Default::default();
        for &(s, c, st) in sources {
            if c < self.dist[s] {
                if self.dist[s].is_infinite() {
                    self.touched.push(s as u32);
                }
                self.dist[s] = c;
                self.parent[s] = u32::MAX;
                src_of.insert(s, st);
                heap.push(HeapItem { f: c + h(s), s: s as u32 });
            }
        }
        let dirs: [(i64, i64, f32); 8] = [(1, 0, 1.0), (-1, 0, 1.0), (0, 1, 1.0), (0, -1, 1.0), (1, 1, SQRT2), (1, -1, SQRT2), (-1, 1, SQRT2), (-1, -1, SQRT2)];
        let mut goal = None;
        let mut pops = 0usize;
        let pop_cap = 40 * gr.nl * n;
        while let Some(HeapItem { f: _, s }) = heap.pop() {
            let s = s as usize;
            let ds = self.dist[s];
            pops += 1;
            if pops > pop_cap {
                break;
            }
            if self.target[s] != 0 {
                goal = Some(s);
                break;
            }
            let l = s / n;
            let c = s % n;
            let (x, y) = (c as i64 % nx, c as i64 / nx);
            let pdir = {
                let ps = self.parent[s];
                if ps == u32::MAX || ps as usize / n != l {
                    (0, 0)
                } else {
                    let pc = ps as usize % n;
                    (x - pc as i64 % nx, y - pc as i64 / nx)
                }
            };
            for &(dx, dy, step) in &dirs {
                let (x2, y2) = (x + dx, y + dy);
                if x2 < 0 || y2 < 0 || x2 >= nx || y2 >= ny {
                    continue;
                }
                let c2 = (y2 * nx + x2) as usize;
                let s2 = l * n + c2;
                if !passable(tmask[s2], net) {
                    continue;
                }
                let occ = self.occ_t[s2];
                if strict && occ > 0 {
                    continue;
                }
                let horiz = dy == 0;
                let vert = dx == 0;
                let against = gr.nl > 1 && ((l % 2 == 0 && vert) || (l % 2 == 1 && horiz));
                let mut base = step * if against { 1.0 + self.prm.dir_penalty } else { 1.0 };
                if pdir != (0, 0) && pdir != (dx, dy) {
                    base += self.prm.turn_cost;
                }
                let cost = (base + self.hist_t[s2] * step) * (1.0 + pres * occ as f32);
                let nd = ds + cost;
                if nd < self.dist[s2] {
                    if self.dist[s2].is_infinite() {
                        self.touched.push(s2 as u32);
                    }
                    self.dist[s2] = nd;
                    self.parent[s2] = s as u32;
                    heap.push(HeapItem { f: nd + h(s2), s: s2 as u32 });
                }
            }
            // Through via to every other layer.
            if gr.nl > 1 && passable(vmask[c], net) {
                let mut occ = 0u16;
                for l2 in 0..gr.nl {
                    occ = occ.max(self.occ_v[l2 * n + c]);
                }
                if strict && occ > 0 {
                    continue;
                }
                let cost = (self.prm.via_cost + self.hist_v[c]) * (1.0 + pres * occ as f32);
                for l2 in 0..gr.nl {
                    if l2 == l {
                        continue;
                    }
                    let s2 = l2 * n + c;
                    if !passable(tmask[s2], net) {
                        continue;
                    }
                    let nd = ds + cost;
                    if nd < self.dist[s2] {
                        if self.dist[s2].is_infinite() {
                            self.touched.push(s2 as u32);
                        }
                        self.dist[s2] = nd;
                        self.parent[s2] = s as u32;
                        heap.push(HeapItem { f: nd + h(s2), s: s2 as u32 });
                    }
                }
            }
        }
        let res = goal.map(|g| {
            let mut path = vec![g];
            let mut cur = g;
            while self.parent[cur] != u32::MAX {
                cur = self.parent[cur] as usize;
                path.push(cur);
            }
            path.reverse();
            let src = src_of.get(&cur).copied().flatten();
            let t = self.target[g] as usize - 1;
            let (k, ai) = (t >> 16, t & 0xffff);
            let _ = &access[k][ai];
            (path, src, (k, ai))
        });
        for &s in &self.touched {
            self.dist[s as usize] = f32::INFINITY;
            self.parent[s as usize] = u32::MAX;
        }
        self.touched.clear();
        res
    }

    /// Cells where this route overlaps another net's halo.
    fn conflicts(&self, ni: usize) -> Vec<usize> {
        let n = self.grid.cells();
        let rt = &self.routes[ni];
        let mut out = Vec::new();
        for path in &rt.paths {
            for w in path.windows(2) {
                if w[0] / n != w[1] / n {
                    let c = w[0] % n;
                    if (0..self.grid.nl).any(|l| self.occ_v[l * n + c] > 1) {
                        out.push(n * self.grid.nl + c);
                    }
                }
            }
            for &s in path {
                if self.occ_t[s] > 1 {
                    out.push(s);
                }
            }
        }
        for &(_, _, s) in &rt.stubs {
            if self.occ_t[s] > 1 {
                out.push(s);
            }
        }
        out
    }

    fn all_geometry(&self) -> (Vec<Track>, Vec<NewVia>) {
        let mut tracks = Vec::new();
        let mut vias = Vec::new();
        for ni in 0..self.nets.len() {
            let c = self.classes[self.nets[ni].class];
            let id = self.nets[ni].id;
            let (segs, vs) = self.geometry(ni);
            for (l, a, b) in segs {
                tracks.push(Track { net: id, layer: l, a, b, width: c.w });
            }
            for q in vs {
                vias.push(NewVia { net: id, pos: q, diameter: c.vd, drill: c.vdrill });
            }
        }
        (tracks, vias)
    }

    pub fn run(mut self) -> Output {
        let t0 = std::time::Instant::now();
        let mut stats = Stats { pitch: self.grid.g, cells: self.grid.cells() * self.grid.nl, nets: self.nets.len(), ..Default::default() };
        let order = self.net_order();
        let obs = Obstacles::new(self.board, self.rules);
        let mut pres = self.prm.pres_fac;
        let mut snapshots = Vec::new();
        let mut dirty: Vec<bool> = vec![true; self.nets.len()];
        for it in 0..self.prm.max_iters {
            for &ni in &order {
                if !dirty[ni] {
                    continue;
                }
                self.rip_up(ni);
                self.route_net(ni, if it == 0 { 0.0 } else { pres }, false);
                self.stamp_route(ni);
            }
            let mut total = 0;
            for ni in 0..self.nets.len() {
                let cf = self.conflicts(ni);
                dirty[ni] = !cf.is_empty();
                total += cf.len();
                let n = self.grid.cells();
                for s in cf {
                    if s >= self.grid.nl * n {
                        self.hist_v[s - self.grid.nl * n] += self.prm.hist_fac;
                    } else {
                        self.hist_t[s] += self.prm.hist_fac;
                    }
                }
            }
            stats.iterations = it + 1;
            stats.conflicts_per_iter.push(total);
            if self.prm.snapshots {
                let (tracks, vias) = self.all_geometry();
                snapshots.push(Snapshot { iteration: it, conflicts: total, tracks, vias });
            }
            if total == 0 {
                break;
            }
            // Halos are conservative: when few conflicts remain, ask the exact DRC whether
            // any of them is real.
            if total <= 40 && (it < 10 || it % 5 == 0) {
                let (tracks, vias) = self.all_geometry();
                if drc::check(&obs, &tracks, &vias).is_empty() {
                    break;
                }
            }
            pres *= self.prm.pres_growth;
        }
        // Legalise: nets still in conflict are rerouted with hard blocking, then the exact
        // DRC has the last word.
        for ni in self.net_order() {
            if !self.conflicts(ni).is_empty() {
                self.rip_up(ni);
                self.route_net(ni, pres, true);
                self.stamp_route(ni);
            }
        }
        for round in 0..3 {
            let (tracks, vias) = self.all_geometry();
            let viol = drc::check(&obs, &tracks, &vias);
            if viol.is_empty() {
                break;
            }
            let bad: HashSet<u32> = viol.iter().map(|v| v.net).collect();
            for ni in self.net_order() {
                if bad.contains(&self.nets[ni].id) {
                    self.rip_up(ni);
                    if round < 2 {
                        self.route_net(ni, pres, true);
                        self.stamp_route(ni);
                    } else {
                        stats.drc_dropped += 1;
                    }
                }
            }
        }
        let (tracks, vias) = self.all_geometry();
        for (ni, net) in self.nets.iter().enumerate() {
            stats.connections += net.pads.len() - 1;
            let rt = &self.routes[ni];
            let done = rt.paths.len();
            stats.connections_routed += done;
            if done == net.pads.len() - 1 {
                stats.nets_complete += 1;
            }
        }
        stats.tracks = tracks.len();
        stats.vias = vias.len();
        stats.length_mm = tracks.iter().map(|t| t.a.dist(t.b)).sum();
        stats.ms = t0.elapsed().as_millis();
        if self.prm.snapshots {
            snapshots.push(Snapshot { iteration: stats.iterations, conflicts: 0, tracks: tracks.clone(), vias: vias.clone() });
        }
        Output { tracks, vias, stats, snapshots }
    }
}

fn net_class_dbg(r: &Router, net: u32) -> usize {
    let c = class_of(r.board, r.rules, net);
    r.classes.iter().position(|k| k.w == c.w && k.cl == c.cl).unwrap_or(0)
}

/// Route a board, retrying on finer grids while connections are missing: narrow
/// corridors (between 2.54 mm header pads, say) only exist on a fine enough grid.
pub fn route_board(board: &Board, rules: &Rules, prm: &Params) -> Output {
    let first = Router::new(board, rules, prm.clone());
    let g0 = first.grid().g;
    let mut best = first.run();
    for f in [0.6, 0.4] {
        if best.stats.connections_routed == best.stats.connections || prm.pitch.is_some() {
            break;
        }
        let g = (g0 * f).max(0.03);
        if g >= best.stats.pitch - 1e-9 {
            break;
        }
        let mut p2 = prm.clone();
        p2.pitch = Some(g);
        let ms = best.stats.ms;
        let mut out = Router::new(board, rules, p2).run();
        out.stats.ms += ms;
        if out.stats.connections_routed > best.stats.connections_routed {
            best = out;
        } else {
            best.stats.ms = out.stats.ms;
        }
    }
    best
}
