#!/usr/bin/env bash
# Route the bundled example boards and score them with KiCad 9's DRC (needs kicad-cli).
set -euo pipefail
KICAD_CLI="${KICAD_CLI:-/Applications/KiCad9/KiCad.app/Contents/MacOS/kicad-cli}"
T="$(mktemp -d)"; trap 'rm -rf "$T"' EXIT
cargo build --release -q
for b in mcu48 qfn24; do
  ./target/release/copperline route "examples/$b.kicad_pcb" -o "$T/$b.kicad_pcb"
  cp "examples/$b.kicad_pro" "$T/$b.kicad_pro"
  "$KICAD_CLI" pcb drc --format json --severity-error --all-track-errors -o "$T/$b.json" "$T/$b.kicad_pcb" >/dev/null
  python3 -c "import json,sys; d=json.load(open(sys.argv[1])); print(f'{sys.argv[2]}: KiCad DRC {len(d[\"violations\"])} violations, {len(d[\"unconnected_items\"])} unconnected')" "$T/$b.json" "$b"
done
