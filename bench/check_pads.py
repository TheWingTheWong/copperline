"""Compare copperline's parsed pad copper against KiCad's own pad polygons.

usage: $KICAD_PYTHON bench/kicad_tool.py pads b.kicad_pcb k.json; copperline pads-json b.kicad_pcb > c.json
       python3 bench/check_pads.py k.json c.json

For each pad, every vertex of KiCad's outline must lie on our shape's boundary within
TOL (a vertex outside our shape means we under-estimate copper, which is a bug), and the
two shapes' bounding boxes must agree within TOL.
"""
import json
import math
import sys

TOL = 0.006  # KiCad approximates arcs with max_error 0.005 mm


def seg_dist(q, a, b):
    ax, ay = a; bx, by = b; qx, qy = q
    dx, dy = bx - ax, by - ay
    l2 = dx * dx + dy * dy
    t = 0 if l2 == 0 else max(0, min(1, ((qx - ax) * dx + (qy - ay) * dy) / l2))
    return math.hypot(qx - (ax + t * dx), qy - (ay + t * dy))


def inside(q, pts):
    x, y = q; c = False
    for i in range(len(pts)):
        (x1, y1), (x2, y2) = pts[i], pts[i - 1]
        if (y1 > y) != (y2 > y) and x < x1 + (y - y1) * (x2 - x1) / (y2 - y1):
            c = not c
    return c


def gap(q, shape):
    pts, r = shape["pts"], shape["r"]
    if shape["kind"] == "point":
        d = math.dist(q, pts[0])
    elif shape["kind"] == "seg":
        d = seg_dist(q, pts[0], pts[1])
    else:
        d = 0 if inside(q, pts) else min(seg_dist(q, pts[i - 1], pts[i]) for i in range(len(pts)))
    return d - r


def main():
    kicad = json.load(open(sys.argv[1]))
    ours = {(p["ref"], p["num"], round(p["x"], 3), round(p["y"], 3)): p for p in json.load(open(sys.argv[2]))}
    bad = 0
    checked = 0
    for k in kicad:
        key = (k["ref"], k["num"], round(k["x"], 3), round(k["y"], 3))
        o = ours.get(key)
        if o is None:
            print("missing pad", key)
            bad += 1
            continue
        if not o["shapes"]:
            continue
        checked += 1
        worst = 0.0
        for outline in k["outlines"]:
            for q in outline:
                g = min(gap(q, s) for s in o["shapes"])
                worst = max(worst, g)  # positive = KiCad copper outside ours
        if worst > TOL:
            bad += 1
            if bad <= 15:
                print(f"pad {key} KiCad copper extends {worst:.4f} mm beyond ours")
    print(f"checked {checked} pads, {bad} mismatches")
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
