# [M] Stick keeps USB serviced during a long transmit

## Goal

`fsk_transmit_long` no longer blocks the main loop for a whole 255-byte packet, so USB is polled
while the FIFO is refilled.

## Plan

Changes how the reverse direction was measured; needs the v6 re-run.
