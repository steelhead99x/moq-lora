# Roadmap and known issues

Measured on 2026-11-17: eight 330 s runs, both directions, 300 kbit/s and 350 kbit/s,
rind off and on. Numbers are in [results/video/table.md](results/video/table.md). The
code review, with what was fixed and what was left alone, is [REVIEW.md](REVIEW.md).

## Before a public release

* Choose a license and replace [LICENSE](LICENSE). The `MIT OR Apache-2.0` field in
  Cargo.toml is not a grant until the texts are added and the owner confirms it.
* History still has the bench machine's ssh alias, a home-directory key path, and the
  CatWAN USB serial string. It was not rewritten. See REVIEW.md.

## Known issues

* The Pi HAT has no DIO1, so frames longer than the 64-byte FIFO are drained by polling.
  Receive on the Pi costs about a full core. A CRC failure mid-frame can splice two frames
  in `fsk_poll_long`; the end-to-end CRC-16 drops the result, it does not prevent it.
* The CatWAN firmware accepts 2..=17 dBm and does not check the carrier frequency. The
  host tools refuse above +5 dBm and anything outside 902-928 MHz (including 925.0). The
  prebuilt `firmware/catwan-fw-v5.uf2` matches the current firmware source. Tightening
  the firmware means a new image.
* 350 kbit/s FSK is outside the SX1276 datasheet's characterized settings. The profile
  flag `:x` is the acknowledgement. It was used for the `f350_*` and `r350_*` runs.
* FCC Part 15.247: the 6 dB bandwidth of this 300 kbit/s, ±100 kHz FSK signal has not
  been measured. The link does not frequency-hop. You are responsible for local rules.
  915.0 MHz is in the US 902-928 MHz band; it is not an ISM band everywhere.
* The ffmpeg CLI decoder holds one frame (~67 ms at 15 fps). That hold is inside the
  glass-to-glass numbers.
* Encryption is a pre-shared key. There is no key exchange, no replay window, and the
  radio-frame headers (sequence, group, object, sizes) are not inside the AEAD. A
  (key, nonce) pair is unique within one run unless two session ids collide (32-bit
  random, redrawn when the group counter wraps). Rotate the key long before 2^16 runs.
* Single fixed channel, no rate adaptation. The encoder bitrate is set by hand.
* Tested with ffmpeg's test pattern, not a camera.
