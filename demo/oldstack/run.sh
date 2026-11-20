#!/bin/sh
# One command: build the moq-lora side, set up the Python side, test both, benchmark both.
# Needs Rust 1.90+, Python 3.10+ and mosquitto (or MOSQUITTO=/path/to/mosquitto).
set -eu
cd "$(dirname "$0")"
cargo build --release -q -p moq-lora-oldstack-demo
[ -x .venv/bin/python ] || python3 -m venv --system-site-packages .venv
.venv/bin/pip install -q -r requirements.txt
cargo test -q -p moq-lora-oldstack-demo
.venv/bin/python -m pytest -q tests
.venv/bin/python -m oldstack.bench "$@"
