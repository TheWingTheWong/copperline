//! Connectivity of routed copper, the way KiCad decides it: a track end connects to a pad
//! when it lies inside the pad's copper, to another track when it touches that track's
//! centerline, and to a via when it sits on the via's centre.

use crate::board::Board;
use crate::drc::{NewVia, Track};
use crate::geom::point_seg_dist;

fn find(p: &mut [usize], mut i: usize) -> usize {
    while p[i] != i {
        p[i] = p[p[i]];
        i = p[i];
    }
    i
}

fn union(p: &mut [usize], a: usize, b: usize) {
    let (ra, rb) = (find(p, a), find(p, b));
    if ra != rb {
        p[ra] = rb;
    }
}

/// For every net with two or more pads: (net, pads, connected components among its pads).
pub fn components(b: &Board, tracks: &[Track], vias: &[NewVia]) -> Vec<(u32, usize, usize)> {
    const TOL: f64 = 1e-6;
    let mut out = Vec::new();
    for (net, pads) in b.net_pads() {
        let ts: Vec<&Track> = tracks.iter().filter(|t| t.net == net).collect();
        let vs: Vec<&NewVia> = vias.iter().filter(|v| v.net == net).collect();
        // Items: pads, then tracks, then vias.
        let (np, nt) = (pads.len(), ts.len());
        let mut par: Vec<usize> = (0..np + nt + vs.len()).collect();
        for (ti, t) in ts.iter().enumerate() {
            for end in [t.a, t.b] {
                for (pi, &pd) in pads.iter().enumerate() {
                    let pad = &b.pads[pd];
                    if pad.on_layer(t.layer) && pad.shapes.iter().any(|s| s.point_gap(end) <= TOL) {
                        union(&mut par, pi, np + ti);
                    }
                }
                for (tj, u) in ts.iter().enumerate() {
                    if tj != ti && u.layer == t.layer && point_seg_dist(end, u.a, u.b) <= TOL {
                        union(&mut par, np + ti, np + tj);
                    }
                }
                for (vi, v) in vs.iter().enumerate() {
                    if v.pos.dist(end) <= TOL {
                        union(&mut par, np + ti, np + nt + vi);
                    }
                }
            }
        }
        // A via sitting inside a through-hole or same-layer pad also connects.
        for (vi, v) in vs.iter().enumerate() {
            for (pi, &pd) in pads.iter().enumerate() {
                if b.pads[pd].shapes.iter().any(|s| s.point_gap(v.pos) <= TOL) {
                    union(&mut par, pi, np + nt + vi);
                }
            }
        }
        let mut roots: Vec<usize> = (0..np).map(|i| find(&mut par, i)).collect();
        roots.sort_unstable();
        roots.dedup();
        out.push((net, np, roots.len()));
    }
    out
}
