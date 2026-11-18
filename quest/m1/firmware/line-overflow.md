# [XS] Stick discards an overlong command line whole

## Goal

A serial line longer than the 600-byte buffer is dropped through its newline, so its tail is never
parsed as a new command.
