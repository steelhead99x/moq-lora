# [S] One copy of the Pi radio glue

## Goal

`moq-lora-send/src/radio.rs` and `moq-lora-recv/src/radio.rs` share `CdevIrq` and `Radio::open`
from one place, without changing the TX path's behavior.
