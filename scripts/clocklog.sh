#!/bin/bash
# Log the Pi-minus-laptop clock offset every 5 s, stamped with the laptop's unix time, until
# $RUNS/clocklog.stop exists. moq-lora-report --clock-log uses it to re-base latencies.
source "$(dirname "$0")/env.sh"
rm -f "$RUNS/clocklog.stop"
while [ ! -f "$RUNS/clocklog.stop" ]; do
  T=$(python3 -c 'import time; print(int(time.time() * 1e6))')
  C=$("$BIN/moq-lora-recv" --clock-check "$PI" 2>/dev/null)
  [ -n "$C" ] && echo "{\"t_us\":$T,\"c\":$C}" >> "$RUNS/clock_log.jsonl"
  sleep 5
done
