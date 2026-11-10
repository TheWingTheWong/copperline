use std::path::{Path, PathBuf};
use std::process::ExitCode;

use copperline::board::Board;
use copperline::geom::Core;
use copperline::rules::Rules;

const USAGE: &str = "copperline: PCB autorouter for KiCad 9 boards

usage:
  copperline info <board.kicad_pcb>
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
