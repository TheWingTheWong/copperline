"""Classify every KiCad DRC violation in the routed benchmark boards.

usage: python3 bench/classify.py

Freerouting receives the board through KiCad's Specctra DSN export, which carries no copper
text and no footprint-level clearance overrides, and its SES comes back with widths rounded
to the DSN resolution. Violations caused by that conversion are not routing mistakes the
router could have avoided with the input it was given. Classes:

  text       an item of the violation is copper text ("PCB text ...")
  footprint  the violated rule is a footprint clearance override ("(footprint X clearance")
  rounding   track_width within 0.01 mm of the minimum
  routing    anything else

Writes bench/results/violations.csv (board, router, type, class). The baseline violations
already present on the unrouted input are not subtracted here; report.py only uses this file
for boards whose subtracted count is non-zero.
"""
import csv
import json
import re
from pathlib import Path

HERE = Path(__file__).resolve().parent


def classify(v):
    items = " ".join(i.get("description", "") for i in v.get("items", []))
    if "PCB text" in items or re.search(r"\btext '", items):
        return "text"
    if "(footprint " in v.get("description", ""):
        return "footprint"
    if v["type"] == "track_width":
        m = re.search(r"min width ([\d.]+) mm; actual ([\d.]+) mm", v["description"])
        if m and abs(float(m.group(1)) - float(m.group(2))) < 0.01:
            return "rounding"
    return "routing"


def main():
    rows = []
    for router in ["copperline", "freerouting"]:
        for f in sorted((HERE / "work" / router).glob("*/routed.drc.json")):
            board = f.parent.name
            for v in json.loads(f.read_text()).get("violations", []):
                rows.append({"board": board, "router": router, "type": v["type"], "class": classify(v)})
    out = HERE / "results" / "violations.csv"
    with open(out, "w", newline="") as fh:
        w = csv.DictWriter(fh, ["board", "router", "type", "class"])
        w.writeheader()
        w.writerows(rows)
    from collections import Counter
    print(Counter((r["router"], r["class"]) for r in rows))


if __name__ == "__main__":
    main()
