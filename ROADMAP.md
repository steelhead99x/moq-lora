# Roadmap

## Known issues

* **Receiving on the Pi costs a whole core.** The Dragino HAT doesn't wire the radio's DIO1 pin,
  so the Pi can't be told when data arrives and has to keep checking for frames over 64 bytes.
  That costs a core, and it misses more frames than the USB stick does.
* **Rarely, the receiver splices two radio frames into one.** When a frame fails the radio's CRC
  partway through, the end-to-end CRC-16 catches and drops the result, but the driver doesn't prevent it.
* **The ffmpeg CLI decoder holds one frame** (~67 ms at 15 fps). That's included in the latency numbers.
* **Encryption is basic.** It uses a pre-shared key, with no key exchange and no replay protection,
  and the radio headers are sent unencrypted. Rotate the key long before 2^16 runs (session ids are
  32-bit random).
* **The stick's firmware doesn't enforce the power and frequency limits.** Only the host tools do.
  Fixing that needs a new firmware image.
* **The 350 kbit/s profile** is outside the SX1276 datasheet's tested range.
* **FCC compliance is unverified.** The signal's 6 dB bandwidth hasn't been measured, and the link
  doesn't hop channels.

## Ideas

* Benchmark a real camera (`--src /dev/video0`).
* Adapt the bitrate to measured loss instead of setting it by hand.
* Decode with libavcodec directly, to get back the frame the CLI decoder holds.
* Use a HAT or board with DIO1 wired, to get interrupt-driven receive on the Pi.
* Use FEC that can repair more than one loss per block.
* Add a replay window, and put the radio headers under the encryption tag.

The full code review, including what was fixed and what was deliberately left alone, is in [REVIEW.md](REVIEW.md).
