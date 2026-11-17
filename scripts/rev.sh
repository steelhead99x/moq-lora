#!/bin/bash
# laptop -> CatWAN TX -> Pi SX1276 RX.  usage: [RX="extra recv args"] rev.sh NAME PROFILE SECS [sender args...]
set -u; N=$1; P=$2; S=$3; shift 3
source "$(dirname "$0")/env.sh"
X=$("$BIN/moq-lora-recv" --clock-check "$PI" | sed 's/.*remote_minus_local_ms"://; s/}//')
echo "$X" > "$RUNS/$N.clock_start"
OFF=$(python3 -c "print(-($X))")   # capture clock (laptop) minus receive clock (Pi)
ssh "$PI" "cd ~/$PI_DIR && rm -rf /tmp/$N && ./moq-lora-recv --radio --profile $P --secs $((S+10)) --out /tmp/$N --decode --raw-log --clock-offset-ms=$OFF ${RX:-}" > "$RUNS/$N.out" 2>&1 &
sleep 4
"$BIN/moq-lora-send" --catwan "$PORT" --profile "$P" --secs "$S" --log "$RUNS/$N.send.jsonl" --i-have-an-antenna "$@" > "$RUNS/$N.send" 2> "$RUNS/$N.send.err"
wait
mkdir -p "$RUNS/$N" && scp -q "$PI:/tmp/$N/*" "$RUNS/$N/" && ssh "$PI" rm -rf "/tmp/$N"
mv "$RUNS/$N.send.jsonl" "$RUNS/$N/send.jsonl"
tail -1 "$RUNS/$N.send" > "$RUNS/$N/send_summary.json"
"$BIN/moq-lora-recv" --clock-check "$PI" > "$RUNS/$N.clock_end"
echo "DONE $N"
