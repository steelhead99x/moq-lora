#!/bin/bash
# Pi -> CatWAN/laptop.  usage: [RX="extra recv args"] run2.sh NAME PROFILE SECS [sender args...]
set -u; N=$1; P=$2; S=$3; shift 3
source "$(dirname "$0")/env.sh"
OFF=$("$BIN/moq-lora-recv" --clock-check "$PI" | sed 's/.*remote_minus_local_ms"://; s/}//')
echo "$OFF" > "$RUNS/$N.clock_start"
"$BIN/moq-lora-recv" --port "$PORT" --profile "$P" --secs $((S+10)) --out "$RUNS/$N" --play --decode --raw-log \
    --clock-offset-ms="$OFF" ${RX:-} > "$RUNS/$N.out" 2>&1 &
sleep 3
ssh "$PI" "cd ~/$PI_DIR && ./moq-lora-send --profile $P --secs $S --log /tmp/${N}_send.jsonl --i-have-an-antenna $* 2>&1 | tail -1" > "$RUNS/$N.send"
wait
ssh "$PI" "cat /tmp/${N}_send.jsonl; rm -f /tmp/${N}_send.jsonl" > "$RUNS/$N/send.jsonl"
tail -1 "$RUNS/$N.send" > "$RUNS/$N/send_summary.json"
"$BIN/moq-lora-recv" --clock-check "$PI" > "$RUNS/$N.clock_end"
echo "DONE $N"
