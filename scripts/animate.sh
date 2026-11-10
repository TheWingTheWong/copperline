#!/usr/bin/env bash
# Route a board with frame capture and turn the frames into a GIF (needs ffmpeg).
#   scripts/animate.sh examples/mcu48.kicad_pcb assets/routing.gif [width]
set -euo pipefail
IN="$1"; OUT="$2"; W="${3:-720}"
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
cargo run --release -q -- route "$IN" -o "$TMP/routed.kicad_pcb" --frames "$TMP/f" --frame-width "$W"
N=$(ls "$TMP/f" | wc -l)
# ~6 s of animation, then hold the routed board for 2.5 s.
FPS=$(( N / 6 > 4 ? N / 6 : 4 ))
ffmpeg -hide_banner -loglevel error -y -framerate "$FPS" -i "$TMP/f/frame_%04d.ppm" \
  -vf "tpad=stop_mode=clone:stop_duration=2.5,split[a][b];[a]palettegen=max_colors=48:stats_mode=diff[p];[b][p]paletteuse=dither=none" "$OUT"
echo "$N frames -> $OUT"
