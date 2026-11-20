# Security

moq-subghz is a hobby project and a bench demo. It is not audited, and it should not carry
anything you need kept secret.

## What the encryption covers

With `--key-file`, every video frame is sealed with an AEAD (ChaCha20-Poly1305 by default) from
[rind](https://github.com/kixel-ai/rind), under a subkey derived from your pre-shared key and a
random per-run salt. Someone listening can't read the picture, and a changed frame fails its
check and is treated as lost.

## What it does not cover

* **No key exchange.** Both ends read the same key file. Anyone with the file can read and send.
* **Replay.** A recorded frame can be sent again. The receiver ignores short replays, but a long
  one can show old video.
* **Radio headers are in the clear.** An attacker can cause loss or skew the loss counters, and
  can see frame sizes and timing.
* **Jamming.** Nothing stops someone transmitting on the same frequency.

The details are in [REVIEW.md](REVIEW.md).

## Reporting a problem

Open a GitHub issue for bugs, including crashes on malformed input. If you would rather not post
the details in public, use GitHub's "Report a vulnerability" button on the repository's Security
tab. There is no bounty and no promised response time.

## Radio

You are responsible for what you transmit. See the radio rules in the [README](README.md#radio-rules).
