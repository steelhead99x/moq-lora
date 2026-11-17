# STATUS (updated 2026-11-17 11:40 PT)

Private repo, branch `video-demo` (main = initial commit). Not public; Kyle decides.

## Done
- Live video Pi 2 -> SX1276 FSK -> CatWAN -> Mac ffplay: 320x240, 15 fps, x264, ~179 kbit/s,
  MoQ groups (GOP) / objects (frames), XOR FEC, group gate, per-frame CRC-16, barcode latency.
- rind AEAD per object: kixel-ai/rind main **d2dc35bd796aaadc42df56aee0f1526cdee94b66**
  (git dep pinned). ChaCha20-Poly1305 default, AES-128/256-GCM. +20 B per object.
- Reverse direction: `moq-lora-send --catwan PORT` (Mac TX via stick), `moq-lora-recv --radio` (Pi RX).
- catwan-fw v5 (vendored in `firmware/`): fixes a TX timeout that broke stick TX streams > 60 s.
  Flashed 2026-11-17 10:42 PT.
- `moq-lora-report`: markdown table from run dirs; `--clock-log` re-bases latency on a 5 s clock log
  (Pi timesyncd stepped its clock ~50 ms during one run).
- HOWTO.md (hardware, pinout, build, flash, run, rind, RF/legal) and scripts/.

## Benchmark: done
8 runs x 330 s (rind off/on, 300k/350k, both directions), raw logs + table in `results/video/`,
table and findings in README.md.

## Next
- PNG frames + clip to the box; temp cleanup; confirm nothing running and radios idle.
- Kyle: review README/HOWTO, decide on making the repo public, merge `video-demo` into `main`.

## Known issues
- `fsk_poll_long` can splice two frames after a CRC fail (caught by the end-to-end CRC-16).
- ffmpeg CLI decoder holds one frame (~67 ms at 15 fps) inside g2g numbers.
- FCC 15.247: 6 dB bandwidth of 300k FSK unmeasured.
