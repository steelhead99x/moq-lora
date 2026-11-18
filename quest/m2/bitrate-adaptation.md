# [L] Sender adapts bitrate to measured loss

## Goal

The sender lowers and raises the encoder bitrate from loss the receiver reports, instead of a
fixed `--bitrate`.

## Plan

Needs a return path, which the forward link lacks: decide between the reverse radio direction and
an out-of-band channel first.
