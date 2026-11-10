//! Board model extracted from a KiCad 9 `.kicad_pcb` file.
//!
//! Only what routing needs is kept: copper layers in stack order, nets, pads with exact
//! copper shapes and holes, the board outline, rule-area keepouts and any copper that is
//! already on the board (tracks, vias, copper graphics, zone fills).

use crate::geom::{self, BBox, Core, P, Shape, p};
use crate::sexpr::Sexp;

/// Tolerance used when turning arcs into chords for obstacles (mm). Chords are pushed
/// outward by this amount, so polygonised arcs are never smaller than the real copper.
pub const ARC_TOL: f64 = 0.002;

#[derive(Debug, Clone)]
pub struct Pad {
    pub footprint: String,
    pub number: String,
    pub net: u32,
    pub pos: P,
    /// Copper on each copper layer index (empty when the pad has no copper there).
    pub layers: Vec<usize>,
    /// Copper outline: union of shapes, identical on every layer it is on.
    pub shapes: Vec<Shape>,
    /// Drilled hole, if any (`plated` false for NPTH).
    pub hole: Option<Shape>,
    pub plated: bool,
    /// Pad's own clearance override (mm), if set.
    pub clearance: Option<f64>,
}

impl Pad {
    pub fn bbox(&self) -> BBox {
        self.shapes.iter().fold(BBox::empty(), |b, s| b.union(&s.bbox()))
    }
    pub fn on_layer(&self, l: usize) -> bool {
        self.layers.contains(&l)
    }
    pub fn gap_to(&self, s: &Shape) -> f64 {
        self.shapes.iter().map(|a| a.gap(s)).fold(f64::INFINITY, f64::min)
    }
}

/// A piece of fixed copper that is not a pad: an existing track, arc, via ring, copper
/// graphic or zone fill.
#[derive(Debug, Clone)]
pub struct Fixed {
    pub net: u32,
    pub layer: usize,
    pub shape: Shape,
}

#[derive(Debug, Clone)]
pub struct Via {
    pub pos: P,
    pub diameter: f64,
    pub drill: f64,
    pub net: u32,
}

#[derive(Debug, Clone)]
pub struct Keepout {
    pub outline: Vec<P>,
    /// Copper layer indices the keepout applies to.
    pub layers: Vec<usize>,
    pub no_tracks: bool,
    pub no_vias: bool,
    pub no_pads: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Board {
    /// Copper layer names in stack order (F.Cu first, B.Cu last).
    pub layers: Vec<String>,
    /// Net names indexed by net code (0 is the unconnected net "").
    pub nets: Vec<String>,
    pub pads: Vec<Pad>,
    pub fixed: Vec<Fixed>,
    pub vias: Vec<Via>,
    pub holes: Vec<Shape>,
    pub keepouts: Vec<Keepout>,
    /// Board edge as segments (Edge.Cuts, arcs and circles polygonised).
    pub edges: Vec<(P, P)>,
    pub bbox: BBox,
    /// Number of copper zones (pours) found; fills are included in `fixed`.
    pub copper_zones: usize,
    pub warnings: Vec<String>,
}

fn xy(node: &Sexp) -> Option<P> {
    Some(p(node.arg_num(0)?, node.arg_num(1)?))
}

fn child_xy(node: &Sexp, name: &str) -> Option<P> {
    node.child(name).and_then(xy)
}

fn pts_of(node: &Sexp) -> Vec<P> {
    node.child("pts")
        .map(|pts| pts.children("xy").filter_map(xy).collect())
        .unwrap_or_default()
}

/// Expand a KiCad layer list (`"*.Cu"`, `"F&B.Cu"`, explicit names) to copper indices.
fn copper_layers_of(names: &[&str], layers: &[String]) -> Vec<usize> {
    let mut out = Vec::new();
    for n in names {
        match *n {
            "*.Cu" => out.extend(0..layers.len()),
            "F&B.Cu" => {
                out.push(0);
                if layers.len() > 1 {
                    out.push(layers.len() - 1);
                }
            }
            name => {
                if let Some(i) = layers.iter().position(|l| l == name) {
                    out.push(i);
                }
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

fn layer_names(node: &Sexp) -> Vec<&str> {
    let mut v: Vec<&str> = Vec::new();
    for c in node.children("layers").chain(node.children("layer")) {
        v.extend(c.args().iter().filter_map(|a| a.text()));
    }
    v
}

/// Copper layers sorted in physical stack order.
fn parse_layers(root: &Sexp) -> Vec<String> {
    let mut cu: Vec<String> = Vec::new();
    if let Some(ls) = root.child("layers") {
        for l in ls.args() {
            let (Some(name), Some(kind)) = (l.items().get(1).and_then(|x| x.text()), l.items().get(2).and_then(|x| x.text())) else {
                continue;
            };
            if name.ends_with(".Cu") && matches!(kind, "signal" | "power" | "mixed" | "jumper") {
                cu.push(name.to_string());
            }
        }
    }
    let rank = |n: &str| -> i64 {
        match n {
            "F.Cu" => -1,
            "B.Cu" => i64::MAX,
            _ => n.trim_start_matches("In").trim_end_matches(".Cu").parse().unwrap_or(1000),
        }
    };
    cu.sort_by_key(|n| rank(n));
    cu
}

/// A pad's copper shapes in board coordinates.
fn pad_shapes(pad: &Sexp, pos: P, angle: f64, warnings: &mut Vec<String>) -> Vec<Shape> {
    let shape = pad.arg_text(2).unwrap_or("rect");
    let size = child_xy(pad, "size").unwrap_or(p(0.0, 0.0));
    let (w, h) = (size.x, size.y);
    let offset = pad.child("drill").and_then(|d| child_xy(d, "offset")).unwrap_or(p(0.0, 0.0));
    let place = |q: P| (q + offset).rot_kicad(angle) + pos;
    let poly = |pts: &[P], r: f64| Shape::poly(pts.iter().map(|&q| place(q)).collect(), r);
    let rect = |w: f64, h: f64| [p(-w / 2.0, -h / 2.0), p(w / 2.0, -h / 2.0), p(w / 2.0, h / 2.0), p(-w / 2.0, h / 2.0)];
    let mut out = Vec::new();
    match shape {
        "circle" => out.push(Shape::circle(place(p(0.0, 0.0)), w / 2.0)),
        "oval" => {
            if (w - h).abs() < 1e-9 {
                out.push(Shape::circle(place(p(0.0, 0.0)), w / 2.0));
            } else if w > h {
                let d = (w - h) / 2.0;
                out.push(Shape::capsule(place(p(-d, 0.0)), place(p(d, 0.0)), h / 2.0));
            } else {
                let d = (h - w) / 2.0;
                out.push(Shape::capsule(place(p(0.0, -d)), place(p(0.0, d)), w / 2.0));
            }
        }
        "rect" => out.push(poly(&rect(w, h), 0.0)),
        "roundrect" => {
            let ratio = pad.child("roundrect_rratio").and_then(|r| r.arg_num(0)).unwrap_or(0.25).clamp(0.0, 0.5);
            let r = ratio * w.min(h);
            let (iw, ih) = ((w - 2.0 * r).max(0.0), (h - 2.0 * r).max(0.0));
            if pad.child("chamfer_ratio").is_some() && pad.child("chamfer").is_some() {
                out.push(poly(&chamfered(pad, w, h), 0.0));
            } else if iw < 1e-9 && ih < 1e-9 {
                out.push(Shape::circle(place(p(0.0, 0.0)), r));
            } else if iw < 1e-9 || ih < 1e-9 {
                out.push(Shape::capsule(place(p(-iw / 2.0, -ih / 2.0)), place(p(iw / 2.0, ih / 2.0)), r));
            } else {
                out.push(poly(&rect(iw, ih), r));
            }
        }
        "trapezoid" => {
            let d = child_xy(pad, "rect_delta").unwrap_or(p(0.0, 0.0)) * 0.5;
            let (hx, hy) = (w / 2.0, h / 2.0);
            let c = [p(-hx - d.y, hy + d.x), p(hx + d.y, hy - d.x), p(hx - d.y, -hy + d.x), p(-hx + d.y, -hy - d.x)];
            out.push(poly(&c, 0.0));
        }
        "custom" => {
            // Anchor pad plus primitives, all in pad-local coordinates.
            let anchor = pad.child("options").and_then(|o| o.child("anchor")).and_then(|a| a.arg_text(0)).unwrap_or("rect");
            if anchor == "circle" {
                out.push(Shape::circle(place(p(0.0, 0.0)), w / 2.0));
            } else {
                out.push(poly(&rect(w, h), 0.0));
            }
            if let Some(prims) = pad.child("primitives") {
                for prim in prims.args() {
                    let width = prim.child("width").and_then(|x| x.arg_num(0)).unwrap_or(0.0);
                    let r = width / 2.0;
                    match prim.name() {
                        Some("gr_poly") => out.push(poly(&pts_of(prim), r)),
                        Some("gr_line") => {
                            if let (Some(a), Some(b)) = (child_xy(prim, "start"), child_xy(prim, "end")) {
                                out.push(Shape::capsule(place(a), place(b), r));
                            }
                        }
                        Some("gr_rect") => {
                            if let (Some(a), Some(b)) = (child_xy(prim, "start"), child_xy(prim, "end")) {
                                out.push(poly(&[a, p(b.x, a.y), b, p(a.x, b.y)], r));
                            }
                        }
                        Some("gr_circle") => {
                            if let (Some(c), Some(e)) = (child_xy(prim, "center"), child_xy(prim, "end")) {
                                out.push(Shape::circle(place(c), c.dist(e) + r));
                            }
                        }
                        Some("gr_arc") => {
                            if let (Some(a), Some(m), Some(b)) = (child_xy(prim, "start"), child_xy(prim, "mid"), child_xy(prim, "end")) {
                                let pts = geom::arc_points(a, m, b, ARC_TOL);
                                for wnd in pts.windows(2) {
                                    out.push(Shape::capsule(place(wnd[0]), place(wnd[1]), r + ARC_TOL));
                                }
                            }
                        }
                        Some(other) => warnings.push(format!("custom pad primitive {other} ignored")),
                        None => {}
                    }
                }
            }
        }
        other => {
            warnings.push(format!("unknown pad shape {other}, using rect"));
            out.push(poly(&rect(w, h), 0.0));
        }
    }
    out
}

/// Rectangle with the corners named in `(chamfer ...)` cut at `chamfer_ratio * min(w, h)`.
fn chamfered(pad: &Sexp, w: f64, h: f64) -> Vec<P> {
    let ratio = pad.child("chamfer_ratio").and_then(|r| r.arg_num(0)).unwrap_or(0.0);
    let c = ratio * w.min(h);
    let corners: Vec<&str> = pad.child("chamfer").map(|x| x.args().iter().filter_map(|a| a.text()).collect()).unwrap_or_default();
    let has = |n: &str| corners.contains(&n);
    let (hx, hy) = (w / 2.0, h / 2.0);
    let mut pts = Vec::new();
    // Clockwise from top-left on screen (y down).
    if has("top_left") { pts.extend([p(-hx, -hy + c), p(-hx + c, -hy)]); } else { pts.push(p(-hx, -hy)); }
    if has("top_right") { pts.extend([p(hx - c, -hy), p(hx, -hy + c)]); } else { pts.push(p(hx, -hy)); }
    if has("bottom_right") { pts.extend([p(hx, hy - c), p(hx - c, hy)]); } else { pts.push(p(hx, hy)); }
    if has("bottom_left") { pts.extend([p(-hx + c, hy), p(-hx, hy - c)]); } else { pts.push(p(-hx, hy)); }
    pts
}

/// Outline segments of a graphic item on Edge.Cuts, transformed by `tf`.
fn edge_segments(item: &Sexp, tf: &dyn Fn(P) -> P) -> Vec<(P, P)> {
    let mut out = Vec::new();
    match item.name() {
        Some("gr_line" | "fp_line") => {
            if let (Some(a), Some(b)) = (child_xy(item, "start"), child_xy(item, "end")) {
                out.push((tf(a), tf(b)));
            }
        }
        Some("gr_rect" | "fp_rect") => {
            if let (Some(a), Some(b)) = (child_xy(item, "start"), child_xy(item, "end")) {
                let c = [a, p(b.x, a.y), b, p(a.x, b.y)];
                for i in 0..4 {
                    out.push((tf(c[i]), tf(c[(i + 1) % 4])));
                }
            }
        }
        Some("gr_arc" | "fp_arc") => {
            if let (Some(a), Some(m), Some(b)) = (child_xy(item, "start"), child_xy(item, "mid"), child_xy(item, "end")) {
                let pts = geom::arc_points(a, m, b, ARC_TOL / 2.0);
                for w in pts.windows(2) {
                    out.push((tf(w[0]), tf(w[1])));
                }
            }
        }
        Some("gr_circle" | "fp_circle") => {
            if let (Some(c), Some(e)) = (child_xy(item, "center"), child_xy(item, "end")) {
                let r = c.dist(e);
                let n = ((std::f64::consts::TAU * r / 0.2).ceil() as usize).clamp(24, 720);
                let pts: Vec<P> = (0..=n)
                    .map(|i| {
                        let a = std::f64::consts::TAU * i as f64 / n as f64;
                        p(c.x + r * a.cos(), c.y + r * a.sin())
                    })
                    .collect();
                for w in pts.windows(2) {
                    out.push((tf(w[0]), tf(w[1])));
                }
            }
        }
        Some("gr_poly" | "fp_poly") => {
            let pts = pts_of(item);
            for i in 0..pts.len() {
                out.push((tf(pts[i]), tf(pts[(i + 1) % pts.len()])));
            }
        }
        Some("gr_curve" | "fp_curve") => {
            let pts = pts_of(item);
            if pts.len() == 4 {
                let n = 32;
                let bez = |t: f64| {
                    let u = 1.0 - t;
                    pts[0] * (u * u * u) + pts[1] * (3.0 * u * u * t) + pts[2] * (3.0 * u * t * t) + pts[3] * (t * t * t)
                };
                for i in 0..n {
                    out.push((tf(bez(i as f64 / n as f64)), tf(bez((i + 1) as f64 / n as f64))));
                }
            }
        }
        _ => {}
    }
    out
}

/// Copper shapes of a graphic item drawn on a copper layer.
fn graphic_copper(item: &Sexp, tf: &dyn Fn(P) -> P) -> Vec<Shape> {
    let width = item.child("stroke").and_then(|s| s.child("width")).or_else(|| item.child("width")).and_then(|w| w.arg_num(0)).unwrap_or(0.0);
    let filled = item.child("fill").is_some_and(|f| matches!(f.arg_text(0), Some("yes" | "solid")));
    let r = width / 2.0;
    match item.name() {
        Some("gr_poly" | "fp_poly") if filled => vec![Shape::poly(pts_of(item).into_iter().map(tf).collect(), r)],
        Some("gr_rect" | "fp_rect") if filled => {
            let (Some(a), Some(b)) = (child_xy(item, "start"), child_xy(item, "end")) else { return vec![] };
            vec![Shape::poly([a, p(b.x, a.y), b, p(a.x, b.y)].into_iter().map(tf).collect(), r)]
        }
        Some("gr_circle" | "fp_circle") if filled => {
            let (Some(c), Some(e)) = (child_xy(item, "center"), child_xy(item, "end")) else { return vec![] };
            vec![Shape::circle(tf(c), c.dist(e) + r)]
        }
        _ => edge_segments(item, tf).into_iter().map(|(a, b)| Shape::capsule(a, b, r + ARC_TOL)).collect(),
    }
}

impl Board {
    pub fn parse(src: &str) -> Result<Board, String> {
        let root = crate::sexpr::parse(src).map_err(|e| e.to_string())?;
        if root.name() != Some("kicad_pcb") {
            return Err("not a kicad_pcb file".into());
        }
        let version = root.child("version").and_then(|v| v.arg_num(0)).unwrap_or(0.0);
        if version < 20221018.0 {
            return Err(format!("board file version {version} is older than KiCad 7; open and save it in KiCad 9 first"));
        }
        let mut b = Board { layers: parse_layers(&root), ..Default::default() };
        if b.layers.is_empty() {
            return Err("no copper layers".into());
        }
        let layer_idx = |name: &str, layers: &[String]| layers.iter().position(|l| l == name);

        for n in root.children("net") {
            let code = n.arg_num(0).unwrap_or(0.0) as usize;
            let name = n.arg_text(1).unwrap_or("").to_string();
            if b.nets.len() <= code {
                b.nets.resize(code + 1, String::new());
            }
            b.nets[code] = name;
        }
        if b.nets.is_empty() {
            b.nets.push(String::new());
        }
        let net_of = |node: &Sexp| node.child("net").and_then(|n| n.arg_num(0)).unwrap_or(0.0) as u32;

        let id = |q: P| q;
        for item in root.args() {
            let name = item.name().unwrap_or("");
            match name {
                "footprint" => b.add_footprint(item),
                "segment" => {
                    let (Some(a), Some(e)) = (child_xy(item, "start"), child_xy(item, "end")) else { continue };
                    let w = item.child("width").and_then(|w| w.arg_num(0)).unwrap_or(0.0);
                    if let Some(l) = item.child("layer").and_then(|l| l.arg_text(0)).and_then(|n| layer_idx(n, &b.layers)) {
                        b.fixed.push(Fixed { net: net_of(item), layer: l, shape: Shape::capsule(a, e, w / 2.0) });
                    }
                }
                "arc" => {
                    let (Some(a), Some(m), Some(e)) = (child_xy(item, "start"), child_xy(item, "mid"), child_xy(item, "end")) else { continue };
                    let w = item.child("width").and_then(|w| w.arg_num(0)).unwrap_or(0.0);
                    if let Some(l) = item.child("layer").and_then(|l| l.arg_text(0)).and_then(|n| layer_idx(n, &b.layers)) {
                        let pts = geom::arc_points(a, m, e, ARC_TOL);
                        for wnd in pts.windows(2) {
                            b.fixed.push(Fixed { net: net_of(item), layer: l, shape: Shape::capsule(wnd[0], wnd[1], w / 2.0 + ARC_TOL) });
                        }
                    }
                }
                "via" => {
                    let Some(at) = child_xy(item, "at") else { continue };
                    let d = item.child("size").and_then(|s| s.arg_num(0)).unwrap_or(0.6);
                    let drill = item.child("drill").and_then(|s| s.arg_num(0)).unwrap_or(0.3);
                    let net = net_of(item);
                    let ls = copper_layers_of(&layer_names(item), &b.layers);
                    let (lo, hi) = (ls.first().copied().unwrap_or(0), ls.last().copied().unwrap_or(b.layers.len() - 1));
                    for l in lo..=hi {
                        b.fixed.push(Fixed { net, layer: l, shape: Shape::circle(at, d / 2.0) });
                    }
                    b.holes.push(Shape::circle(at, drill / 2.0));
                    b.vias.push(Via { pos: at, diameter: d, drill, net });
                }
                "zone" => b.add_zone(item, None),
                "gr_line" | "gr_arc" | "gr_circle" | "gr_rect" | "gr_poly" | "gr_curve" => {
                    let lname = item.child("layer").and_then(|l| l.arg_text(0)).unwrap_or("");
                    if lname == "Edge.Cuts" {
                        b.edges.extend(edge_segments(item, &id));
                    } else if let Some(l) = layer_idx(lname, &b.layers) {
                        for s in graphic_copper(item, &id) {
                            b.fixed.push(Fixed { net: net_of(item), layer: l, shape: s });
                        }
                    }
                }
                "gr_text" | "gr_text_box" => {
                    let lname = item.child("layer").and_then(|l| l.arg_text(0)).unwrap_or("");
                    if let Some(l) = layer_idx(lname, &b.layers) {
                        if let Some(s) = text_box(item, None) {
                            b.fixed.push(Fixed { net: 0, layer: l, shape: s });
                        }
                    }
                }
                _ => {}
            }
        }
        b.bbox = BBox::of_points(&b.edges.iter().flat_map(|&(a, e)| [a, e]).collect::<Vec<_>>());
        if b.bbox.is_empty() {
            b.bbox = b.pads.iter().fold(BBox::empty(), |acc, pd| acc.union(&pd.bbox())).inflate(1.0);
            b.warnings.push("no Edge.Cuts outline; using pad extents".into());
        }
        Ok(b)
    }

    fn add_footprint(&mut self, fp: &Sexp) {
        let at = fp.child("at");
        let pos = at.and_then(xy).unwrap_or_default();
        let angle = at.and_then(|a| a.arg_num(2)).unwrap_or(0.0);
        let reference = fp
            .children("property")
            .find(|pr| pr.arg_text(0) == Some("Reference"))
            .and_then(|pr| pr.arg_text(1))
            .unwrap_or("?")
            .to_string();
        let tf = move |q: P| q.rot_kicad(angle) + pos;
        for item in fp.args() {
            match item.name() {
                Some("pad") => {
                    let pat = item.child("at");
                    let local = pat.and_then(xy).unwrap_or_default();
                    // Pad orientation in the file is absolute (includes the footprint's).
                    let pangle = pat.and_then(|a| a.arg_num(2)).unwrap_or(0.0);
                    let ppos = tf(local);
                    let kind = item.arg_text(1).unwrap_or("smd");
                    let cu = copper_layers_of(&layer_names(item), &self.layers);
                    let shapes = pad_shapes(item, ppos, pangle, &mut self.warnings);
                    let hole = item.child("drill").and_then(|d| {
                        let offset_free = |q: P| q.rot_kicad(pangle) + ppos;
                        if d.has_atom("oval") {
                            let (w, h) = (d.arg_num(1)?, d.arg_num(2).or(d.arg_num(1))?);
                            if w > h {
                                let k = (w - h) / 2.0;
                                Some(Shape::capsule(offset_free(p(-k, 0.0)), offset_free(p(k, 0.0)), h / 2.0))
                            } else {
                                let k = (h - w) / 2.0;
                                Some(Shape::capsule(offset_free(p(0.0, -k)), offset_free(p(0.0, k)), w / 2.0))
                            }
                        } else {
                            let dia = d.arg_num(0)?;
                            (dia > 0.0).then(|| Shape::circle(ppos, dia / 2.0))
                        }
                    });
                    let plated = kind == "thru_hole";
                    let net = item.child("net").and_then(|n| n.arg_num(0)).unwrap_or(0.0) as u32;
                    if let Some(h) = &hole {
                        self.holes.push(h.clone());
                    }
                    // NPTH pads only carry copper when they are larger than their hole.
                    let has_copper = kind != "np_thru_hole" || hole.as_ref().is_some_and(|h| {
                        let hb = h.bbox();
                        let pb = shapes.iter().fold(BBox::empty(), |acc, s| acc.union(&s.bbox()));
                        pb.max.x - pb.min.x > hb.max.x - hb.min.x + 1e-6 || pb.max.y - pb.min.y > hb.max.y - hb.min.y + 1e-6
                    });
                    let clearance = item.child("clearance").and_then(|c| c.arg_num(0));
                    self.pads.push(Pad {
                        footprint: reference.clone(),
                        number: item.arg_text(0).unwrap_or("").to_string(),
                        net,
                        pos: ppos,
                        layers: if has_copper { cu } else { Vec::new() },
                        shapes,
                        hole,
                        plated,
                        clearance,
                    });
                }
                Some("fp_line" | "fp_arc" | "fp_circle" | "fp_rect" | "fp_poly" | "fp_curve") => {
                    let lname = item.child("layer").and_then(|l| l.arg_text(0)).unwrap_or("");
                    if lname == "Edge.Cuts" {
                        self.edges.extend(edge_segments(item, &tf));
                    } else if let Some(l) = self.layers.iter().position(|x| x == lname) {
                        for s in graphic_copper(item, &tf) {
                            self.fixed.push(Fixed { net: 0, layer: l, shape: s });
                        }
                    }
                }
                Some("fp_text") => {
                    let lname = item.child("layer").and_then(|l| l.arg_text(0)).unwrap_or("");
                    if !item.has_atom("hide") {
                        if let Some(l) = self.layers.iter().position(|x| x == lname) {
                            if let Some(s) = text_box(item, Some(&tf)) {
                                self.fixed.push(Fixed { net: 0, layer: l, shape: s });
                            }
                        }
                    }
                }
                Some("zone") => self.add_zone(item, Some(&tf)),
                _ => {}
            }
        }
    }

    fn add_zone(&mut self, z: &Sexp, _tf: Option<&dyn Fn(P) -> P>) {
        // Zone coordinates are absolute, even inside footprints.
        let names = layer_names(z);
        let layers = copper_layers_of(&names, &self.layers);
        let outline = z.child("polygon").map(pts_of).unwrap_or_default();
        if let Some(k) = z.child("keepout") {
            let na = |what: &str| k.child(what).and_then(|x| x.arg_text(0)) == Some("not_allowed");
            self.keepouts.push(Keepout { outline, layers, no_tracks: na("tracks"), no_vias: na("vias"), no_pads: na("pads") });
            return;
        }
        self.copper_zones += 1;
        let net = z.child("net").and_then(|n| n.arg_num(0)).unwrap_or(0.0) as u32;
        for fp in z.children("filled_polygon") {
            let lname = fp.child("layer").and_then(|l| l.arg_text(0)).unwrap_or("");
            if let Some(l) = self.layers.iter().position(|x| x == lname) {
                let pts = pts_of(fp);
                if pts.len() >= 3 {
                    self.fixed.push(Fixed { net, layer: l, shape: Shape::poly(pts, 0.0) });
                }
            }
        }
    }

    pub fn net_name(&self, net: u32) -> &str {
        self.nets.get(net as usize).map(|s| s.as_str()).unwrap_or("")
    }

    pub fn net_by_name(&self, name: &str) -> Option<u32> {
        self.nets.iter().position(|n| n == name).map(|i| i as u32)
    }

    /// Pads grouped by net (only nets with at least two pads that carry copper).
    pub fn net_pads(&self) -> Vec<(u32, Vec<usize>)> {
        let mut by: std::collections::BTreeMap<u32, Vec<usize>> = Default::default();
        for (i, pd) in self.pads.iter().enumerate() {
            if pd.net != 0 && !pd.layers.is_empty() {
                by.entry(pd.net).or_default().push(i);
            }
        }
        by.into_iter().filter(|(_, v)| v.len() >= 2).collect()
    }

    /// True when `q` is inside the board outline (even-odd over all edge segments).
    pub fn inside(&self, q: P) -> bool {
        let mut inside = false;
        for &(a, b) in &self.edges {
            if (a.y > q.y) != (b.y > q.y) {
                let x = a.x + (q.y - a.y) * (b.x - a.x) / (b.y - a.y);
                if q.x < x {
                    inside = !inside;
                }
            }
        }
        inside
    }

    /// Distance from `q` to the nearest board edge.
    pub fn edge_dist(&self, q: P) -> f64 {
        self.edges.iter().map(|&(a, b)| geom::point_seg_dist(q, a, b)).fold(f64::INFINITY, f64::min)
    }

    pub fn shape_edge_gap(&self, s: &Shape) -> f64 {
        self.edges.iter().map(|&(a, b)| s.gap(&Shape::capsule(a, b, 0.0))).fold(f64::INFINITY, f64::min)
    }
}

/// Conservative bounding box of a text item (KiCad stroke font is narrower than this).
fn text_box(item: &Sexp, tf: Option<&dyn Fn(P) -> P>) -> Option<Shape> {
    let text = item.arg_text(1).or(item.arg_text(0))?;
    let at = item.child("at");
    let pos = at.and_then(xy)?;
    let angle = at.and_then(|a| a.arg_num(2)).unwrap_or(0.0);
    let font = item.child("effects").and_then(|e| e.child("font"));
    let size = font.and_then(|f| child_xy(f, "size")).unwrap_or(p(1.0, 1.0));
    let thick = font.and_then(|f| f.child("thickness")).and_then(|t| t.arg_num(0)).unwrap_or(0.15);
    let lines = text.lines().count().max(1) as f64;
    let chars = text.lines().map(|l| l.chars().count()).max().unwrap_or(1) as f64;
    let (hw, hh) = (chars * size.x * 0.6 + thick, lines * size.y * 0.8 + thick);
    let corners = [p(-hw, -hh), p(hw, -hh), p(hw, hh), p(-hw, hh)];
    let pts = corners.iter().map(|&c| {
        let q = c.rot_kicad(angle) + pos;
        match tf {
            Some(f) => f(q - pos + pos),
            None => q,
        }
    });
    Some(Shape::poly(pts.collect(), 0.0))
}

impl Shape {
    pub fn core_points(&self) -> Vec<P> {
        match &self.core {
            Core::Point(q) => vec![*q],
            Core::Seg(a, b) => vec![*a, *b],
            Core::Poly(v) => v.clone(),
        }
    }
}
