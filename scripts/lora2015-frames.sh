#!/usr/bin/env bash
# Rebuild the test frames of results/lora-2015/ from the testsrc2 frame in media/.
# Needs ffmpeg and, for the SSDV files, the `ssdv` tool (codeberg.org/fsphil/ssdv, make).
#   320x240 q10, 160x120 q20: ffmpeg MJPEG quality scale (lower is better)
#   SSDV wants width and height in multiples of 16, so its small image is 160x112
set -euo pipefail
cd "$(dirname "$0")/.."
src=media/frame-f350-rind-120s.png
out=results/lora-2015/frames
mkdir -p "$out"
# -force_duplicated_matrix writes the quantisation table twice, which ssdv needs
enc() { ffmpeg -v error -y -i "$src" -vf "$1" -pix_fmt yuvj420p -q:v "$2" -force_duplicated_matrix 1 -frames:v 1 "$3"; }
enc scale=320:240 10 "$out/f320x240-q10.jpg"
enc scale=160:120 20 "$out/f160x120-q20.jpg"
enc scale=160:120,crop=160:112:0:0 20 "$out/f160x112-q20.jpg"
if command -v ssdv >/dev/null; then
  ssdv -e -n -c TEST01 -i 1 "$out/f320x240-q10.jpg" "$out/f320x240-q10.ssdv" 2>/dev/null
  ssdv -e -n -c TEST01 -i 2 "$out/f160x112-q20.jpg" "$out/f160x112-q20.ssdv" 2>/dev/null
else
  echo "ssdv not on PATH: skipped the .ssdv files" >&2
fi
ls -l "$out"
