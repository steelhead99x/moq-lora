# [XS] Sender exits non-zero when the source or encoder fails

## Goal

A run whose source produced no frame, or whose encoder exited with an error, exits 1 after
printing its summary, which says `encoder_ok`.

## Plan

The source is killed on purpose at the end, so its status is meaningless; `captured == 0` is the
source check. The encoder exits on its own when its input closes, so its status is real.
