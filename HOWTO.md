# HOWTO: live video over an SX1276, step by step

This walks from a bare Raspberry Pi 2 and a CatWAN stick to a live 320x240 picture in
`ffplay`, with numbers. It is the exact setup the results in [README.md](README.md) came
from. Budget an hour if the Pi is already on your network.

> **Radio rules first.** Everything here transmits on **915.0 MHz** at **+5 dBm** (the tools
> refuse anything outside 902-928 MHz, refuse 925.0 MHz, and refuse more than +5 dBm), and
> nothing transmits without `--i-have-an-antenna`. Read [RF and legal notes](#9-rf-and-legal-notes)
> before you key up. Never run an SX1276 without an antenna or a load attached.

---

## 1. Hardware

| part | role | notes |
|---|---|---|
| Raspberry Pi 2 Model B (BCM2836, 4x Cortex-A7 @ 900 MHz, 1 GB) | camera end / sender | any Pi with a 40-pin header works; the Pi 2 is the "old hardware" point |
| Dragino LoRa/GPS HAT (HopeRF RFM95W = Semtech SX1276, 868/915 MHz version) | Pi radio | the GPS half is unused |
| Electronic Cats **CatWAN USB Stick RP2040** (RP2040 + RFM95W) | viewer end / receiver | plugs into the laptop's USB |
| 915 MHz antennas (2) | | the HAT's SMA whip and the stick's whip; our bench had ~1 m between them |
| A laptop (macOS or Linux) | runs the receiver and `ffplay` | Rust >= 1.90, `ffmpeg`/`ffplay` |

No camera is needed: by default the sender uses ffmpeg's `testsrc2` pattern and burns a clock and a
40-bit timestamp barcode into each frame so the receiver can measure glass-to-glass
latency. Swap `--src` for a V4L2 camera later (see [section 8](#8-going-further)).

### Pinout

**Dragino HAT on the Pi** (fixed by the HAT; listed so you can check a different board):

| SX1276 | Pi (BCM) | Pi header pin | Linux |
|---|---|---|---|
| SCK / MISO / MOSI | GPIO11 / GPIO9 / GPIO10 | 23 / 21 / 19 | `/dev/spidev0.0` (SPI0, CE0 not used for chip-select) |
| NSS (chip select) | **GPIO25** | 22 | driven as a plain GPIO via `/dev/gpiochip0` |
| RESET | **GPIO17** | 11 | GPIO |
| DIO0 | **GPIO4** | 7 | GPIO (TX done / RX done) |
| DIO1 | not wired on this HAT | | frames > 63 B are handled by polling the FIFO instead |

**CatWAN USB Stick RP2040** (from the board's KiCad netlist; firmware in `firmware/catwan-fw`):

| SX1276 | RP2040 |
|---|---|
| MISO / NSS / SCK / MOSI | GPIO16 / GPIO17 / GPIO18 / GPIO19 (SPI0) |
| RESET | GPIO21 |
| DIO0 | GPIO10 |
| RX LED | GPIO25 |

---

## 2. Prepare the Pi

1. Raspberry Pi OS (the bench Pi ran Trixie, 32-bit; Bookworm or later should be fine). Enable SPI: `sudo raspi-config` ->
   Interface Options -> SPI, or add `dtparam=spi=on` to the boot config, then reboot.
2. Give your user the device groups so nothing needs root:
   `sudo usermod -aG spi,gpio $USER` (log out and back in). Check: `ls -l /dev/spidev0.0 /dev/gpiochip0`.
3. Install ffmpeg with libx264: `sudo apt install ffmpeg`. Check: `ffmpeg -hide_banner -encoders | grep -E 'libx264|h264_v4l2m2m'`.
   (We measured the Pi's hardware encoder `h264_v4l2m2m` too; it works but adds ~275 ms of
   encoder delay versus ~20-24 ms for x264 `ultrafast`/`zerolatency`, so x264 is the default.)
4. Make sure the laptop can `ssh` to the Pi without a password (the test harness uses it for
   clock checks): `ssh-copy-id <user>@<pi>`, and an ssh alias helps (`Host pi` in `~/.ssh/config`).

## 3. Build

Everything is one Cargo workspace. On a Linux or macOS machine with Rust 1.90 or newer
(the `rust-version` in `Cargo.toml`; rind's own MSRV):

```sh
git clone <this repo> moq-lora && cd moq-lora
cargo test -p moq-lora-wire                    # framing, FEC, reassembly, AEAD, barcode
cargo build --release                          # host tools: send, recv, report

# The Pi binary: a static armv7 musl executable, linked with rust-lld (no cross gcc needed)
rustup target add armv7-unknown-linux-musleabihf
cargo build --release -p moq-lora-send -p moq-lora-recv --target armv7-unknown-linux-musleabihf
scp target/armv7-unknown-linux-musleabihf/release/moq-lora-{send,recv} pi:moq-lora/
```

`.cargo/config.toml` points `armv7-unknown-linux-musleabihf` at `rust-lld`. Those
are the binaries to copy to the Pi: static, about 1.4 MB, nothing needed on the
Pi except ffmpeg.

`armv7-unknown-linux-gnueabihf` also builds, linked with `arm-linux-gnueabihf-gcc`
(package `gcc-arm-linux-gnueabihf`). That binary is dynamically linked against
glibc. `rust-lld` cannot see that sysroot, so it is not the default.

## 4. Flash the CatWAN firmware

The stick runs `firmware/catwan-fw` (Rust, `rp2040-hal`, the same vendored `sx1276` driver).
A prebuilt image is in `firmware/catwan-fw-v5.uf2` (`firmware/SHA256SUMS`).

1. **Back up what is on the stick first.** Hold BOOT while plugging it in (or, if it already
   runs this firmware, send `BOOT` on its serial port), then:
   ```sh
   picotool info -a                       # make sure you are talking to the stick, not another RP2040
   picotool save -a catwan-backup-$(date +%F).bin
   sha256sum catwan-backup-*.bin >> SHA256SUMS
   ```
   With more than one RP2040 attached, add `--ser <serial>` to every picotool command.
2. Flash: `picotool load -x firmware/catwan-fw-v5.uf2` (or drag the `.uf2` onto the
   `RPI-RP2` drive).
3. Or build it yourself:
   ```sh
   cd firmware/catwan-fw
   rustup target add thumbv6m-none-eabi
   cargo build --release
   python3 elf2uf2.py target/thumbv6m-none-eabi/release/catwan-fw catwan-fw.uf2
   ```
4. Check it: open the port (macOS `/dev/cu.usbmodem*`, Linux `/dev/ttyACM*`) and send `I`;
   it answers `I catwan-fw <version> sx_version=0x.. t_us=...` (the SX1276 reads 0x12 once it
   has been configured; 0x00 straight after boot is normal). The line protocol is documented at the top of
   `firmware/catwan-fw/src/main.rs` (`F` configure FSK, `R` receive, `Q` transmit, `S` stats).

Firmware v4 formats received frames with a lookup table instead of `core::fmt`;
at 300 kbit/s the old formatter was slow enough to miss the start of the next back-to-back
frame. v5 (the one in this repo) fixes a transmit timeout that was measured from the start
of a burst instead of per packet: after 60 s of continuous `Q` traffic v4 started each packet
on top of the previous one, which the Pi received as short, corrupt frames. Only the
reverse direction (stick transmitting) is affected. See the results for before/after.

## 5. First light (Pi -> laptop)

On the laptop (receiver first, it waits for frames):

```sh
./target/release/moq-lora-recv --port /dev/cu.usbmodemXXXX \
    --profile 300000:100000:6 --secs 120 --out runs/first --play --decode
```

On the Pi:

```sh
cd ~/moq-lora && ./moq-lora-send --profile 300000:100000:6 --fec-k 8 --secs 110 \
    --log /tmp/first_send.jsonl --i-have-an-antenna
```

An `ffplay` window titled *LoRa SX1276 live video (moq-lora)* opens with the test pattern,
a running clock and the barcode strip at the bottom. When the run ends both sides print a
one-line JSON summary; the receiver also writes `runs/first/summary.json`, `objects.jsonl`,
`decoded.jsonl` and `rec.h264`.

Make a clip to share: `ffmpeg -framerate 15 -i runs/first/rec.h264 -c copy first.mp4`.

### What the profile means

`--profile BITRATE:FDEV:PREAMBLE[:x]` sets the FSK link: 300000:100000:6 is 300 kbit/s,
±100 kHz deviation, 6-byte preamble. `:x` allows settings outside the SX1276 datasheet's
checked envelope (we use `350000:100000:6:x`); it works on our units, but it is your call.
Both ends must use the same bitrate and deviation.

## 6. Measuring properly

Latency needs the two clocks compared. `moq-lora-recv --clock-check pi` runs an ssh
ping-pong and prints the Pi-minus-laptop offset; pass it as `--clock-offset-ms` (positive if
the capture clock is ahead). Our clocks drifted 20-40 ms between runs, so the harness
re-checks before and after each run.

The harness in `scripts/` does all of it:

```sh
scripts/run2.sh  NAME PROFILE SECS [sender args]   # Pi -> laptop (receiver plays + decodes)
scripts/rev.sh   NAME PROFILE SECS [sender args]   # laptop -> CatWAN -> Pi (reverse)
scripts/bench.sh                                     # the matched rind off/on matrix in README
cargo run --release -p moq-lora-report -- runs/*/    # markdown table
```

Set `PORT`, `PI`, and the key paths in the environment or in `scripts/env.sh`. See
[scripts/README.md](scripts/README.md). `run2.sh` is Pi to laptop; `rev.sh` is laptop to Pi.
They are the bench harness, not leftover scratch.

What gets measured:

* **delivered video bitrate**: H.264 bytes handed to the player per second (after the MoQ group gate).
* **radio goodput**: frame bytes the receiver accepted per second (headers, padding, parity included).
* **radio frame loss**: frames missing by sequence number; CRC-rejected frames are counted separately.
* **object / video frame loss**: frames that never reached the player (lost, or cut with their group).
* **object latency**: capture (Pi clock) -> object reassembled (laptop clock), offset-corrected.
* **glass-to-glass (g2g)**: capture -> frame decoded by ffmpeg on the receiver, read back from the
  barcode in the picture itself. It includes encode, airtime, reassembly and decode.
* **stalls**: gaps > 250 ms between consecutive delivered objects / decoded frames.
* **CPU**: getrusage of the Rust process and of its ffmpeg children over the run, in % of one core.

## 7. Encrypt it with rind

Each MoQ object can be sealed with authenticated encryption from
[rind](https://github.com/kixel-ai/rind) (ChaCha20-Poly1305 by default; AES-128-GCM and
AES-256-GCM also available):

```sh
( umask 077; openssl rand -hex 32 > moq-lora-key.hex )      # once; mode 0600; never commit it
scp moq-lora-key.hex pi:                                    # same key on both ends
# Pi
./moq-lora-send ... --key-file ~/moq-lora-key.hex [--cipher chacha20poly1305|aes128gcm|aes256gcm]
# laptop
./moq-lora-recv ... --key-file moq-lora-key.hex   [--cipher ...]
```

Both tools refuse the file if group or other permission bits are set. The path is an
argument; the key itself is not.

On the air an object becomes `session u32 | ciphertext | tag 16` (20 bytes more per object).
The nonce is `session | group | object`; the session id is random per run and is redrawn
when the 16-bit group id wraps, so a (key, nonce) pair is never reused within a run. The
group and object ids are bound as associated data, so a frame from one object cannot be
spliced into another. Objects that fail authentication are dropped and counted
(`auth_fail`), and the group is cut like any other loss.

What it does not do: there is no key exchange (pre-shared key), no replay window across
runs with the same key beyond the random session id, and the fragment headers (sequence,
group, object, sizes) stay in clear.

## 8. Going further

* **Reverse direction**: `moq-lora-send --catwan /dev/cu.usbmodemXXXX` transmits through the
  stick, `moq-lora-recv --radio` receives on the Pi's HAT. Use a 12-byte preamble at 300k
  and 14 at 350k: the Pi polls the FIFO from userspace and needs the extra lead-in.
* **A real camera**: `--src` takes an ffmpeg lavfi source name (`testsrc2`, `smptehdbars`, ...)
  or a path, which is handed to ffmpeg as input (files loop). A USB/V4L2 camera at
  `/dev/video0` goes through the same path; we have not benchmarked one. The clock and barcode are burned into
  whatever the source delivers (bottom 16 rows), so latency measurement still works.
* **Hardware encoder**: `--encoder hw` uses `h264_v4l2m2m`. Lower CPU, ~250 ms more latency.
* **Loss vs. overhead**: `--fec-k` trades airtime for repair (k=8 costs 1/8 more frames);
  `--min-frame` pads tiny last fragments, which the receiver's FSK path loses more often.
* **Testing without RF**: `moq-lora-send --loopback frames.txt` writes the frames as CatWAN
  `R` lines; `moq-lora-recv --replay frames.txt --paced` plays them back.

## 9. RF and legal notes

* Frequency: **915.0 MHz** only in this project (US 902-928 MHz ISM band). The tools reject
  anything outside 902-928 MHz and refuse 925.0 MHz outright.
* Power: **+5 dBm** conducted at most (the sender refuses more; 2-5 dBm accepted).
* This is a single-channel, continuously keyed FSK link at up to 79% duty cycle. In the US,
  FCC Part 15.247 allows digital modulation in this band without hopping only if the 6 dB
  bandwidth is at least 500 kHz, with power spectral density limits; we have **not measured**
  the 6 dB bandwidth of 300 kbit/s, ±100 kHz FSK, and it may fall short. Otherwise the rules
  for frequency hopping systems (dwell time, channel count) apply, and this link does not hop.
  We ran short bench tests at +5 dBm with antennas ~1 m apart. Check your own rules
  (FCC, ISED, ETSI, ...) before you transmit; outside the Americas 915 MHz may not be an ISM
  band at all (Europe uses 868 MHz, with duty-cycle limits this link would exceed).
* The SX1276 and RFM95W modules are not certified for this use as configured here; you are the
  operator and responsible for the emissions.
* The host tools enforce the frequency and +5 dBm limits above. The CatWAN firmware does not:
  its `F` and `L` commands accept 2 to 17 dBm and the frequency number they are given, and
  350 kbit/s is outside the SX1276 datasheet's characterized FSK settings (`:x` on the
  profile acknowledges that). Do not type those commands into a raw serial session if you
  are counting on the host checks. The prebuilt `firmware/catwan-fw-v5.uf2` matches the
  source in this tree, including that wider clamp.
