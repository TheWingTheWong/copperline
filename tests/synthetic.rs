//! Router invariants on generated boards: whatever it routes must pass copperline's exact
//! DRC, and every net it reports complete must be connected the way KiCad sees it.

use copperline::board::Board;
use copperline::connect;
use copperline::drc::{self, Obstacles};
use copperline::rng::Rng;
use copperline::router::{Params, Router};
use copperline::rules::Rules;

struct Pad {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    tht: bool,
    net: usize,
}

fn board_text(w: f64, h: f64, pads: &[Pad], nets: usize) -> String {
    let mut s = String::from("(kicad_pcb\n\t(version 20241229)\n\t(generator \"test\")\n\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(2 \"B.Cu\" signal)\n\t\t(25 \"Edge.Cuts\" user)\n\t)\n");
    s.push_str("\t(net 0 \"\")\n");
    for n in 1..=nets {
        s.push_str(&format!("\t(net {n} \"N{n}\")\n"));
    }
    for (i, pd) in pads.iter().enumerate() {
        let (kind, shape, layers, drill) = if pd.tht {
            ("thru_hole", "circle", "\"*.Cu\" \"*.Mask\"", format!("\n\t\t\t(drill {:.3})", pd.w * 0.5))
        } else {
            ("smd", "rect", "\"F.Cu\" \"F.Mask\"", String::new())
        };
        let net = if pd.net == 0 { String::new() } else { format!("\n\t\t\t(net {} \"N{}\")", pd.net, pd.net) };
        s.push_str(&format!(
            "\t(footprint \"T:P\"\n\t\t(layer \"F.Cu\")\n\t\t(at {:.4} {:.4})\n\t\t(property \"Reference\" \"U{i}\")\n\t\t(pad \"1\" {kind} {shape}\n\t\t\t(at 0 0)\n\t\t\t(size {:.4} {:.4}){drill}\n\t\t\t(layers {layers}){net}\n\t\t)\n\t)\n",
            pd.x, pd.y, pd.w, pd.h
        ));
    }
    for (a, b) in [((0.0, 0.0), (w, 0.0)), ((w, 0.0), (w, h)), ((w, h), (0.0, h)), ((0.0, h), (0.0, 0.0))] {
        s.push_str(&format!("\t(gr_line\n\t\t(start {} {})\n\t\t(end {} {})\n\t\t(layer \"Edge.Cuts\")\n\t)\n", a.0, a.1, b.0, b.1));
    }
    s.push_str(")\n");
    s
}

fn rules(clearance: f64, track: f64) -> Rules {
    Rules::parse(&format!(
        r#"{{"board":{{"design_settings":{{"rules":{{"min_clearance":0.1,"min_copper_edge_clearance":0.3,"min_hole_clearance":0.25,"min_hole_to_hole":0.25}}}}}},
        "net_settings":{{"classes":[{{"name":"Default","clearance":{clearance},"track_width":{track},"via_diameter":0.6,"via_drill":0.3}}]}}}}"#
    ))
    .unwrap()
}

/// Random pads on a grid that keeps them legal, nets of 2-4 pads.
fn random_board(seed: u64) -> String {
    let mut r = Rng::new(seed);
    let (w, h) = (30.0, 22.0);
    let mut pads = Vec::new();
    let mut taken = std::collections::HashSet::new();
    let nets = 6 + r.below(6);
    for net in 1..=nets {
        for _ in 0..2 + r.below(3) {
            loop {
                let (gx, gy) = (1 + r.below(13), 1 + r.below(9));
                if taken.insert((gx, gy)) {
                    let tht = r.chance(0.4);
                    let (pw, ph) = if tht { (1.6, 1.6) } else { (r.range(0.6, 1.4), r.range(0.6, 1.4)) };
                    pads.push(Pad { x: gx as f64 * 2.1 + r.range(-0.2, 0.2), y: gy as f64 * 2.1 + r.range(-0.2, 0.2), w: pw, h: ph, tht, net });
                    break;
                }
            }
        }
    }
    board_text(w, h, &pads, nets)
}

/// A 0.5 mm pitch QFN-24 (0.25 x 0.8 mm pads, 0.2 mm clearance) fanned out to resistor pads.
fn fine_pitch_board() -> String {
    let mut pads = Vec::new();
    let (cx, cy) = (15.0, 15.0);
    let mut net = 0;
    for side in 0..4 {
        for k in 0..6 {
            net += 1;
            let off = (k as f64 - 2.5) * 0.5;
            let (x, y, w, h) = match side {
                0 => (cx + off, cy - 2.0, 0.25, 0.8),
                1 => (cx + 2.0, cy + off, 0.8, 0.25),
                2 => (cx - off, cy + 2.0, 0.25, 0.8),
                _ => (cx - 2.0, cy - off, 0.8, 0.25),
            };
            pads.push(Pad { x, y, w, h, tht: false, net });
            // Partner pad on a ring 9-11 mm out.
            let ang = (side as f64 * 6.0 + k as f64) / 24.0 * std::f64::consts::TAU - std::f64::consts::FRAC_PI_2;
            let rr = if k % 2 == 0 { 9.0 } else { 11.5 };
            pads.push(Pad { x: cx + rr * ang.cos(), y: cy + rr * ang.sin(), w: 1.0, h: 1.0, tht: false, net });
        }
    }
    board_text(30.0, 30.0, &pads, net)
}

fn check(text: &str, rules: &Rules) -> (usize, usize, usize) {
    let b = Board::parse(text).unwrap();
    let out = Router::new(&b, rules, Params::default()).run();
    let obs = Obstacles::new(&b, rules);
    let v = drc::check(&obs, &out.tracks, &out.vias);
    assert!(v.is_empty(), "router shipped {} violations, first {:?}", v.len(), v.first());
    let comps = connect::components(&b, &out.tracks, &out.vias);
    let complete = comps.iter().filter(|c| c.2 == 1).count();
    // The router's own count of complete nets must agree with the independent check.
    assert_eq!(complete, out.stats.nets_complete, "router reports {} complete nets, connectivity check finds {}", out.stats.nets_complete, complete);
    (complete, comps.len(), out.stats.vias)
}

#[test]
fn random_boards_route_clean_and_connected() {
    let r = rules(0.2, 0.25);
    let (mut done, mut total) = (0, 0);
    for seed in 1..=12 {
        let (c, n, _) = check(&random_board(seed), &r);
        done += c;
        total += n;
    }
    // Sparse boards: nearly everything should route.
    assert!(done * 10 >= total * 9, "only {done}/{total} nets complete");
}

#[test]
fn fine_pitch_qfn_escapes() {
    let r = rules(0.2, 0.2);
    if let Some(dir) = std::env::var_os("COPPERLINE_FIXTURES") {
        std::fs::write(std::path::Path::new(&dir).join("qfn24.kicad_pcb"), fine_pitch_board()).unwrap();
    }
    let (c, n, _) = check(&fine_pitch_board(), &r);
    assert_eq!(c, n, "QFN fan-out: {c}/{n} nets complete");
}

#[test]
fn output_keeps_the_input_byte_for_byte() {
    use copperline::router::route_board;
    use copperline::write;
    let text = random_board(3);
    let b = Board::parse(&text).unwrap();
    let r = rules(0.2, 0.25);
    let out = route_board(&b, &r, &Params::default());
    assert!(!out.tracks.is_empty());
    let block = write::block(&b, &out.tracks, &out.vias, 1);
    let routed = write::append(&text, &block).unwrap();
    // Removing the appended block gives back the input exactly.
    assert_eq!(routed.replacen(&block, "", 1), text);
    // The routed file parses, and its new copper is exactly what the router produced.
    let rb = Board::parse(&routed).unwrap();
    assert_eq!(rb.fixed.len() - b.fixed.len(), out.tracks.len() + out.vias.len() * b.layers.len());
    assert_eq!(rb.vias.len(), out.vias.len());
    // Fresh, unique UUIDs.
    let uuids: std::collections::HashSet<&str> = block.match_indices("(uuid \"").map(|(i, _)| &block[i + 7..i + 43]).collect();
    assert_eq!(uuids.len(), out.tracks.len() + out.vias.len());
    assert!(!uuids.iter().any(|u| text.contains(u)));
}

#[test]
fn pth_pad_listing_only_front_copper_is_on_every_layer() {
    // Found by KiCad on a real board: KiCad treats PTH pad copper as present on all layers.
    let text = "(kicad_pcb\n\t(version 20241229)\n\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(2 \"B.Cu\" signal)\n\t)\n\t(net 0 \"\")\n\t(net 1 \"GND\")\n\t(footprint \"T:P\"\n\t\t(layer \"F.Cu\")\n\t\t(at 10 10)\n\t\t(pad \"3\" thru_hole rect\n\t\t\t(at 0 0 270)\n\t\t\t(size 2 2.5)\n\t\t\t(drill 1.016)\n\t\t\t(layers \"*.Mask\" \"F.Cu\" \"F.SilkS\")\n\t\t\t(net 1 \"GND\")\n\t\t)\n\t)\n)\n";
    let b = Board::parse(text).unwrap();
    assert_eq!(b.pads[0].layers, vec![0, 1]);
}

#[test]
fn drc_sees_tracks_on_opposite_sides_of_a_bucket_edge() {
    // Found by KiCad on a real board: two new tracks 0.1493 mm apart were never compared
    // because they fell into different 1 mm buckets.
    use copperline::drc::Track;
    use copperline::geom::p;
    let text = "(kicad_pcb\n\t(version 20241229)\n\t(layers\n\t\t(0 \"F.Cu\" signal)\n\t\t(2 \"B.Cu\" signal)\n\t)\n\t(net 0 \"\")\n\t(net 1 \"A\")\n\t(net 2 \"B\")\n)\n";
    let b = Board::parse(text).unwrap();
    let r = rules(0.15, 0.2);
    let obs = Obstacles::new(&b, &r);
    // Edges at x = 0.9 and x = 1.2: gap 0.3 - 0.2 = 0.1 < 0.15, across the x = 1.0 bucket edge.
    let t = [
        Track { net: 1, layer: 0, a: p(0.8, 0.5), b: p(0.8, 5.0), width: 0.2 },
        Track { net: 2, layer: 0, a: p(1.1, 0.5), b: p(1.1, 5.0), width: 0.2 },
    ];
    assert!(!drc::check(&obs, &t, &[]).is_empty());
}
