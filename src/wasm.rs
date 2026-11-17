//! Browser entry points, plain C ABI (no wasm-bindgen): the page copies the board and
//! project text into memory from `cl_alloc`, calls `cl_route`, and reads back one JSON
//! document with the board geometry, every routing frame, the stats and the routed file.

use crate::board::Board;
use crate::geom::{Core, Shape};
use crate::router::{Params, route_board};
use crate::rules::Rules;
use crate::write;
use serde_json::{Value, json};

static mut RESULT: Vec<u8> = Vec::new();

#[unsafe(no_mangle)]
pub extern "C" fn cl_alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len.max(1));
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr` must come from `cl_alloc(cap)` and not be freed twice.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cl_free(ptr: *mut u8, cap: usize) {
    unsafe { drop(Vec::from_raw_parts(ptr, 0, cap.max(1))) };
}

fn shape_json(s: &Shape) -> Value {
    match &s.core {
        Core::Point(q) => json!({"k": "c", "p": [[q.x, q.y]], "r": s.radius}),
        Core::Seg(a, b) => json!({"k": "s", "p": [[a.x, a.y], [b.x, b.y]], "r": s.radius}),
        Core::Poly(v) => json!({"k": "p", "p": v.iter().map(|q| [q.x, q.y]).collect::<Vec<_>>(), "r": s.radius}),
    }
}

fn r3(v: f64) -> f64 {
    (v * 1000.0).round() / 1000.0
}

pub fn route_json(pcb: &str, pro: &str) -> Result<Value, String> {
    let b = Board::parse(pcb)?;
    let r = if pro.trim().is_empty() { Rules::default() } else { Rules::parse(pro)? };
    let prm = Params { snapshots: true, time_budget_s: 60.0, ..Params::default() };
    let out = route_board(&b, &r, &prm);
    let pads: Vec<Value> = b
        .pads
        .iter()
        .map(|pd| json!({"top": pd.layers.contains(&0), "net": pd.net, "s": pd.shapes.iter().map(shape_json).collect::<Vec<_>>(), "h": pd.hole.as_ref().map(shape_json)}))
        .collect();
    let frames: Vec<Value> = out
        .snapshots
        .iter()
        .map(|f| {
            json!({
                "it": f.iteration, "conflicts": f.conflicts, "hot": f.hot,
                "t": f.tracks.iter().map(|t| [t.layer as f64, r3(t.a.x), r3(t.a.y), r3(t.b.x), r3(t.b.y), t.width, t.net as f64]).collect::<Vec<_>>(),
                "v": f.vias.iter().map(|v| [r3(v.pos.x), r3(v.pos.y), v.diameter, v.drill]).collect::<Vec<_>>(),
            })
        })
        .collect();
    let s = &out.stats;
    let routed = write::append(pcb, &write::block(&b, &out.tracks, &out.vias, 0x5eed))?;
    Ok(json!({
        "layers": b.layers, "bbox": [b.bbox.min.x, b.bbox.min.y, b.bbox.max.x, b.bbox.max.y],
        "edges": b.edges.iter().map(|(a, e)| [a.x, a.y, e.x, e.y]).collect::<Vec<_>>(),
        "fixed": b.fixed.iter().map(|f| json!({"l": f.layer, "s": shape_json(&f.shape)})).collect::<Vec<_>>(),
        "pads": pads, "frames": frames,
        "stats": {"pitch": s.pitch, "nets": s.nets, "nets_complete": s.nets_complete, "connections": s.connections,
                  "connections_routed": s.connections_routed, "iterations": s.iterations, "tracks": s.tracks,
                  "vias": s.vias, "length_mm": s.length_mm, "ms": s.ms},
        "routed": routed,
    }))
}

/// Route `pcb` (UTF-8, `pcb_len` bytes) with the rules in `pro` (may be empty). Returns a
/// pointer to the JSON result; its length is `cl_result_len()`.
///
/// # Safety
/// Pointers must reference `*_len` readable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn cl_route(pcb: *const u8, pcb_len: usize, pro: *const u8, pro_len: usize) -> *const u8 {
    let (pcb, pro) = unsafe { (std::slice::from_raw_parts(pcb, pcb_len), std::slice::from_raw_parts(pro, pro_len)) };
    let res = match (std::str::from_utf8(pcb), std::str::from_utf8(pro)) {
        (Ok(a), Ok(b)) => route_json(a, b).unwrap_or_else(|e| json!({"error": e})),
        _ => json!({"error": "input is not UTF-8"}),
    };
    unsafe {
        let r = &mut *std::ptr::addr_of_mut!(RESULT);
        *r = res.to_string().into_bytes();
        r.as_ptr()
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cl_result_len() -> usize {
    unsafe { (*std::ptr::addr_of!(RESULT)).len() }
}
