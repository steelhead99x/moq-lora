# [XS] Receiver writes valid JSON when a run has no latency samples

## Goal

`summary.json` is valid JSON for every run. A run without `--decode`, or with no objects, writes
`null` for the latency percentiles and `rssi_mean` it cannot compute, and `moq-lora-report` reads it.

## Plan

Found in the second review: a NaN printed with `{:.1}` is not JSON. Repro: `--loopback` into
`--replay` without `--decode`, then parse `summary.json` with NaN rejected; the report fails on it.
