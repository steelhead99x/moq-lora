#!/bin/bash
# laptop -> CatWAN TX -> Pi SX1276 RX.  usage: [RX_KEY=path] [RX="extra recv args"] rev.sh NAME PROFILE SECS [sender args...]
# See scripts/README.md.
set -u
N=$1
P=$2
S=$3
shift 3
source "$(dirname "$0")/env.sh"
clock=$("$BIN/moq-lora-recv" --clock-check "$PI") || { echo "clock check failed" >&2; exit 1; }
X=$(printf '%s\n' "$clock" | sed 's/.*remote_minus_local_ms"://; s/}//')
printf '%s\n' "$X" | grep -Eq '^-?[0-9]+(\.[0-9]+)?$' || { echo "clock offset is not a number: $X" >&2; exit 1; }
echo "$X" > "$RUNS/$N.clock_start"
OFF=$(python3 -c 'import sys; print(-float(sys.argv[1]))' "$X") # capture clock (laptop) minus receive clock (Pi)
# shellcheck disable=SC2086
ssh "$PI" "cd ~/$(printf '%q' "$PI_DIR") && rm -rf $(printf '%q' "/tmp/$N") && ./moq-lora-recv --radio --profile $(printf '%q' "$P") --secs $((S + 10)) --out $(printf '%q' "/tmp/$N") --decode --raw-log --clock-offset-ms=$(printf '%q' "$OFF") ${RX_KEY:+--key-file $(printf '%q' "$RX_KEY")} ${RX:-}" > "$RUNS/$N.out" 2>&1 &
sleep 4
"$BIN/moq-lora-send" --catwan "$PORT" --profile "$P" --secs "$S" --log "$RUNS/$N.send.jsonl" --i-have-an-antenna "$@" > "$RUNS/$N.send" 2> "$RUNS/$N.send.err"
wait
mkdir -p "$RUNS/$N" && scp -q "$PI:/tmp/$N/*" "$RUNS/$N/" && ssh "$PI" rm -rf "$(printf '%q' "/tmp/$N")"
mv "$RUNS/$N.send.jsonl" "$RUNS/$N/send.jsonl"
tail -1 "$RUNS/$N.send" > "$RUNS/$N/send_summary.json"
"$BIN/moq-lora-recv" --clock-check "$PI" > "$RUNS/$N.clock_end"
echo "DONE $N"
