#!/usr/bin/env bash
# Rebuild PCBWorld's D3 benchmark boards from a PCBench clone, natively (no Docker).
#
#   PCBENCH=~/src/PCBench PCBWORLD=~/src/pcbworld D3_ROOT=~/d3 bench/prepare.sh
#
# Runs PCBWorld's own BSD-3 prep chain (tools/datagen/pcbench_prep): KiCad 5 -> 9
# conversion, rule patch + DRC filter, per-net guide widths, routing stripped. Board files
# are never committed to this repo. Needs KiCad 9 (kicad-cli + a Python that imports
# pcbnew). Takes 1-2 hours on a laptop with 2 workers; reruns skip finished boards.
set -euo pipefail
: "${PCBENCH:?clone https://github.com/PCBench/PCBench and set PCBENCH}"
: "${PCBWORLD:?clone the PCBWorld repo and set PCBWORLD}"
: "${D3_ROOT:?set D3_ROOT to an output directory}"
KICAD_APP=/Applications/KiCad9/KiCad.app/Contents
export KICAD_CLI="${KICAD_CLI:-$KICAD_APP/MacOS/kicad-cli}"
export PCBNEW_PYTHON="${KICAD_PYTHON:-$KICAD_APP/Frameworks/Python.framework/Versions/Current/bin/python3}"
WORKERS="${WORKERS:-2}"
WORK="$(dirname "$D3_ROOT")/pcbench_work"
export PYTHONPATH="$PCBWORLD"
export PCBWORLD_DATA_ROOT="$WORK"
export PCBENCH_PCBS_ROOT="$PCBENCH/PCBs"
export PCBENCH_V9_ROOT="$WORK/v9"
export PCBENCH_NEWDRC_OUT="$D3_ROOT"
export PCBENCH_SORTED_OUT="$WORK/sorted"
PP="$PCBWORLD/tools/datagen/pcbench_prep"
cd "$PCBWORLD"
[ -f "$PCBENCH_V9_ROOT/.converted" ] || { python3 "$PP/convert_v9.py" --workers "$WORKERS"; touch "$PCBENCH_V9_ROOT/.converted"; }
python3 "$PP/drc_fix_v9.py" --workers "$WORKERS"
python3 "$PP/make_guide.py" --base-dir "$D3_ROOT" --stem processed_v9 --suffix _guide_v3 --workers "$WORKERS"
echo "D3 boards ready under $D3_ROOT (bench/run.py reads D3_ROOT)"
