# [S] One backward sequence number can't skew loss accounting

## Goal

A single old or hostile sequence number does not make the next honest frame look like a large gap,
and a frame counted bad is not also counted ok.

## Plan

`Reassembler::push` updates `last_seq` for any delta and counts `frames_ok` before the header
consistency check. Keep accepting a late fragment that completes an object.
