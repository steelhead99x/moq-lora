# [XS] Interrupted reads and backward clock steps don't break a run

## Goal

`read_line_limited` retries `Interrupted` like std's `read_line`, so a signal doesn't end a
serial reader, and stall counting saturates when the wall clock steps back.

## Plan

Both `stalls()` copies (receiver and report) use `saturating_sub`.
