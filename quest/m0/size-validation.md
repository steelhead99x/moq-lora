# [S] Both tools refuse picture sizes they cannot handle

## Goal

The sender refuses a size too short for its clock overlay above the barcode (640x64 panicked) and
the receiver refuses one too small for the barcode (0x0 with `--decode` looped forever), each with
a clear error. `stamp::draw_text` clips to the plane.

## Plan

The overlay is two text rows at scale `w/80` from row 2, so it needs `2 + 15 * max(w/80, 1)`
rows above the 16-row barcode. `draw_text` takes the plane height.
