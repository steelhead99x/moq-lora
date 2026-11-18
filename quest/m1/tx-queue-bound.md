# [S] Sender's TX queue is bounded

## Goal

A stalled radio or a too-fast encoder cannot grow the sender's queue without bound: past a limit
derived from `--max-age-ms` and the radio rate, whole groups are dropped and counted.
