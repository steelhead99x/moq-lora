# [XS] Each stick reports its own USB serial number

## Goal

A flashed stick enumerates with a serial derived from its flash chip's unique id, not the one
board's `613863E6354E26A3` that is hard-coded in the image.
