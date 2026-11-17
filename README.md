# moq-lora

**Live H.264 video over a Semtech SX1276 radio, on a 2015 Raspberry Pi 2, written in Rust.**

The SX1276 is best known as "the LoRa chip". LoRa itself tops out around tens of kbit/s, but
the same silicon also has an FSK modem that goes to 300 kbit/s (350k if you leave the
datasheet's checked envelope). This project pushes a 320x240, 15 fps H.264 stream through
that modem, framed the way [Media over QUIC](https://datatracker.ietf.org/wg/moq/about/)
frames media: **one MoQ group per GOP, one object per frame**, so the receiver can always
throw a damaged group away and pick up at the next keyframe instead of smearing.

```
 Raspberry Pi 2 (armv7, 900 MHz)                                   laptop
 ffmpeg testsrc2 ─▶ moq-lora-send ──── FSK 300 kbit/s ────▶ CatWAN RP2040 ─USB─▶ moq-lora-recv ─▶ ffplay (live)
                    · burn clock + timestamp barcode   915.0 MHz, +5 dBm   (Rust firmware)    · reassembly + FEC     ─▶ rec.h264
                    · x264 320x240 15 fps ~180 kbit/s                                          · MoQ group gate       ─▶ ffmpeg decode
                    · rind AEAD per object (optional)                                          · rind open + verify      reads the barcode
                    · <=255 B frames, XOR parity, CRC-16                                                                  = glass-to-glass
```

Everything between ffmpeg and the antenna is Rust: the `no_std` SX1276 driver, the RP2040
USB-stick firmware, the wire format, the sender, the receiver and the report tool. ffmpeg
and ffplay are used as external programs for encoding and display.

## Results

Bench setup: Pi 2 + Dragino LoRa HAT and a CatWAN USB Stick RP2040, antennas about 1 m apart,
**915.0 MHz, +5 dBm**, 320x240 @ 15 fps, GOP 15 (one keyframe per second), x264
`ultrafast`/`zerolatency` at 180 kbit/s, XOR parity FEC with k = 8, **330 s per run**,
measured 2026-11-17. Encryption: [rind](https://github.com/kixel-ai/rind) main
`d2dc35bd796aaadc42df56aee0f1526cdee94b66`, ChaCha20-Poly1305 per object.

| run | dir | FSK | rind | video kbit/s | radio goodput kbit/s | radio frame loss | obj lost | video frame loss | obj lat p50/p95 ms | g2g p50/p95 ms | stalls >250 ms | Pi CPU % | bytes/obj: AEAD / total air overhead | seal / open µs |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| f300_off | Pi→Mac | 300k | none | 171.2 | 222.7 | 0.510% | 21 | 4.52% | 70 / 170 | 139 / 239 | 20 obj, 20 dec (worst 1207 ms) | 108 (tx 62 + enc 46) | 0 / 361 | 0.0 / 0.0 |
| f300_on | Pi→Mac | 300k | chacha20poly1305 | 171.8 | 225.6 | 0.568% | 27 | 4.58% | 71 / 172 | 138 / 241 | 20 obj, 20 dec (worst 1198 ms) | 109 (tx 64 + enc 46) | 20 / 382 | 94.9 / 5.5 |
| f350_off | Pi→Mac | 350k (x) | none | 172.2 | 222.6 | 0.386% | 20 | 4.01% | 62 / 143 | 131 / 212 | 17 obj, 17 dec (worst 1178 ms) | 100 (tx 54 + enc 46) | 0 / 361 | 0.0 / 0.0 |
| f350_on | Pi→Mac | 350k (x) | chacha20poly1305 | 173.0 | 225.2 | 0.392% | 18 | 3.52% | 63 / 145 | 132 / 214 | 17 obj, 17 dec (worst 1192 ms) | 101 (tx 55 + enc 46) | 20 / 383 | 95.8 / 17.3 |
| r300_off | Mac→Pi | 300k | none | 161.9 | 219.3 | 1.464% | 50 | 10.34% | 49 / 148 | 123 / 227 | 44 obj, 44 dec (worst 1680 ms) | 111 (rx 97 + dec 14) | 0 / 361 | 0.0 / 0.0 |
| r300_on | Mac→Pi | 300k | chacha20poly1305 | 165.5 | 222.1 | 1.563% | 44 | 8.46% | 51 / 153 | 126 / 229 | 37 obj, 36 dec (worst 1959 ms) | 111 (rx 97 + dec 14) | 20 / 382 | 17.9 / 77.6 |
| r350_off | Mac→Pi | 350k (x) | none | 154.6 | 217.2 | 2.553% | 74 | 14.85% | 43 / 125 | 117 / 207 | 64 obj, 64 dec (worst 2000 ms) | 110 (rx 97 + dec 13) | 0 / 360 | 0.0 / 0.0 |
| r350_on | Mac→Pi | 350k (x) | chacha20poly1305 | 154.9 | 218.6 | 2.937% | 77 | 14.89% | 44 / 128 | 118 / 207 | 59 obj, 59 dec (worst 2000 ms) | 110 (rx 97 + dec 13) | 20 / 382 | 20.0 / 75.8 |

Raw logs for every run are in [`results/video/`](results/video/); the table is regenerated with
`cargo run --release -p moq-lora-report -- --clock-log results/video/clock_log.jsonl results/video/<runs>`.

### What the numbers say

* **It plays.** Pi 2 -> laptop at 300 kbit/s FSK: 171 of 178 kbit/s of H.264 reached the player
  and 95.5% of frames were shown over 330 s, with glass-to-glass latency of **139 ms median,
  239 ms p95** (capture, x264 encode, airtime, reassembly, decode, one held frame).
* **rind costs 20 bytes per object and nothing measurable on the link.** With ChaCha20-Poly1305
  on, the air overhead per object went from 361 to 382 bytes (+1.3 kbit/s). Sealing took ~95 µs
  per object on the Pi 2 (0.14% of a core at 15 fps), and Pi CPU went from 108% to 109%. Loss
  and latency differences between off and on stayed within run-to-run variation at both bitrates
  and in both directions. AES-GCM is available too (`--cipher`); the Pi 2's Cortex-A7 has no
  AES instructions, so ChaCha20 is the default.
* **350k beats 300k Pi -> laptop** (outside the datasheet's checked envelope, `:x`): shorter
  airtime per frame gives 8-27 ms lower latency and slightly less loss in this pass.
* **The reverse direction works but loses more.** Laptop -> CatWAN -> Pi receives on the Pi's
  HAT, which has no DIO1 line, so the receiver busy-polls the FIFO from Linux userspace (one full
  core) and misses more frames: 1.5-2.9% radio frame loss and 8-15% of video frames not shown.
  Opening objects on the Pi took ~77 µs each. Latency is lower than the forward direction because
  the laptop encodes faster than the Pi.
* **Loss varies a lot between runs.** An earlier pass of the same 300k Pi -> laptop pair
  (10:22 PT, same settings) lost **0 objects** in both the off and on runs, versus 21 and 27 in the
  table above (11:00 PT). Compare off/on within a pass, not across passes.

### Firmware and transport fixes, before and after

All Pi -> laptop at 300k, 330 s. Latency here uses the offset measured before each run.

| change | before | after |
|---|---|---|
| equal-size fragments + 96 B minimum frame (short frames after long ones were lost) | radio frame loss 0.27% | 0.05% |
| end-to-end CRC-16 per frame (FIFO splices passed the radio CRC) | corrupt access units reached the decoder | 0 (rejected and counted) |
| AUD appended after each access unit | decoder held 2 frames | 1 frame |
| CatWAN fw v3 -> v4 (table-driven hex output), FEC k=8 | 1.5% video frame loss, 6 groups cut | 0.28%, 1 group cut |
| CatWAN fw v3 -> v4, no FEC | 6.2% video frame loss, 22 groups cut | 4.8%, 22 groups cut |
| CatWAN fw v4 -> v5 (per-packet TX timeout), reverse direction | stream corrupt after 60 s | 330 s runs as in the table |
| x264 instead of the Pi's hardware encoder (h264_v4l2m2m) | ~275 ms encoder delay | ~20-24 ms |

### How to read the columns

* **video kbit/s**: H.264 the player actually received, after the group gate threw away broken groups.
* **radio goodput**: bytes per second of frames the receiver accepted (headers, padding and parity included).
* **radio frame loss**: radio frames missing by sequence number. **obj lost**: frames FEC could not repair.
  **video frame loss**: frames that never reached the player (lost, or dropped with the rest of their group).
* **obj lat**: capture on the sender -> object complete on the receiver. **g2g**: capture -> frame
  decoded on the receiver, read from the barcode burned into the picture. Both are corrected for
  the clock difference between the two machines using a clock check every 5 s.
  g2g includes one frame (~67 ms) that the ffmpeg CLI decoder holds back.
* **stalls**: gaps longer than 250 ms between delivered objects / decoded frames.
* **Pi CPU**: percent of one Cortex-A7 core (the Pi 2 has four). Pi -> laptop: sender + its ffmpeg
  (capture and x264). Laptop -> Pi: receiver (it busy-polls the radio FIFO; the HAT has no DIO1) +
  the ffmpeg decoder.
* **bytes/obj**: what encryption adds per object, and everything the air carries beyond the H.264 itself
  (object header, AEAD, 12 B per radio frame, padding, parity).

## How it works

**Wire format.** A radio frame is at most 255 bytes: a 12-byte header
`seq u16 | group u16 | object u8 | fragment u8 | fragments u8 | flags u8 | object length u16 | CRC-16 u16`
and up to 243 bytes of the object. Objects are cut into equal-size fragments (no runt last
fragment) and frames are padded to 96 bytes: on this link a very short frame after a long one
was lost several times more often than the rest. With `--fec-k K` an XOR parity frame follows every
K data frames and repairs any single loss among them. The CRC-16 is end to end, on top of the
radio's own CRC, because the receiver's FIFO-polling path can occasionally splice two frames
into one that passes the radio check.

**Objects and groups.** Each frame of video is one object with a 17-byte header (frame number,
capture time, encoder delay, keyframe flag). Each GOP is one group. The sender drops whole groups
that have gone stale in its queue (`--max-age-ms`); the receiver's group gate cuts a group at the
first unrecoverable object and resumes at the next keyframe, so the decoder never gets a broken
reference chain. A lost frame costs at most the rest of its second of video.

**Latency measurement.** The sender burns its capture time into the bottom 16 rows of every
frame as a 40-bit barcode (plus a readable clock). The receiver decodes the live stream with
ffmpeg, reads the barcode back out of the pixels and subtracts. A clock check over ssh every
5 s supplies the offset between the two machines.

**Live decode.** Each access unit is fed to the player with an AUD appended, which makes
ffmpeg's parser release the frame immediately instead of waiting for the next one.

## Encryption with rind

With `--key-file` on both ends, every object is sealed with authenticated encryption from
[rind](https://github.com/kixel-ai/rind) before it is fragmented:
`session u32 | ciphertext | 16-byte tag`. The nonce is the session id plus the group and object
ids; the session id is random per run and redrawn whenever the group counter wraps. Group and
object ids are bound as associated data. An object that fails authentication is dropped and
counted, and its group is cut like any loss. The cost on air is 20 bytes per object (about
1.3 kbit/s at 15 fps); on the Pi 2 sealing took ~95 µs per object and opening ~77 µs; on the laptop 5-20 µs.
Pre-shared key only: no key exchange, and the fragment headers stay in clear.

## Hardware and build

See **[HOWTO.md](HOWTO.md)** for the parts list, pinouts, Pi setup, cross-compiling the static
armv7 binaries, flashing the CatWAN firmware (back up first), running, measuring, and RF notes.

| path | what |
|---|---|
| `moq-lora-wire` | wire format, fragmenter, XOR FEC, reassembler, MoQ group gate, H.264 Annex-B splitter, barcode, rind sealing |
| `moq-lora-send` | sender: ffmpeg capture + encode, stamping, SX1276 TX on the Pi (or CatWAN TX with `--catwan`) |
| `moq-lora-recv` | receiver: CatWAN serial (or the Pi's SX1276 with `--radio`), ffplay, recording, decode, metrics |
| `moq-lora-report` | markdown results table from run directories |
| `firmware/catwan-fw` | RP2040 firmware for the CatWAN stick (USB CDC line protocol), prebuilt `catwan-fw-v5.uf2` |
| `vendor/sx1276` | `no_std` embedded-hal 1.0 SX1276 driver (LoRa + FSK, long-packet FIFO refill/drain) |
| `scripts/` | the harness used for the table above |
| `results/video/` | raw logs for every run in the table |

## Radio rules

915.0 MHz only (US 902-928 MHz ISM band), +5 dBm maximum, never 925.0 MHz: the sender refuses
anything else, and nothing transmits without `--i-have-an-antenna`. This is a single-channel,
non-hopping FSK link keyed up to ~80% of the time; whether that meets FCC Part 15.247 depends on
its 6 dB bandwidth, which we have not measured. Outside the Americas, 915 MHz may not be an ISM band
at all. Details in [HOWTO.md](HOWTO.md#9-rf-and-legal-notes). You are responsible for your emissions.

## Known limitations

* The Pi's HAT has no DIO1, so frames longer than the 64-byte FIFO are drained by polling. On
  the Pi that costs a full core in receive mode; on the CatWAN a CRC failure mid-frame can splice
  two frames (caught by the end-to-end CRC, not fixed in the driver).
* The ffmpeg CLI decoder holds one frame (~67 ms at 15 fps) that a library decoder would not.
* Clock offset between the machines comes from an ssh round trip (~11 ms); latency figures carry
  a few ms of uncertainty.
* No key exchange: encryption uses a pre-shared key file.
* Single fixed channel, no hopping, no rate adaptation; the encoder bitrate is set by hand.
* Tested with ffmpeg's test pattern only, not a camera.

## License

MIT OR Apache-2.0.

## Example output

![Frame received over the SX1276 link with rind encryption](media/frame-f350-rind-45s-3x.png)

60 seconds of received video: [media/moq-lora-f350-rind-60s.mp4](media/moq-lora-f350-rind-60s.mp4). More in [media/](media/).
