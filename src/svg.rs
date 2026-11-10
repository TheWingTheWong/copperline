//! SVG rendering of a board (pads, outline, tracks, vias) for docs and debugging.

use crate::board::Board;
use crate::drc::{NewVia, Track};
use crate::geom::{Core, Shape};

const LAYER_COLORS: [&str; 4] = ["#d9483b", "#3b7dd8", "#e0a030", "#40a070"];

fn color(l: usize, nl: usize) -> &'static str {
    if l + 1 == nl && nl > 1 { LAYER_COLORS[1] } else if l == 0 { LAYER_COLORS[0] } else { LAYER_COLORS[2 + (l % 2)] }
}

fn shape_svg(s: &Shape, fill: &str, opacity: f64) -> String {
    match &s.core {
        Core::Point(q) => format!("<circle cx=\"{:.4}\" cy=\"{:.4}\" r=\"{:.4}\" fill=\"{fill}\" fill-opacity=\"{opacity}\"/>", q.x, q.y, s.radius.max(0.01)),
        Core::Seg(a, b) => format!(
            "<line x1=\"{:.4}\" y1=\"{:.4}\" x2=\"{:.4}\" y2=\"{:.4}\" stroke=\"{fill}\" stroke-opacity=\"{opacity}\" stroke-width=\"{:.4}\" stroke-linecap=\"round\"/>",
            a.x, a.y, b.x, b.y, (2.0 * s.radius).max(0.02)
        ),
        Core::Poly(pts) => {
            let d: Vec<String> = pts.iter().map(|q| format!("{:.4},{:.4}", q.x, q.y)).collect();
            let sw = if s.radius > 0.0 { format!(" stroke=\"{fill}\" stroke-width=\"{:.4}\" stroke-linejoin=\"round\" stroke-opacity=\"{opacity}\"", 2.0 * s.radius) } else { String::new() };
            format!("<polygon points=\"{}\" fill=\"{fill}\" fill-opacity=\"{opacity}\"{sw}/>", d.join(" "))
        }
    }
}

/// Render the board. `extra` tracks/vias are drawn on top (routed output).
pub fn render(b: &Board, extra_tracks: &[Track], extra_vias: &[NewVia], width_px: f64) -> String {
    let bb = b.bbox.inflate(1.0);
    let (w, h) = (bb.max.x - bb.min.x, bb.max.y - bb.min.y);
    let nl = b.layers.len();
    let mut s = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"{:.3} {:.3} {:.3} {:.3}\" width=\"{:.0}\" height=\"{:.0}\">\n<rect x=\"{:.3}\" y=\"{:.3}\" width=\"{:.3}\" height=\"{:.3}\" fill=\"#10151c\"/>\n",
        bb.min.x, bb.min.y, w, h, width_px, width_px * h / w, bb.min.x, bb.min.y, w, h
    );
    if !b.edges.is_empty() {
        s.push_str("<path d=\"");
        for &(a, e) in &b.edges {
            s.push_str(&format!("M{:.4} {:.4}L{:.4} {:.4}", a.x, a.y, e.x, e.y));
        }
        s.push_str("\" stroke=\"#c8c26a\" stroke-width=\"0.15\" fill=\"none\"/>\n");
    }
    // Bottom layers first so the top layer reads on top.
    for l in (0..nl).rev() {
        let c = color(l, nl);
        for f in b.fixed.iter().filter(|f| f.layer == l) {
            s.push_str(&shape_svg(&f.shape, c, 0.85));
        }
        for t in extra_tracks.iter().filter(|t| t.layer == l) {
            s.push_str(&shape_svg(&t.shape(), c, 0.85));
        }
    }
    for pd in &b.pads {
        let top = pd.layers.contains(&0);
        let c = if top { "#e8b04a" } else { "#7aa7e0" };
        for sh in &pd.shapes {
            s.push_str(&shape_svg(sh, c, 1.0));
        }
        if let Some(hole) = &pd.hole {
            s.push_str(&shape_svg(hole, "#10151c", 1.0));
        }
    }
    for v in &b.vias {
        s.push_str(&shape_svg(&Shape::circle(v.pos, v.diameter / 2.0), "#cfcfcf", 1.0));
        s.push_str(&shape_svg(&Shape::circle(v.pos, v.drill / 2.0), "#10151c", 1.0));
    }
    for v in extra_vias {
        s.push_str(&shape_svg(&Shape::circle(v.pos, v.diameter / 2.0), "#cfcfcf", 1.0));
        s.push_str(&shape_svg(&Shape::circle(v.pos, v.drill / 2.0), "#10151c", 1.0));
    }
    s.push_str("</svg>\n");
    s
}
