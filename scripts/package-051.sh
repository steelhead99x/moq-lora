#!/bin/bash
# Copy the data worth keeping from a bench-051.sh runs directory into results/stick-0.5.1/ (no logs that hold host paths).
# usage: package-051.sh RUNS_DIR DEMO_DIR OUT_DIR
set -eu
R=$1; DEMO=$2; O=$3; mkdir -p "$O/fsk" "$O/lora" "$O/demo"
for d in "$R"/f300-*/; do
  n=$(basename "$d"); [ -s "$d/summary.json" ] || continue
  grep -q '"radio_frames_ok":0,' "$d/summary.json" && continue
  mkdir -p "$O/fsk/$n"
  cp "$d/summary.json" "$d/send_summary.json" "$O/fsk/$n/"
  for f in objects.jsonl decoded.jsonl send.jsonl serial.log; do gzip -9c "$d/$f" > "$O/fsk/$n/$f.gz"; done
done
for d in "$R"/*-r[0-9]/; do
  n=$(basename "$d"); mkdir -p "$O/lora/$n"
  cp "$d/exact/images.csv" "$d/exact/rx.csv" "$O/lora/$n/"
  cp "$d"/exact/image*.jpg "$d"/exact/image*.ssdv "$O/lora/$n/" 2>/dev/null || true
  gzip -9c "$d/raw.log" > "$O/lora/$n/raw.log.gz"
  cp "$R/$n.tx.csv" "$O/lora/$n/tx.csv"; grep 'sent frames' "$R/$n.send" > "$O/lora/$n/send.txt"
done
# demo run
cp "$DEMO/fsk-live.mp4" "$DEMO/fsk-live-still.png" "$O/demo/"
cp "$DEMO/fsk-live/summary.json" "$O/demo/fsk-live-summary.json"
for d in "$DEMO"/demo-*/; do n=$(basename "$d"); mkdir -p "$O/demo/$n"; cp "$d/exact/images.csv" "$O/demo/$n/"; cp "$d"/exact/image*.jpg "$O/demo/$n/" 2>/dev/null || true; done
