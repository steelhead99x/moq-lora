# Code review

Review of the `video-demo` tree for a public release: `moq-lora-wire`, `moq-lora-send`,
`moq-lora-recv`, `moq-lora-report`, `vendor/sx1276`, `firmware/catwan-fw`, and `scripts/`.

No radio hardware is attached to this environment. Nothing here was checked on a Pi or a
CatWAN stick, and the 330 s numbers in `results/video/` were not regenerated. Changes that
would alter on-air framing, FEC, pacing, or loss accounting were left in place and listed
below. What was fixed is covered by unit tests (`cargo test --all-targets` on the
workspace, plus the sx1276 driver tests).

Host build, clippy, and tests were run with rustc 1.90.0. `moq-lora-send` and
`moq-lora-recv` were also built for `armv7-unknown-linux-musleabihf` (rust-lld, static)
and `armv7-unknown-linux-gnueabihf` (`arm-linux-gnueabihf-gcc`, dynamic). The firmware
was built and clippy-clean for `thumbv6m-none-eabi`. `firmware/catwan-fw-v5.uf2` was
not replaced.

## Critical

None. No path was found that leaks the AEAD key, reuses a nonce on the measured
configuration (15 fps, GOP 15, one run), or panics on a well-formed 255-byte frame.
The items under High are real, but they are either gated by the host tools or they
need a protocol or firmware change that would invalidate the measured runs.

## High

### Firmware will transmit above +5 dBm and on any carrier. Deferred.

`firmware/catwan-fw/src/main.rs:232` and `:259` clamp `dbm` to `2..=17` for both the
`L` and `F` commands and program the frequency they are given. There is no check for
902-928 MHz and no refusal of 925.0 MHz. `moq-lora-send` (`main.rs:273` and `:276`, before it opens the stick) and
`moq-lora-recv` (`main.rs:313` hard-codes 5 dBm on the `F` command)
enforce the limits only when the host tools are used. A raw serial session can key the
stick at +17 dBm on an arbitrary frequency.

Left unchanged. The measured image is `firmware/catwan-fw-v5.uf2` (sha256 in
`firmware/SHA256SUMS`). Tightening the clamp means a new UF2 and a reflash, which was
not done here. The module comment at `firmware/catwan-fw/src/main.rs:16` says so.
README and HOWTO now say the same thing in the RF notes.

### No replay window, and object 0 always opens a group. Deferred.

`ObjectSealer::open` (`moq-lora-wire/src/seal.rs`) decrypts any ciphertext whose salt,
together with the cleartext group and object ids, matches the key. It does not
remember which `(salt, group, obj)` values it has already accepted. Replayed object 0s
from two older runs also push the live salt out of the receiver's two-entry cache, so
the live run's objects fail as `auth_fail` until its next object 0.
`Reassembler` (`moq-lora-wire/src/lib.rs:507`) remembers only the single object it just
finished. `GroupGate::accept` (`moq-lora-wire/src/lib.rs:575`) treats every `obj == 0`
as a fresh group and clears `broken`.

An active attacker who recorded a valid object can replay it later. If it is object 0,
the gate will hand it to the decoder as a new group. Integrity of the live picture
fails open under replay; confidentiality and tag checks still hold. A replay bitmap
would change what a lossy but honest link delivers (a late fragment of an earlier
object can still complete today). Not changed.

### Radio headers and FEC are outside the AEAD. By design.

`moq-lora-wire/src/seal.rs` binds only `group` and `obj` as AAD; the nonce is built
from the run's salt, `group` and `obj`. Sequence number, fragment index, `nfrag`, the
parity bit, `k`, and `objlen` are clear and unauthenticated. What that lets an on-path attacker do,
with `--key-file`:

* Drop, reorder, or retarget fragments, or alter parity: the reassembled object then
  fails `open` and is counted as `auth_fail`, a lost object. That is denial of service,
  which jamming the channel achieves anyway.
* Skew the loss statistics: with two or more consecutive forged frames far ahead it can
  fake a gap (one frame can add at most `SEQ_STEP - 1`; see `moq-lora-wire/src/lib.rs`).
  The video itself is unaffected.

It cannot read the picture or make altered content authenticate: any change to how
fragments reassemble changes the sealed bytes and fails the tag. It can replay recorded
objects at their original position, which the previous section covers. The headers also
show object sizes, so keyframe timing is visible. Authenticating the headers would cost a tag on
every radio frame (16 bytes, about 6% of airtime, or 4 bytes truncated, about 1.6%) and
a wire change, to protect only statistics. Decided 2026-11-18 (maintainer): keep the
headers clear. Without `--key-file` nothing is authenticated and an attacker can inject
Annex-B.

### `fsk_poll_long` can splice or reject a long packet. Deferred.

`vendor/sx1276/src/lib.rs:557`. On PayloadReady the driver reads whatever is still
needed in one `read_raw_burst` (`:574`). `read_raw_burst` (`:425`) rejects a read
longer than the 64-byte FIFO, so a long packet that is first observed only when
PayloadReady is already set comes back as an error and can leave `FskLongRx.got`
non-zero. Separately, a CRC failure clears the chip FIFO after the poller has already
copied a prefix; the next packet's bytes continue that prefix. The end-to-end CRC-16
in `FrameHdr::decode` drops the splice. It does not stop the driver from returning it.

This is the receive path that produced the Pi-side loss numbers. Changing it would
change those numbers. Not changed. `FskLongRx::payload` (`:169`) no longer panics when
no packet is stored (`1..=0` used to panic). That does not change a successful read.

### Nonce uniqueness is a caller contract. Fixed with per-run subkeys.

`Nonce::assume_unique_for_key` will silently reuse a ChaCha20 or AES-GCM keystream if a
nonce repeats under one key. Each run used to draw a random 32-bit session id as the
nonce prefix, so two runs under one key collided after about 2^16 runs. Now each run
draws a 96-bit salt and encrypts under a subkey derived from the key and the salt
(HKDF-SHA256, `moq-lora-wire/src/seal.rs`); object 0 of each group carries the salt in
the clear, other objects its first 4 bytes. The sender draws a new salt when the
16-bit group id wraps. Reusing a keystream now needs two equal 96-bit salts. The
receiver caches a salt's subkey only after an object under it authenticates, so a
forged object 0 can't evict the real one (a replayed one can; see "No replay window").
Until an object 0 authenticates, a receiver that joins mid-run counts the rest of that
group as `auth_fail`, so that counter mixes "no key yet" with tampering. Frame `VERSION` is 4 for this format.

Tag comparison is rind's `ct_equal` (ChaCha20-Poly1305 and AES-GCM `open`). This crate
does not compare tags itself. rind's `Key` drop wipes its copy; `open_sealer`
wipes the hex file bytes and the temporary key after `Key::new`.

## Medium

### A backward sequence number rewinds loss accounting. Deferred.

`moq-lora-wire/src/lib.rs:494`. A duplicate (`d == 0`) is ignored. A gap with
`d < 0x8000` counts as loss. Any other delta, including a single step backwards, still
updates `last_seq` and is fed to the reassembler. One old or hostile sequence number
then makes the next honest frame look like a large gap. Ignoring backward sequence
numbers would also drop a fragment that arrives late, which can be the fragment that
completes an object. The measured link does not reorder. Not changed.

### The sender's TX queue is unbounded. Deferred.

`moq-lora-send/src/main.rs` pushes every object onto the queue and only drops stale
groups when the radio thread reaches them. At 180 kbit/s into a 300 kbit/s radio the
queue stays small (the runs recorded `max_queue_frames` around 50). A faster encoder
or a stalled radio grows it without a cap. A cap would drop groups the current code
still sends. Not changed.

### Object id 255 starts a new group that may not be a keyframe. Fixed.

`advance` in `moq-lora-send/src/main.rs` starts a group only on a keyframe; an encoder
that sends more than 256 frames without one stops the run (`gop_overrun`) instead of
handing the decoder a P-frame as a random access point.

### If the capture queue is empty, the object is stamped `frame_no = u32::MAX`. Deferred.

`moq-lora-send/src/main.rs:561`. An extra access unit from the encoder is still sent,
with capture time set to "now". It shows up in the logs as a bogus frame number. The
measured runs have `captured == objects`. Not changed.

### A slow player drops access units. Deferred.

`moq-lora-recv/src/main.rs:135`. `try_send` into a 256-deep channel discards the
access unit when ffplay or the decoder is behind. That is intentional so the radio
loop does not block. It is not counted. Not changed.

### Inconsistent headers are counted as both good and bad. Deferred.

`Reassembler::push` increments `frames_ok` before it notices that `objlen` or `k`
disagrees with the object in progress, then also increments `frames_bad`. Loss rate
uses `frames_missing / (frames_ok + frames_missing)`, so this does not move the
published loss column, but `radio_frames_bad` can include frames that were also
counted as ok. Not changed.

### `fsk_transmit_long` blocks the USB loop. Deferred.

`firmware/catwan-fw/src/main.rs` calls `fsk_transmit_long` with a 1_000_000 spin limit
from inside the main loop, so USB is not polled for the whole long packet. That is
how v5 was measured. The per-packet TX timeout at the `tx_pkt_t` check is the v5 fix
and was not touched.

### Key file mode is checked, then the file is read. Fixed for the check; TOCTOU deferred.

`read_key_file` (`moq-lora-wire/src/seal.rs`). On Unix the file must be a regular file with no
group/other permission bits. A mode change between `metadata` and `read` is not
closed. Good enough for a pre-shared key on a single-user bench machine.

### Harness word-split the receiver's key path. Fixed.

`scripts/run2.sh` and `scripts/rev.sh` reject a clock offset that is not a number,
quote the sender arguments and remote paths with `printf %q`, and take the receiver's
key as `RX_KEY`, quoted on both sides of ssh, so a key path may contain spaces. `RX`
still adds other receiver arguments and is split by the shell. `run2.sh` and `rev.sh`
were kept separate: they drive opposite directions and the forward path waits 3 s, the
reverse path 4 s. Folding them together would be a new harness, not a cleanup.

## Low

### `fn seal` and `mod seal` shared a name. Fixed.

`moq-lora-wire/src/lib.rs` had both `pub mod seal` (AEAD) and `pub fn seal` (CRC-16).
The CRC function is now `write_crc`, which says what it does.

### The Pi GPIO glue is copied twice. Deferred.

`moq-lora-send/src/radio.rs` and `moq-lora-recv/src/radio.rs` duplicate `CdevIrq` and
`Radio::open`. The receiver's unused `send` method was removed. Merging the two
modules risks the TX path. Not merged.

### `localtime_r` failure is ignored. Deferred.

`moq-lora-send/src/main.rs` `local_clock`. A failure leaves the zeroed `tm`, so the
burned-in clock reads `00:00:00`. The barcode does not use that string. Not changed.

### CatWAN TX formats each byte with `format!("{:02x}")`. Deferred.

`moq-lora-send` `Link::send` for `Catwan`. It allocates per byte. Backpressure is the
stick's 12-frame queue, not this formatting. Changing it could change host CPU during
TX. Not changed.

### Report tool trusts the JSON it reads. Partially fixed.

`moq-lora-report/src/main.rs:92` no longer divides by zero when two clock samples
share a timestamp, and a trailing `--clock-log` without a path is an error (`:107`)
instead of a panic. `pct` no longer panics on NaN (`:45`). A run directory that is
not a directory still hits `file_name()` and prints an empty name rather than
panicking. The tool does not validate that `summary.json` came from this program.

### Window title said "LoRa". Fixed.

`moq-lora-recv` ffplay title is now `SX1276 FSK live video (moq-lora)`. The link is
FSK. The README's "best known as the LoRa chip" sentence is about the silicon, and
was left.

## Fixed in this change

| what | where | note |
|---|---|---|
| Reject frames longer than 255 before the CRC copy | `moq-lora-wire/src/lib.rs:178` | a hostile replay line used to allocate the whole buffer |
| `chunk_size` / `chunk_len` do not divide by zero or underflow | `lib.rs:220` | property test over every legal object length |
| FEC recovery does not `unwrap` a missing chunk | `lib.rs:407` | honest single-erasure repair is unchanged |
| Serial and replay lines capped at 1024 bytes | `lib.rs:48`, recv `main.rs:280` and `:367`, send `main.rs:152` | real `R` lines are about 550 bytes |
| Barcode read/write refuse a tiny plane instead of panicking | `stamp.rs:20`, `stamp.rs:47` | 320x240 is unchanged |
| Annex-B tail above `MAX_OBJECT` is dropped | `annexb.rs:54` | a sendable access unit is still emitted |
| Key file must be mode 0600 (or stricter); hex bytes wiped | `read_key_file`, `open_sealer` in `seal.rs` | both binaries use `open_sealer` |
| Profile parser rejects bitrate 0 and a suffix other than `x` | `lib.rs:95` | `0:...` used to panic in the airtime division |
| Per-run subkeys from a 96-bit salt replace the 32-bit session id | `seal.rs` | no birthday bound at 2^16 runs |
| Empty `FskLongRx::payload` does not panic | `vendor/sx1276/src/lib.rs:169` | |
| Stats line from the stick is JSON-escaped | `moq-lora-recv/src/main.rs:176` | normal `OK S ...` lines are unchanged |
| `panic = "abort"` on the firmware profiles | `firmware/catwan-fw/Cargo.toml` | thumbv6m cannot unwind; rustc 1.90 would not link without it |

Tests for the wire fixes are in `moq-lora-wire` (`hostile_frames_do_not_panic`,
`chunk_lengths_sum_to_objlen`, `read_line_limited_caps_and_keeps_short_lines`,
`key_file_mode_and_roundtrip`, and the existing FEC round-trips).

## Crypto notes (what was checked)

* Nonce is 12 bytes, unique per `(group, obj)` under one run's subkey; the subkey comes
  from a random 96-bit salt per run, redrawn on group wrap.
* AAD is the group and object ids. Moving a ciphertext to another position fails open.
  Flipping the clear salt prefix fails open (it picks the subkey and is in the nonce).
* Tag check is rind's constant-time compare, not a `==` in this repo.
* Key material is a file path, not an argument. The tools refuse a group- or
  world-readable file. No key file is in git, and `.gitignore` ignores
  `moq-lora-key.hex` and `*.key`.
* There is no key exchange and no replay cache. Both are documented in HOWTO and
  ROADMAP.

## Private data and history

No key, token, password, or private-key block was found in the tree or in any of the
16 commits. No LAN address was found.

Scrubbed from the working tree:

* `results/video/README.md` from `4d7f226` had `~/sx1276-link/moq-lora-key.hex`,
  `~/moq-lora-key.hex` (Pi account name `admin`), ssh alias `pi-host-a`, and
  `/dev/cu.usbmodem21301`. The file now uses `$PI`, `$PORT`, `$LKEY`, `$PKEY`.
* `scripts/env.sh` from `e358988` defaulted `PORT` to `/dev/cu.usbmodem21301` and
  `PI` to `pi-host-a`. Defaults are now `/dev/ttyACM0` and `pi`.

Left in measured files on purpose (`results/video/clock_log.jsonl`, 471 lines, and
each run's `clock_end.json`): `"host":"pi-host-a"`. Editing those would change the logs
the table was computed from. The alias is not a hostname with a public DNS name,
but it is this bench's ssh alias.

`firmware/catwan-fw/src/main.rs:157` hard-codes USB serial `613863E6354E26A3`. The
same bytes are in `firmware/catwan-fw-v5.uf2`. It looks like one board's id, so
every stick flashed with this image enumerates as that serial. It was not changed,
because the UF2 was not rebuilt.

AppleDouble `._*` files under `results/` were removed. A scan of their printable
strings did not find home paths or names.

Update 2026-11-17: after this review the owner chose a fresh root commit, so the
earlier history (and the strings above that lived only there) is not in this repository.
The license is `MIT OR Apache-2.0`, with both texts at the top level.
