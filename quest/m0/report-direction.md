# [XS] Report takes run direction from the sender's link field

## Goal

A run is labelled laptop-to-Pi exactly when its sender ran with `--catwan`, so a low-CPU forward
run (such as `--encoder hw`) gets the Pi to laptop label and the sender's CPU.

## Plan

Use `send_summary.json`'s `link`; keep the CPU guess only for summaries without it, which is
every run in `results/video` (they predate the field).
