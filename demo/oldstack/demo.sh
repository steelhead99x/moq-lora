#!/bin/sh
# A quick (about a minute) run of both paths at 5% radio loss, then a side-by-side table.
# Needs the same things as run.sh (Rust, Python 3.10+, mosquitto or MOSQUITTO=/path/to/mosquitto).
# It writes to a temp folder and leaves results/ alone.
set -eu
cd "$(dirname "$0")"
OUT="${TMPDIR:-/tmp}/oldstack-demo.$$"
cargo build --release -q -p moq-lora-oldstack-demo
[ -x .venv/bin/python ] || python3 -m venv --system-site-packages .venv
.venv/bin/pip install -q -r requirements.txt
.venv/bin/python -m oldstack.bench --quick --loss 0.05 --runs "$OUT/runs" --results "$OUT/results" >/dev/null
.venv/bin/python viz/side_by_side.py "$OUT/results/results.json"
rm -rf "$OUT"
