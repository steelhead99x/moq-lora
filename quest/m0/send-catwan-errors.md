# [S] Sender counts the CatWAN's TX errors

## Goal

In `--catwan` mode, every `ERR tx` and `ERR txtimeout` line from the stick counts in
`tx_errors`, so the summary shows it and more than 20 trigger the `tx_fatal` stop.

## Plan

The stick reports errors asynchronously, after the packet was queued; count them in the reader
thread and fold them into the TX loop's count before each send and once after the final drain.
