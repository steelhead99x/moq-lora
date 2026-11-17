# moq-lora

Live video over a 2016-era Semtech **SX1276** radio (the "LoRa chip"), on old hardware:
a **Raspberry Pi 2** with a Dragino LoRa HAT transmits, a **CatWAN RP2040 USB stick**
(RFM95W = SX1276) plugged into a Mac receives. Everything between ffmpeg and the radio
is Rust: the SX1276 driver, the RP2040 firmware, the MoQ-style transport, the sender and
the receiver.

```
Pi 2 (armv7, 900 MHz)                                  Mac
ffmpeg testsrc2 ─raw─▶ moq-lora-send ─FSK 300 kbit/s─▶ CatWAN RP2040 ─USB─▶ moq-lora-recv ─▶ ffplay (live)
  (-re 15 fps)          • burn clock + barcode           915.0 MHz, +5 dBm   (sx1276-rs fw)    • reassembly + FEC   ─▶ rec.h264 / .mp4
                        • ffmpeg H.264 (x264 or VideoCore)                                       • MoQ group gate     ─▶ ffmpeg decode ─▶ barcode latency
                        • 1 object per frame, 1 group per GOP
                        • fragments + XOR parity, stale-group drop
```

## Crates

| crate | runs on | what |
|---|---|---|
| `moq-lora-wire` | anywhere | MoQ-style track/group/object framing in <= 255 B radio frames (10 B header), XOR-parity FEC (1 repair frame per k), reassembler, MoQ group gate, H.264 Annex-B access-unit splitter, in-picture barcode + clock |
| `moq-lora-send` | Pi 2 (armv7 musl, static) | spawns ffmpeg source + encoder, stamps capture time, objects -> frames -> SX1276 FSK; `--loopback FILE` for testing without RF |
| `moq-lora-recv` | Mac (any Unix) | CatWAN serial -> frames -> objects -> ffplay + recording + ffmpeg decode, measures loss / latency / stalls; `--replay FILE` |
| `vendor/sx1276` | — | vendored no_std embedded-hal 1.0 SX1276 driver (from Kyle's sx1276-rs) |

The CatWAN firmware is the sx1276-rs `catwan-fw` (USB CDC line protocol: `F` configure FSK,
`R` receive -> `R <t_us> <rssi> <snr> <hex>` lines).

## Wire format (MoQ mapping)

* **track** = the video stream; **group** = one GOP (starts with an IDR keyframe, SPS/PPS in band);
  **object** = one encoded frame (H.264 access unit, AUD-delimited) with a 17 B header
  (frame number, capture time in unix µs, encoder delay, key flag).
* Radio frame: `seq u16 | group u16 | obj u8 | idx u8 | nfrag u8 | flags u8 (parity, version, k) | objlen u16 | crc16 u16 | <=243 B` (equal-size chunks, padded to `--min-frame`).
* FEC: with `--fec-k K` a parity frame (XOR of the K data fragments, padded) follows each block,
  repairing any single lost fragment per block.
* Sender: objects older than `--max-age-ms` before their first fragment goes out are dropped
  together with the rest of their group (MoQ: newest group wins under congestion).
* Receiver: an unrecoverable object (or an object-id gap) cuts the group; delivery resumes at the
  next group's keyframe, so the decoder never sees a broken reference chain.

## Build

```sh
# Pi sender (cross from Linux; rust-lld, static musl)
cargo build --release -p moq-lora-send --target armv7-unknown-linux-musleabihf
scp target/armv7-unknown-linux-musleabihf/release/moq-lora-send pi2b:moq-lora/
# Mac receiver
cargo build --release -p moq-lora-recv
cargo test -p moq-lora-wire
```

## Run

See `results/video/README.md` for the exact commands and numbers of each run.

Rules this project keeps: transmit only on 915.0 MHz (902-928 MHz band), <= +5 dBm (the sender
refuses more), never 925.0 MHz; `--i-have-an-antenna` is required to transmit.
