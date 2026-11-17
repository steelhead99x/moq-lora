# STATUS (updated 2026-11-17 11:00 PT)

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

## Running now (Mac, unattended, started 10:43 PT, ends ~11:35 PT)
`runs/bench.sh`: FEC k=8, 330 s each: f300_off, f300_on, r300_off, r300_on, f350_off, f350_on,
r350_off, r350_on (f = Pi->Mac, r = Mac->Pi). Clock log every 5 s alongside.

## Numbers so far (300k, Pi->Mac, FEC 8, 330 s, this pass)
| | rind off | rind on (chacha20poly1305) |
|---|---|---|
| delivered video kbit/s | 171.2 | 171.8 |
| radio goodput kbit/s | 222.7 | 225.6 |
| radio frame loss | 0.51% | 0.57% |
| objects lost / video frame loss | 21 / 4.5% | 27 / 4.6% |
| obj latency p50/p95 ms (start offset) | 67 / 168 | 68 / 171 |
| g2g p50/p95 ms (start offset) | 137 / 238 | 136 / 238 |
| stalls > 250 ms | 20 | 20 |
| Pi CPU % of one core (tx + ffmpeg) | 108 (63 + 46) | 109 (64 + 46) |
| bytes per object (AEAD / total air overhead) | 0 / 361 | 20 / 382 |
| seal on Pi / open on Mac, µs per object | - | 95 / 5.5 |

An earlier pass of the same pair (10:22 PT, before the clock log) lost 0 objects in both runs;
loss varies run to run with the RF environment, so compare off/on pairs, not passes.

## Next
1. When bench finishes: `moq-lora-report --clock-log runs/clock_log.jsonl runs/{f,r}{300,350}_{off,on}`.
2. Commit raw logs (gzipped jsonl + serial logs, summaries) to `results/video/<run>/`.
3. README.md rewrite with the results table; results/video/README.md with exact commands.
4. PNG frames + <20 MB clip to the box (/workspace/video-demo/).
5. Clean temp files; leave no processes; radios idle.

## Known issues
- `fsk_poll_long` can splice two frames after a CRC fail (caught by the end-to-end CRC-16).
- ffmpeg CLI decoder holds one frame (~67 ms at 15 fps) inside g2g numbers.
- FCC 15.247: 6 dB bandwidth of 300k FSK unmeasured.
