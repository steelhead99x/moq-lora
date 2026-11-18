# [XS] Report keeps latency on reverse runs when the Pi clock is behind

## Goal

`moq-lora-report --clock-log` re-bases every latency sample of a laptop-to-Pi run, whichever
clock is ahead.

## Plan

`rebase` cast a negative shift to `u64`, so with a positive `clock_offset_ms` every lookup fell
outside the log and the row read `NaN / NaN (clock log gap)`. Do the shift in signed arithmetic.
The shipped rows must regenerate identically (the Pi was ahead in all of them).
