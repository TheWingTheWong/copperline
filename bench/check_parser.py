"""Check copperline's pad geometry against KiCad's own pad polygons on many boards.

usage: python3 bench/check_parser.py --split dev-medium [--limit N]

For every board: KiCad (pcbnew, via bench/kicad_tool.py pads) dumps each pad's effective
copper polygon; copperline dumps its parsed shapes (`copperline pads-json`); every KiCad
vertex must lie inside or on our shape within 0.006 mm (KiCad's arc approximation error).
Our shapes may be larger (conservative), never smaller. Writes bench/results/parser-<split>.txt.
"""
import argparse
import json
import subprocess
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))
import run  # noqa: E402  (paths and tool locations)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", required=True)
    ap.add_argument("--limit", type=int, default=0)
    a = ap.parse_args()
    names = json.loads((HERE / "splits.json").read_text())[a.split]
    if a.limit:
        names = names[: a.limit]
    lines, boards, pads, bad_boards = [], 0, 0, 0
    with tempfile.TemporaryDirectory() as td:
        for n in names:
            src = run.folder(n) / "processed_v9_guide_v3_unrouted.kicad_pcb"
            if not src.exists():
                continue
            k, c = Path(td) / "k.json", Path(td) / "c.json"
            run.kpy("pads", src, k)
            c.write_text(subprocess.run([run.COPPERLINE, "pads-json", str(src)], capture_output=True, text=True, check=True).stdout)
            r = subprocess.run([sys.executable, str(HERE / "check_pads.py"), str(k), str(c)], capture_output=True, text=True)
            last = r.stdout.strip().splitlines()[-1]
            boards += 1
            pads += int(last.split()[1])
            if r.returncode:
                bad_boards += 1
                lines.append(f"{n}: {last}")
                lines += ["    " + x for x in r.stdout.strip().splitlines()[:-1][:5]]
    summary = f"{a.split}: {boards} boards, {pads} pads checked, {bad_boards} boards with a pad where KiCad copper lies outside ours"
    lines.insert(0, summary)
    (HERE / "results" / f"parser-{a.split}.txt").write_text("\n".join(lines) + "\n")
    print("\n".join(lines[:30]))


if __name__ == "__main__":
    main()
