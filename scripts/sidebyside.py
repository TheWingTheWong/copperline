"""Side-by-side renders of copperline vs Freerouting on benchmark boards.

usage: python3 scripts/sidebyside.py <board> [<board> ...]   (D3 names, e.g. 0441_kitspace_spike_n_hold)

Reads the routed files left in bench/work/{copperline,freerouting}/<board>/routed.kicad_pcb and
the scores in bench/results/*.csv, renders both with `copperline render`, and writes
assets/compare-<board>.png via headless Chrome (scripts/shot.sh).
"""
import base64
import csv
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "target/release/copperline"


def score(board, router):
    for f in sorted((ROOT / "bench/results").glob(f"*-{router}*.csv")):
        if "-v2" in f.name:
            continue
        for r in csv.DictReader(open(f)):
            if r["board"] == board:
                return r
    return None


def label(r, name):
    if r is None:
        return f"{name}: not run"
    if r["clean"] == "1":
        verdict = "clean"
    elif r["status"] != "ok":
        verdict = r["status"]
    else:
        parts = []
        if r["unconnected"] not in ("", "0"):
            parts.append(f"{r['unconnected']} unconnected")
        if r["violations"] not in ("", "0"):
            parts.append(f"{r['violations']} DRC errors ({r['violation_types']})")
        verdict = ", ".join(parts)
    return f"{name}: {verdict}, {float(r['route_s'] or 0):.1f} s, {r['vias'] or 0} vias"


def main():
    for board in sys.argv[1:]:
        with tempfile.TemporaryDirectory() as td:
            imgs = []
            for router, name in [("copperline", "copperline"), ("freerouting", "Freerouting 2.1.0")]:
                src = ROOT / "bench/work" / router / board / "routed.kicad_pcb"
                ppm, png = Path(td) / f"{router}.ppm", Path(td) / f"{router}.png"
                subprocess.run([str(BIN), "render", str(src), "-o", str(ppm), "--width", "640"], check=True)
                subprocess.run(["ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-i", str(ppm), str(png)], check=True)
                imgs.append((base64.b64encode(png.read_bytes()).decode(), label(score(board, router), name)))
            title = board.split("_", 1)[1]
            cells = "".join(f'<figure><img src="data:image/png;base64,{b}"><figcaption>{c}</figcaption></figure>' for b, c in imgs)
            html = (f"<html><body style='margin:0;background:#0b0f14;color:#d8dee6;font:15px Helvetica,Arial'>"
                    f"<div style='padding:12px 16px 0;font-size:17px'>{title} <span style='color:#8592a3'>(PCBench, same input for both)</span></div>"
                    f"<div style='display:flex;gap:12px;padding:8px 16px'>{cells}</div>"
                    f"<style>figure{{margin:0}} img{{width:640px;display:block;border-radius:6px}} figcaption{{padding:6px 2px;color:#c9d1db}}</style></body></html>")
            page = Path(td) / "p.html"
            page.write_text(html)
            from_ppm = (Path(td) / "copperline.ppm").read_bytes().split(b"\n", 2)[1].split()
            h = int(int(from_ppm[1]) * 640 / int(from_ppm[0])) + 90
            out = ROOT / "assets" / f"compare-{board.split('_', 1)[0]}.png"
            subprocess.run([str(ROOT / "scripts/shot.sh"), str(page), str(out), "1330", str(h)], check=True)
            print(out)


if __name__ == "__main__":
    main()
