#!/usr/bin/env bash
# The calculated airtime table of results/lora-2015/README.md, from the sender's --dry-run.
# Calculated from the SX1276 datasheet formula, not measured. Needs a release build.
set -euo pipefail
cd "$(dirname "$0")/.."
bin=target/release/lora2015-send
d=results/lora-2015/frames
echo "| image | mode | setting | frames | bytes on air | airtime per image (s) | best fps | images per 30 s |"
echo "|---|---|---|---|---|---|---|---|"
row() { # label mode file profile
  s=$($bin --mode "$2" --file "$3" --profile "$4" --dry-run | sed -n 's/^summary //p')
  frames=$(sed -n 's/.*frames=\([0-9]*\).*/\1/p' <<<"$s")
  bytes=$(sed -n 's/.*air_bytes=\([0-9]*\).*/\1/p' <<<"$s")
  toa=$(sed -n 's/.*toa_s=\([0-9.]*\).*/\1/p' <<<"$s")
  awk -v l="$1" -v m="$2" -v p="$4" -v f="$frames" -v b="$bytes" -v t="$toa" \
    'BEGIN { printf "| %s | %s | %s | %d | %d | %.2f | %.3f | %d |\n", l, m, p, f, b, t, 1/t, int(30/t) }'
}
for p in sf7-125 sf7-250 sf7-500 sf8-500; do
  row "320x240 q10 JPEG" chunk "$d/f320x240-q10.jpg" $p
  row "320x240 q10 JPEG" lorawan "$d/f320x240-q10.jpg" $p
  row "320x240 q10 SSDV" ssdv "$d/f320x240-q10.ssdv" $p
  row "160x120 q20 JPEG" chunk "$d/f160x120-q20.jpg" $p
  row "160x120 q20 JPEG" lorawan "$d/f160x120-q20.jpg" $p
  row "160x112 q20 SSDV" ssdv "$d/f160x112-q20.ssdv" $p
done
