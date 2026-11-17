"""Generate examples/mcu48.kicad_pcb + .kicad_pro: a synthetic two-layer MCU breakout
(LQFP-48 at 0.5 mm pitch, four 2.54 mm headers, decoupling caps). Written for copperline's
demo and tests; no third-party design data.

usage: python3 examples/gen_mcu48.py
"""
import json
import random
import uuid
from pathlib import Path

HERE = Path(__file__).resolve().parent
rng = random.Random(48)
U = lambda: str(uuid.UUID(int=rng.getrandbits(128), version=4))

X0, Y0, W, H = 100.0, 100.0, 52.0, 44.0
CX, CY = X0 + W / 2, Y0 + H / 2
nets = [""]


def net(name):
    if name not in nets:
        nets.append(name)
    return nets.index(name)


fps = []


def footprint(ref, x, y, pads):
    body = "".join(pads)
    fps.append(f'\t(footprint "copperline:{ref}"\n\t\t(layer "F.Cu")\n\t\t(uuid "{U()}")\n\t\t(at {x:.3f} {y:.3f})\n'
               f'\t\t(property "Reference" "{ref}"\n\t\t\t(at 0 0 0)\n\t\t\t(layer "F.Fab")\n\t\t\t(hide yes)\n\t\t\t(uuid "{U()}")\n\t\t)\n{body}\t)\n')


def smd(num, x, y, w, h, n):
    return (f'\t\t(pad "{num}" smd roundrect\n\t\t\t(at {x:.3f} {y:.3f})\n\t\t\t(size {w:.3f} {h:.3f})\n\t\t\t(layers "F.Cu" "F.Mask" "F.Paste")\n'
            f'\t\t\t(roundrect_rratio 0.25)\n\t\t\t(net {net(n)} "{n}")\n\t\t\t(uuid "{U()}")\n\t\t)\n')


def tht(num, x, y, n, square=False):
    shape = "rect" if square else "circle"
    return (f'\t\t(pad "{num}" thru_hole {shape}\n\t\t\t(at {x:.3f} {y:.3f})\n\t\t\t(size 1.7 1.7)\n\t\t\t(drill 1.0)\n\t\t\t(layers "*.Cu" "*.Mask")\n'
            f'\t\t\t(net {net(n)} "{n}")\n\t\t\t(uuid "{U()}")\n\t\t)\n')


# MCU pins counter-clockwise from pin 1 (left side, top). Power on fixed pins as on real parts.
power = {11: "GND", 12: "+3V3", 23: "GND", 24: "+3V3", 35: "GND", 36: "+3V3", 47: "GND", 48: "+3V3"}
mcu_pads = []
signals = []
for i in range(48):
    pin = i + 1
    side, k = divmod(i, 12)
    off = (k - 5.5) * 0.5
    if side == 0:
        x, y, w, h = -4.15, off, 1.5, 0.3
    elif side == 1:
        x, y, w, h = off, 4.15, 0.3, 1.5
    elif side == 2:
        x, y, w, h = 4.15, -off, 1.5, 0.3
    else:
        x, y, w, h = -off, -4.15, 0.3, 1.5
    name = power.get(pin) or f"P{pin}"
    if pin not in power:
        signals.append(name)
    mcu_pads.append(smd(pin, x, y, w, h, name))
footprint("U1", CX, CY, mcu_pads)

# Headers around the edge, counter-clockwise like the MCU, starting with GND, +3V3.
headers = [("J1", "v", X0 + 3.0, CY, 13), ("J2", "h", CX, Y0 + H - 3.0, 15),
           ("J3", "v", X0 + W - 3.0, CY, 13), ("J4", "h", CX, Y0 + 3.0, 15)]
slots = []
for ref, orient, hx, hy, n in headers:
    for k in range(n):
        off = (k - (n - 1) / 2) * 2.54
        if ref in ("J3", "J4"):
            off = -off
        slots.append((ref, k + 1, (0, off) if orient == "v" else (off, 0)))
# Rotate the mapping so the seam forces crossings, then swap a few pairs.
order = signals[7:] + signals[:7]
for _ in range(1):
    a, b = rng.sample(range(len(order)), 2)
    order[a], order[b] = order[b], order[a]
by_ref = {}
it = iter(order)
for ref, k, (dx, dy) in slots:
    name = "GND" if k == 1 else "+3V3" if k == 2 else next(it, None)
    if name is None:
        continue
    by_ref.setdefault(ref, []).append((k, dx, dy, name))
for ref, orient, hx, hy, n in headers:
    footprint(ref, hx, hy, [tht(k, dx, dy, name, square=(k == 1)) for k, dx, dy, name in by_ref[ref]])

# 0603 decoupling caps next to each power pin pair.
for i, (dx, dy, rot) in enumerate([(-7.0, 2.6, 0), (2.6, 7.0, 1), (7.0, -2.6, 0), (-2.6, -7.0, 1)]):
    if rot:
        pads = [smd(1, -0.8, 0, 0.9, 0.95, "+3V3"), smd(2, 0.8, 0, 0.9, 0.95, "GND")]
    else:
        pads = [smd(1, 0, -0.8, 0.95, 0.9, "+3V3"), smd(2, 0, 0.8, 0.95, 0.9, "GND")]
    footprint(f"C{i + 1}", CX + dx, CY + dy, pads)

out = ['(kicad_pcb\n\t(version 20241229)\n\t(generator "copperline-examples")\n\t(generator_version "1")\n\t(general\n\t\t(thickness 1.6)\n\t)\n\t(paper "A4")\n'
       '\t(layers\n\t\t(0 "F.Cu" signal)\n\t\t(2 "B.Cu" signal)\n\t\t(25 "Edge.Cuts" user)\n\t\t(1 "F.Mask" user)\n\t\t(3 "B.Mask" user)\n\t\t(13 "F.Paste" user)\n\t\t(5 "F.SilkS" user "F.Silkscreen")\n\t\t(39 "F.Fab" user)\n\t)\n'
       '\t(setup\n\t\t(pad_to_mask_clearance 0)\n\t)\n']
out += [f'\t(net {i} "{n}")\n' for i, n in enumerate(nets)]
out += fps
for (a, b), (c, d) in [((X0, Y0), (X0 + W, Y0)), ((X0 + W, Y0), (X0 + W, Y0 + H)), ((X0 + W, Y0 + H), (X0, Y0 + H)), ((X0, Y0 + H), (X0, Y0))]:
    out.append(f'\t(gr_line\n\t\t(start {a} {b})\n\t\t(end {c} {d})\n\t\t(stroke\n\t\t\t(width 0.1)\n\t\t\t(type default)\n\t\t)\n\t\t(layer "Edge.Cuts")\n\t\t(uuid "{U()}")\n\t)\n')
out.append(")\n")
(HERE / "mcu48.kicad_pcb").write_text("".join(out))
def netclass(name, clearance, track, via, drill, priority):
    return {"name": name, "clearance": clearance, "track_width": track, "via_diameter": via, "via_drill": drill,
            "microvia_diameter": 0.3, "microvia_drill": 0.1, "diff_pair_gap": 0.25, "diff_pair_via_gap": 0.25,
            "diff_pair_width": 0.2, "bus_width": 12, "wire_width": 6, "line_style": 0, "priority": priority,
            "pcb_color": "rgba(0, 0, 0, 0.000)", "schematic_color": "rgba(0, 0, 0, 0.000)"}


# The field set KiCad 9 writes; without net_settings.meta.version KiCad ignores the classes.
pro = {"board": {"design_settings": {"rules": {"min_clearance": 0.15, "min_track_width": 0.15, "min_copper_edge_clearance": 0.3,
                                                "min_hole_clearance": 0.25, "min_hole_to_hole": 0.25, "min_via_diameter": 0.5, "min_through_hole": 0.3}}},
       "meta": {"filename": "mcu48.kicad_pro", "version": 3},
       "net_settings": {"classes": [netclass("Default", 0.15, 0.2, 0.6, 0.3, 2147483647), netclass("Power", 0.15, 0.3, 0.7, 0.35, 0)],
                        "meta": {"version": 4}, "net_colors": None, "netclass_assignments": None,
                        "netclass_patterns": [{"netclass": "Power", "pattern": "GND"}, {"netclass": "Power", "pattern": "+3V3"}]}}
(HERE / "mcu48.kicad_pro").write_text(json.dumps(pro, indent=2) + "\n")
print(f"{len(nets) - 1} nets, {len(fps)} footprints")
