"""Route every board of a split with copperline and/or Freerouting and score each result
with KiCad 9's own DRC.

usage:
  python3 bench/run.py --split test-a --router freerouting [--limit N] [--boards a,b]
  python3 bench/run.py --split dev-easy --router copperline

Inputs are PCBWorld's D3 guide boards (`processed_v9_guide_v3_unrouted.kicad_pcb` plus the
guide `.kicad_pro`) under $D3_ROOT/<board>/. Results are appended to
bench/results/<split>-<router>.csv (one row per board; rows already present are skipped
unless --force), so an interrupted run resumes where it stopped.

Scoring (PCBWorld's clean-pass): a routed board is clean when KiCad reports zero
unconnected items and zero error-severity violations, after subtracting, per violation
type, any violation already present on the unrouted input. Zones are refilled with pcbnew's
ZONE_FILLER before DRC for both routers (kicad-cli 9 has no refill flag).

Environment: KICAD_CLI, KICAD_PYTHON (an interpreter that imports pcbnew), JAVA,
FREEROUTING_JAR, COPPERLINE (binary), D3_ROOT. Defaults point at a macOS KiCad 9 install.
Boards are routed one at a time, Freerouting with -Xmx2g, so the run fits on an 8 GB laptop.
"""
import argparse
import collections
import csv
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
KICAD_APP = "/Applications/KiCad9/KiCad.app/Contents"
KICAD_CLI = os.environ.get("KICAD_CLI", f"{KICAD_APP}/MacOS/kicad-cli")
KICAD_PYTHON = os.environ.get("KICAD_PYTHON", f"{KICAD_APP}/Frameworks/Python.framework/Versions/Current/bin/python3")
JAVA = os.environ.get("JAVA", "/opt/homebrew/opt/openjdk/bin/java")
FREEROUTING_JAR = os.environ.get("FREEROUTING_JAR", str(ROOT.parent.parent / "research/tools/freerouting-2.1.0.jar"))
COPPERLINE = os.environ.get("COPPERLINE", str(ROOT / "target/release/copperline"))
D3_ROOT = Path(os.environ.get("D3_ROOT", str(ROOT.parent.parent / "research/d3work/pcbench_work/newdrc")))
WORK = HERE / "work"
# Build provenance for copperline rows (the binary must be built from this checkout).
COMMIT = subprocess.run(["git", "describe", "--always", "--dirty", "--tags"], cwd=ROOT, capture_output=True, text=True).stdout.strip()
RESULTS = HERE / "results"
CAP_S = 1800

FIELDS = ["board", "router", "commit", "seed", "status", "clean", "unconnected", "violations", "baseline_violations",
          "violation_types", "route_s", "convert_s", "tracks", "vias", "length_mm"]


def folder(name):
    """D3 names carry a 4-digit index prefix; the prep chain's folders do not."""
    return D3_ROOT / name.split("_", 1)[1]


def drc(board):
    out = board.with_suffix(".drc.json")
    if "(zone" in board.read_text(errors="replace"):
        kpy("refill", board)
    subprocess.run([KICAD_CLI, "pcb", "drc", "--format", "json", "--severity-error", "--all-track-errors",
                    "-o", str(out), str(board)], capture_output=True, timeout=600)
    rep = json.loads(out.read_text())
    types = collections.Counter(v["type"] for v in rep.get("violations", []))
    return len(rep.get("unconnected_items", [])), types


def kpy(*args, timeout=600):
    r = subprocess.run([KICAD_PYTHON, str(HERE / "kicad_tool.py"), *map(str, args)], capture_output=True,
                       text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError(f"kicad_tool {args[0]} failed: {r.stderr.strip()[-300:]}")
    return r.stdout


def route_freerouting(src, dst, seed):
    t0 = time.monotonic()
    dsn, ses = dst.with_suffix(".dsn"), dst.with_suffix(".ses")
    kpy("dsn", src, dsn)
    conv = time.monotonic() - t0
    cmd = [JAVA, "-Djava.awt.headless=true", f"-Djava.io.tmpdir={dst.parent}", "-Xmx2g", "-jar", FREEROUTING_JAR,
           "-de", str(dsn), "-do", str(ses), "-mp", "10"]
    if seed is not None:
        cmd += ["--random_seed", str(seed)]
    t1 = time.monotonic()
    with open(dst.with_suffix(".log"), "w") as log:
        subprocess.run(cmd, stdout=log, stderr=subprocess.STDOUT, timeout=CAP_S)
    route = time.monotonic() - t1
    if not ses.exists():
        raise RuntimeError("no SES written")
    t2 = time.monotonic()
    kpy("ses", src, ses, dst)
    conv += time.monotonic() - t2
    return route, conv


def route_copperline(src, dst, seed):
    t0 = time.monotonic()
    with open(dst.with_suffix(".log"), "w") as log:
        r = subprocess.run([COPPERLINE, "route", str(src), "-o", str(dst), "--stats", str(dst.with_suffix(".stats.json"))],
                           stdout=log, stderr=subprocess.STDOUT, timeout=CAP_S)
    if r.returncode not in (0, 2) or not dst.exists():  # 2 = legal but incomplete
        raise RuntimeError(f"copperline exited {r.returncode}")
    return time.monotonic() - t0, 0.0


ROUTERS = {"freerouting": route_freerouting, "copperline": route_copperline}


def run_board(name, router, seed):
    src_dir = folder(name)
    wdir = WORK / router / name
    wdir.mkdir(parents=True, exist_ok=True)
    src = wdir / "input.kicad_pcb"
    shutil.copy(src_dir / "processed_v9_guide_v3_unrouted.kicad_pcb", src)
    shutil.copy(src_dir / "processed_v9_guide_v3.kicad_pro", wdir / "input.kicad_pro")
    bfile = WORK / "baseline" / f"{name}.json"
    if not bfile.exists():
        bfile.parent.mkdir(parents=True, exist_ok=True)
        bfile.write_text(json.dumps(drc(src)[1]))
    base = collections.Counter(json.loads(bfile.read_text()))
    dst = wdir / "routed.kicad_pcb"
    shutil.copy(wdir / "input.kicad_pro", wdir / "routed.kicad_pro")
    row = {"board": name, "router": router, "commit": COMMIT if router == "copperline" else "freerouting-2.1.0", "seed": "" if seed is None else seed,
           "baseline_violations": sum(base.values())}
    try:
        route_s, conv_s = ROUTERS[router](src, dst, seed)
    except subprocess.TimeoutExpired:
        return {**row, "status": "timeout", "clean": 0, "route_s": CAP_S}
    except RuntimeError as e:
        return {**row, "status": f"error: {e}"[:120], "clean": 0}
    unconnected, types = drc(dst)
    adj = collections.Counter({t: max(0, n - base.get(t, 0)) for t, n in types.items()})
    adj = +adj
    sj = dst.with_suffix(".stats.json")
    st = json.loads(sj.read_text()) if router == "copperline" else json.loads(kpy("stats", dst))
    return {**row, "status": "ok", "clean": int(unconnected == 0 and not adj), "unconnected": unconnected,
            "violations": sum(adj.values()), "violation_types": ";".join(f"{t}:{n}" for t, n in sorted(adj.items())),
            "route_s": round(route_s, 3), "convert_s": round(conv_s, 3), "tracks": st["tracks"], "vias": st["vias"],
            "length_mm": round(st["length_mm"], 3)}


def same_run(out):
    """CSVs of the same run as `out`: same split, router and tag, any shard suffix."""
    stem = re.sub(r"-shard\d+r?$", "", out.stem)
    return [f for f in RESULTS.glob("*.csv") if re.sub(r"-shard\d+r?$", "", f.stem) == stem]


def main():
    global CAP_S
    ap = argparse.ArgumentParser()
    ap.add_argument("--split", required=True)
    ap.add_argument("--router", choices=sorted(ROUTERS), required=True)
    ap.add_argument("--seed", type=int, default=None, help="Freerouting --random_seed")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--boards", default="")
    ap.add_argument("--force", action="store_true")
    ap.add_argument("--tag", default="", help="suffix for the CSV name")
    ap.add_argument("--cap", type=float, default=CAP_S, help="per-board time cap in seconds (default 1800, as PCBWorld)")
    ap.add_argument("--reverse", action="store_true", help="walk the boards from the end (a second worker on a shard)")
    ap.add_argument("--shard", default="", help="i/n: only boards with index %% n == i (CSV gets -shard<i>)")
    a = ap.parse_args()
    CAP_S = a.cap
    splits = json.loads((HERE / "splits.json").read_text())
    names = splits[a.split]
    if a.boards:
        want = a.boards.split(",")
        names = [n for n in names if any(w in n for w in want)]
    if a.limit:
        names = names[: a.limit]
    if a.shard:
        i, n = map(int, a.shard.split("/"))
        names = [x for k, x in enumerate(names) if k % n == i]
        a.tag = (a.tag + "-" if a.tag else "") + f"shard{i}" + ("r" if a.reverse else "")
    RESULTS.mkdir(exist_ok=True)
    tag = f"-{a.tag}" if a.tag else ""
    out = RESULTS / f"{a.split}-{a.router}{tag}.csv"
    done = {}
    if out.exists() and not a.force:
        with open(out) as f:
            done = {r["board"]: r for r in csv.DictReader(f)}
    rows = dict(done)
    if not a.force and a.shard:
        # Boards finished by any other run of this split/router (another shard, or the
        # unsharded CSV) are skipped too.
        for f in same_run(out):
            if f != out:
                with open(f) as fh:
                    for r in csv.DictReader(fh):
                        done.setdefault(r["board"], r)
    order = list(enumerate(names))
    if a.reverse:
        order.reverse()
    for i, n in order:
        if n in done:
            continue
        if a.shard:
            # Another worker may have finished this board since we started.
            for f in same_run(out):
                if f != out and n in f.read_text():
                    break
            else:
                f = None
            if f is not None:
                continue
        if not (folder(n) / "processed_v9_guide_v3_unrouted.kicad_pcb").exists():
            print(f"[{i + 1}/{len(names)}] {n}: input missing, skipped", flush=True)
            continue
        r = run_board(n, a.router, a.seed)
        rows[n] = r
        print(f"[{i + 1}/{len(names)}] {n}: clean={r.get('clean')} unconn={r.get('unconnected', '')} "
              f"viol={r.get('violations', '')} {r.get('violation_types', '')} t={r.get('route_s', '')}s {r['status']}",
              flush=True)
        with open(out, "w", newline="") as f:
            w = csv.DictWriter(f, FIELDS)
            w.writeheader()
            for k in names:
                if k in rows:
                    w.writerow({fk: rows[k].get(fk, "") for fk in FIELDS})
    clean = sum(int(rows[n]["clean"]) for n in names if n in rows)
    print(f"{a.split} {a.router}: clean {clean}/{len(names)}")


if __name__ == "__main__":
    sys.exit(main())
