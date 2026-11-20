# Changelog

## Unreleased

Changes since the runs in `results/video/` were measured (2026-11-17). None of these has been
run on the radios yet, only through the no-radio loopback and the unit tests.

### Renamed

* **The project is now moq-subghz** (was moq-lora). The crates and tools are `moq-subghz-send`,
  `moq-subghz-recv`, `moq-subghz-wire`, `moq-subghz-pi` and `moq-subghz-report`. It never used
  LoRa modulation, and the framing isn't tied to one radio chip.

### Changed on the air

* **Frame format version 4.** Encrypted objects now carry a per-run salt, and each run encrypts
  under its own subkey derived from the pre-shared key. The first frame of each GOP carries 28
  bytes of overhead and the rest 20. Version 3 and version 4 tools refuse each other's frames.
* The key-derivation label was renamed with the project, so encrypted version 4 output from before
  the rename can't be opened. No such output was published.

### Receiver

* Replayed and injected radio frames no longer break the object in progress or re-open old
  groups. They are counted as `radio_frames_stale`. The first frame, and any large jump ahead,
  must be confirmed by the next frame.
* The expected-frame count ignores a garbled or injected frame number, so `video_frame_loss`
  can no longer go negative or count billions of frames.
* Loss accounting separates late, stray and stale frames.
* The receiver keeps every decoded frame, including the first GOP.

### Sender and tools

* The sender stops if the encoder sends more than 256 frames without a keyframe, and stops when
  its video source stalls.
* Key files must be readable only by their owner.
* The bench scripts quote key paths and pass the receiver's key separately.
* The Pi radio setup code is shared by the sender and receiver in a new `moq-subghz-pi` crate.

### Docs

* README and HOWTO rewritten for a public release. Added SECURITY, CONTRIBUTING and this file.

## Bench release, 2026-11-17

The first working version: live H.264 over SX1276 FSK from a Raspberry Pi 2 to a CatWAN USB
stick, with optional encryption, and the eight measured runs in `results/video/`.
