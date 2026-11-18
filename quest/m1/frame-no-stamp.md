# [XS] An access unit with no capture record keeps an honest frame number

## Goal

An extra access unit from the encoder is logged as such instead of being stamped `frame_no = u32::MAX`.
