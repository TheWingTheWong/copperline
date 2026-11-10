"""Runs inside the copperline-kicad container (KiCad 9 pcbnew bindings).

prep   <in.kicad_pcb> <out.kicad_pcb> [out.dsn]
    Load any KiCad board (v5..v9), delete every track, arc and via, delete copper
    fill zones (keep rule areas / keepouts), save in KiCad 9 format, optionally
    export a Specctra DSN for Freerouting.
ses    <board.kicad_pcb> <in.ses> <out.kicad_pcb>
    Import a Freerouting session file onto the board and save it.
"""
import sys
import pcbnew


def prep(src, dst, dsn=None):
    board = pcbnew.LoadBoard(src)
    for t in list(board.GetTracks()):
        board.Remove(t)
    for z in list(board.Zones()):
        if not z.GetIsRuleArea():
            board.Remove(z)
    board.Save(dst)
    if dsn:
        board = pcbnew.LoadBoard(dst)
        if not pcbnew.ExportSpecctraDSN(board, dsn):
            sys.exit("DSN export failed")


def ses(src, ses_path, dst):
    board = pcbnew.LoadBoard(src)
    if not pcbnew.ImportSpecctraSES(board, ses_path):
        sys.exit("SES import failed")
    board.Save(dst)


def pads(src, out_json):
    """Dump every pad's copper outline as KiCad computes it (for testing our parser)."""
    import json
    board = pcbnew.LoadBoard(src)
    rows = []
    for fp in board.GetFootprints():
        for pad in fp.Pads():
            layer = pcbnew.F_Cu if pad.IsOnLayer(pcbnew.F_Cu) else pcbnew.B_Cu
            if not pad.IsOnLayer(layer):
                continue
            poly = pad.GetEffectivePolygon(layer, pcbnew.ERROR_INSIDE)
            outlines = []
            for i in range(poly.OutlineCount()):
                o = poly.Outline(i)
                outlines.append([[o.CPoint(j).x / 1e6, o.CPoint(j).y / 1e6] for j in range(o.PointCount())])
            pos = pad.GetPosition()
            rows.append({"ref": fp.GetReference(), "num": pad.GetNumber(), "net": pad.GetNetCode(),
                         "x": pos.x / 1e6, "y": pos.y / 1e6, "outlines": outlines})
    json.dump(rows, open(out_json, "w"))


if __name__ == "__main__":
    cmd, *args = sys.argv[1:]
    {"prep": prep, "ses": ses, "pads": pads}[cmd](*args)
