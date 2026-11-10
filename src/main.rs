use std::path::{Path, PathBuf};
use std::process::ExitCode;

use copperline::board::Board;
use copperline::geom::Core;
use copperline::router::{Params, route_board};
use copperline::rules::Rules;
use copperline::write;

const USAGE: &str = "copperline: PCB autorouter for KiCad 9 boards

usage:
  copperline route <in.kicad_pcb> -o <out.kicad_pcb> [--stats stats.json] [--pitch mm] [--iters N] [--budget s] [--hweight w]
                   [--frames DIR [--frame-width PX]]   write one PPM per routing step
  copperline check <board.kicad_pcb>          run copperline's own DRC on the tracks in a board
  copperline info <board.kicad_pcb>
  copperline svg <board.kicad_pcb> -o <out.svg> [--width px]
  copperline pads-json <board.kicad_pcb>      dump parsed pad shapes as JSON (testing)
";

fn load(path: &Path) -> Result<(Board, Rules, String), String> {
    let src = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let board = Board::parse(&src)?;
    let pro: PathBuf = path.with_extension("kicad_pro");
    let rules = match std::fs::read_to_string(&pro) {
        Ok(j) => Rules::parse(&j)?,
        Err(_) => Rules::default(),
    };
    Ok((board, rules, src))
}

fn run(args: &[String]) -> Result<(), String> {
    match args.first().map(|s| s.as_str()) {
        Some("info") => {
            let (b, r, _) = load(Path::new(args.get(1).ok_or(USAGE)?))?;
            let nets = b.net_pads();
            println!("layers {:?}", b.layers);
            println!("pads {} nets-to-route {} connections {}", b.pads.len(), nets.len(), nets.iter().map(|(_, v)| v.len() - 1).sum::<usize>());
            println!("fixed copper {} vias {} keepouts {} copper zones {}", b.fixed.len(), b.vias.len(), b.keepouts.len(), b.copper_zones);
            println!("outline segments {} bbox {:.2},{:.2} .. {:.2},{:.2}", b.edges.len(), b.bbox.min.x, b.bbox.min.y, b.bbox.max.x, b.bbox.max.y);
            for c in &r.classes {
                println!("class {} clearance {} track {} via {}/{}", c.name, c.clearance, c.track_width, c.via_diameter, c.via_drill);
            }
            for w in b.warnings.iter().take(10) {
                println!("warning: {w}");
            }
            Ok(())
        }
        Some("route") => {
            let input = Path::new(args.get(1).ok_or(USAGE)?);
            let mut out_path = None;
            let mut stats_path = None;
            let mut frames: Option<PathBuf> = None;
            let mut frame_w = 800usize;
            let mut prm = Params::default();
            let mut i = 2;
            while i < args.len() {
                let val = args.get(i + 1).ok_or(USAGE)?;
                match args[i].as_str() {
                    "-o" => out_path = Some(PathBuf::from(val)),
                    "--stats" => stats_path = Some(PathBuf::from(val)),
                    "--pitch" => prm.pitch = Some(val.parse().map_err(|_| "bad --pitch")?),
                    "--iters" => prm.max_iters = val.parse().map_err(|_| "bad --iters")?,
                    "--hweight" => prm.h_weight = val.parse().map_err(|_| "bad --hweight")?,
                    "--budget" => prm.time_budget_s = val.parse().map_err(|_| "bad --budget")?,
                    "--frames" => frames = Some(PathBuf::from(val)),
                    "--frame-width" => frame_w = val.parse().map_err(|_| "bad --frame-width")?,
                    other => return Err(format!("unknown option {other}\n{USAGE}")),
                }
                i += 2;
            }
            let out_path = out_path.ok_or(USAGE)?;
            let (b, r, src) = load(input)?;
            prm.snapshots = frames.is_some();
            let res = route_board(&b, &r, &prm);
            if let Some(dir) = &frames {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
                for (i, f) in res.snapshots.iter().enumerate() {
                    let img = copperline::raster::frame(&b, &f.tracks, &f.vias, &f.hot, frame_w);
                    std::fs::write(dir.join(format!("frame_{i:04}.ppm")), img.ppm()).map_err(|e| e.to_string())?;
                }
            }
            let s = &res.stats;
            eprintln!(
                "pitch {:.3} mm, {} nets, {}/{} connections, {} iterations, {} tracks, {} vias, {:.1} mm, {} ms",
                s.pitch, s.nets, s.connections_routed, s.connections, s.iterations, s.tracks, s.vias, s.length_mm, s.ms
            );
            let text = write::append(&src, &write::block(&b, &res.tracks, &res.vias, 0x5eed))?;
            std::fs::write(&out_path, text).map_err(|e| format!("{}: {e}", out_path.display()))?;
            if let Some(sp) = stats_path {
                let j = serde_json::json!({
                    "pitch": s.pitch, "cells": s.cells, "nets": s.nets, "nets_complete": s.nets_complete,
                    "connections": s.connections, "connections_routed": s.connections_routed,
                    "iterations": s.iterations, "conflicts_per_iter": s.conflicts_per_iter,
                    "drc_dropped": s.drc_dropped, "tracks": s.tracks, "vias": s.vias,
                    "length_mm": s.length_mm, "ms": s.ms,
                });
                std::fs::write(&sp, j.to_string()).map_err(|e| e.to_string())?;
            }
            Ok(())
        }
        Some("svg") => {
            let input = Path::new(args.get(1).ok_or(USAGE)?);
            let out = args.iter().position(|a| a == "-o").and_then(|i| args.get(i + 1)).ok_or(USAGE)?;
            let width = args.iter().position(|a| a == "--width").and_then(|i| args.get(i + 1)).and_then(|v| v.parse().ok()).unwrap_or(1200.0);
            let (b, _, _) = load(input)?;
            std::fs::write(out, copperline::svg::render(&b, &[], &[], width)).map_err(|e| e.to_string())?;
            Ok(())
        }
        Some("pads-json") => {
            let (b, _, _) = load(Path::new(args.get(1).ok_or(USAGE)?))?;
            let mut out = Vec::new();
            for pd in &b.pads {
                let shapes: Vec<serde_json::Value> = pd
                    .shapes
                    .iter()
                    .map(|s| {
                        let (kind, pts) = match &s.core {
                            Core::Point(q) => ("point", vec![*q]),
                            Core::Seg(a, c) => ("seg", vec![*a, *c]),
                            Core::Poly(v) => ("poly", v.clone()),
                        };
                        serde_json::json!({"kind": kind, "r": s.radius, "pts": pts.iter().map(|q| [q.x, q.y]).collect::<Vec<_>>()})
                    })
                    .collect();
                out.push(serde_json::json!({"ref": pd.footprint, "num": pd.number, "net": pd.net, "x": pd.pos.x, "y": pd.pos.y, "layers": pd.layers, "shapes": shapes}));
            }
            println!("{}", serde_json::Value::Array(out));
            Ok(())
        }
        _ => Err(USAGE.to_string()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
