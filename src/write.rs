//! Append routed tracks and vias to a `.kicad_pcb` without touching anything else.

use crate::board::Board;
use crate::drc::{NewVia, Track};
use crate::rng::Rng;

fn uuid(r: &mut Rng) -> String {
    let a = r.next_u64();
    let b = r.next_u64();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:04x}-{:012x}",
        a >> 32,
        (a >> 16) & 0xffff,
        a & 0xfff,
        0x8000 | ((b >> 48) & 0x3fff),
        b & 0xffff_ffff_ffff
    )
}

fn mm(v: f64) -> String {
    let s = format!("{:.6}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// The routed block KiCad 9 reads: one `segment` per track, one through `via` per via.
pub fn block(board: &Board, tracks: &[Track], vias: &[NewVia], seed: u64) -> String {
    let mut r = Rng::new(seed);
    let mut out = String::new();
    for t in tracks {
        out.push_str(&format!(
            "\t(segment\n\t\t(start {} {})\n\t\t(end {} {})\n\t\t(width {})\n\t\t(layer \"{}\")\n\t\t(net {})\n\t\t(uuid \"{}\")\n\t)\n",
            mm(t.a.x), mm(t.a.y), mm(t.b.x), mm(t.b.y), mm(t.width), board.layers[t.layer], t.net, uuid(&mut r)
        ));
    }
    let (first, last) = (board.layers.first().cloned().unwrap_or_default(), board.layers.last().cloned().unwrap_or_default());
    for v in vias {
        out.push_str(&format!(
            "\t(via\n\t\t(at {} {})\n\t\t(size {})\n\t\t(drill {})\n\t\t(layers \"{}\" \"{}\")\n\t\t(net {})\n\t\t(uuid \"{}\")\n\t)\n",
            mm(v.pos.x), mm(v.pos.y), mm(v.diameter), mm(v.drill), first, last, v.net, uuid(&mut r)
        ));
    }
    out
}

/// The original file text with `block` inserted before the final closing parenthesis.
pub fn append(src: &str, block: &str) -> Result<String, String> {
    let end = src.rfind(')').ok_or("not a kicad_pcb file")?;
    let mut out = String::with_capacity(src.len() + block.len());
    out.push_str(&src[..end]);
    if !out.ends_with('\n') {
        out.push('\n');
    }
    out.push_str(block);
    out.push_str(&src[end..]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_block_round_trips_bytes() {
        let src = "(kicad_pcb\n\t(version 20241229)\n)\n";
        assert_eq!(append(src, "").unwrap(), src);
    }

    #[test]
    fn appended_block_strips_back_to_the_original() {
        let src = "(kicad_pcb\n\t(version 20241229)\n)\n";
        let blk = "\t(via\n\t)\n";
        let out = append(src, blk).unwrap();
        assert_eq!(out.replacen(blk, "", 1), src);
    }

    #[test]
    fn uuids_are_distinct_v4() {
        let mut r = Rng::new(7);
        let a = uuid(&mut r);
        let b = uuid(&mut r);
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(&a[14..15], "4");
    }
}
