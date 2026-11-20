#!/bin/bash
# One lora2015 over-the-air run: logger on this host, sender on the Pi.
# usage: lora2015-run.sh NAME MODE FILE PROFILE PASSES [sender args...]
#   FILE is a path under results/lora-2015/frames/ (the same files are in ~/lora2015/frames on the Pi).
# Writes $RUNS/NAME/ (rx.csv, images.csv, raw.log, image*.jpg), NAME.tx.csv (the Pi's per frame log),
# NAME.send (the sender's last line) and NAME/exact/ (logger replayed with the real sent-frame count).
# See results/lora-2015/README.md and results/stick-0.5.1/README.md.
set -u
N=$1; MODE=$2; FILE=$3; PROF=$4; PASSES=$5; shift 5
source "$(dirname "$0")/env.sh"
ROOT=$(cd "$(dirname "$0")/.." && pwd)
F="$ROOT/results/lora-2015/frames/$FILE"
PASS_S=$("$BIN/lora2015-send" --mode "$MODE" --file "$F" --profile "$PROF" --dry-run "$@" | sed -n 's/.*pass_s=\([0-9.]*\).*/\1/p')
SECS=$(awk -v p="$PASS_S" -v n="$PASSES" 'BEGIN{printf "%d", n*(p+1)+14}')
REF=""; case "$FILE" in *.jpg) REF="--reference $F";; esac
mkdir -p "$RUNS/$N"
# shellcheck disable=SC2086
"$BIN/lora2015-log" --port "$PORT" --mode "$MODE" --profile "$PROF" --secs "$SECS" --out "$RUNS/$N" $REF > "$RUNS/$N.log.out" 2>&1 &
LP=$!
sleep 3
remote_args=''; [ $# -gt 0 ] && remote_args=$(printf '%q ' "$@")
ssh "$PI" "cd ~/lora2015 && ./lora2015-send --mode $MODE --file frames/$FILE --profile $PROF --power 5 --passes $PASSES --tx-log /tmp/$N.tx.csv --i-have-an-antenna ${remote_args}2>&1 | tail -n 3" > "$RUNS/$N.send"
wait $LP
scp -q "$PI:/tmp/$N.tx.csv" "$RUNS/$N.tx.csv"; ssh "$PI" "rm -f /tmp/$N.tx.csv"
SENT=$(sed -n 's/.*sent frames=\([0-9]*\).*/\1/p' "$RUNS/$N.send")
# shellcheck disable=SC2086
[ -n "$SENT" ] && "$BIN/lora2015-log" --replay "$RUNS/$N/raw.log" --mode "$MODE" --profile "$PROF" --sent-frames "$SENT" --out "$RUNS/$N/exact" $REF > "$RUNS/$N.exact.out" 2>&1
echo "DONE $N sent=$SENT $(grep -m1 'sent frames' "$RUNS/$N.send")"
