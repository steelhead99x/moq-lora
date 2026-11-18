# [S] Receiver's decoder and player keep the first GOP

## Goal

The live decoder and ffplay output every frame the receiver hands them, from the first IDR on,
without giving up the per-frame latency the low-delay flags exist for.

## Plan

Both run with `-fflags nobuffer`, which discards the packets ffmpeg reads while it analyses the
stream. Found while counting dropped AUs: a 30 s loopback run delivered 455 objects and the
decoder output 440 (the first decoded frame came 917 ms after the first object). `rec.h264`
from the same run decodes to 455 frames with the receiver's other flags, 440 with them plus
`nobuffer`, and 395 with `nobuffer` alone (ffmpeg 6.1.1). Every published run's `decoded_frames`
is likely one GOP short for the same reason; stalls and glass-to-glass percentiles exclude the
start, so they should be unaffected, but check.

Measure before and after on a live loopback (sender `--loopback` into a FIFO the receiver
replays unpaced), comparing decoded count and g2g p50/p95.
