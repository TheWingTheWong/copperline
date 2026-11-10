"""Helpers that need KiCad's pcbnew module. Run with KiCad's own Python ($KICAD_PYTHON).

dsn <board.kicad_pcb> <out.dsn>               export a Specctra DSN (for Freerouting)
ses <board.kicad_pcb> <in.ses> <out.kicad_pcb>   import a Freerouting session and save
refill <board.kicad_pcb>                      refill copper zones in place
stats <board.kicad_pcb>                       print track/via counts and track length as JSON
"""
import json
import sys

import pcbnew


def dsn(src, out):
    board = pcbnew.LoadBoard(src)
    if not pcbnew.ExportSpecctraDSN(board, out):
        sys.exit("DSN export failed")


def ses(src, ses_path, dst):
    board = pcbnew.LoadBoard(src)
    if not pcbnew.ImportSpecctraSES(board, ses_path):
        sys.exit("SES import failed")
    board.Save(dst)


def refill(src):
    """Refill copper zones in place (kicad-cli 9 has no --refill-zones); no-op without zones."""
    board = pcbnew.LoadBoard(src)
    zones = [z for z in board.Zones() if not z.GetIsRuleArea()]
    if zones:
        pcbnew.ZONE_FILLER(board).Fill(board.Zones())
        board.Save(src)
    print(len(zones))


def stats(src):
    board = pcbnew.LoadBoard(src)
    tracks = vias = 0
    length = 0.0
    for t in board.GetTracks():
        if t.GetClass() == "PCB_VIA":
            vias += 1
        else:
            tracks += 1
            length += t.GetLength() / 1e6
    print(json.dumps({"tracks": tracks, "vias": vias, "length_mm": round(length, 3)}))


if __name__ == "__main__":
    cmd, *args = sys.argv[1:]
    {"dsn": dsn, "ses": ses, "refill": refill, "stats": stats}[cmd](*args)
