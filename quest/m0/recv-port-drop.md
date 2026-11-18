# [S] Receiver keeps its summary when the serial port drops

## Goal

When the CatWAN port closes mid-run, the receiver stops, writes `summary.json` with
`"port_closed":true`, and exits 1, instead of bailing with no stats.

## Plan

Skip the closing `S` command and its stats wait when the port is gone. Validate with a fake stick
on a pty that disappears mid-stream.
