# [S] Receiver's expected-frame count can't be inflated by one object

## Goal

`frames_expected` and `video_frame_loss` stay meaningful when one object carries a wrong or
repeated frame number: a garbled or injected `frame_no` cannot make a run report 2^32 expected
frames, and loss is never negative.

## Plan

The receiver computes `last_frame_no - first_frame_no + 1` from the last and first objects it
decoded, and `frame_no` is unauthenticated without `--key-file`. Found while fixing the sender's
`u32::MAX` stamp: replaying 30 objects plus one stamped `u32::MAX` reported `frames_expected`
4294967296 and `video_frame_loss` 1.0. With the sender now reusing the last frame number for such
an AU, the same replay delivers 31 objects against 30 expected and reports a loss of -0.033, so
loss must count distinct frame numbers delivered, not objects. Count expected frames from frame numbers that advance
plausibly from the previous one (bounded by the elapsed time and fps), and count the rest as
rejected.
