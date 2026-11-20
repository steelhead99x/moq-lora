# Loss sweep (no radio)

**These numbers did not come off a radio.** They come from the real sender and receiver, run
through a file, with radio frames removed on purpose. They answer one question the eight radio
runs in [`../video/`](../video/) can't: how does the picture hold up as the link gets worse?

Measured 2026-11-20 on the current code (frame format version 4).

## What was run

1. `moq-subghz-send --loopback` recorded 300 s of "air" four times: no FEC, FEC k=4, FEC k=8, and
   FEC k=8 with encryption. Same settings as the radio runs otherwise: 320x240, 15 fps, GOP 15,
   x264 at 180 kbit/s, FSK profile `300000:100000:6`, test pattern.
2. `scripts/loss_sweep.py` removed radio frames from each recording and played what was left
   through `moq-subghz-recv --replay`. Every case was replayed 20 times with different frames
   removed, and the table shows the mean.
3. Frames were removed two ways:
   * **Random:** each radio frame is lost on its own with the same chance.
   * **Radio trace:** the loss pattern of a real radio run, taken from its serial log and laid
     over the recording from a random starting point.

## The recordings

| recording | FEC | encrypted | air rate, kbit/s | radio busy | bytes on air per video frame, on top of the video |
|---|---|---|---|---|---|
| k0 | off | no | 190.7 | 72.8% | 97 |
| k4 | 1 parity per 4 | no | 251.9 | 96.2% | 607 |
| k8 | 1 parity per 8 | no | 222.5 | 84.8% | 361 |
| k8enc | 1 parity per 8 | yes | 224.9 | 85.9% | 383 |

A video frame averages 1490 bytes (a keyframe 4217, the rest 1295). A keyframe takes about 18
radio frames and the others about 6, so a one-second GOP is about 100 radio frames.

## Video frames not shown, random loss

| radio frames lost | no FEC | FEC k=4 | FEC k=8 | FEC k=8, encrypted |
|---|---|---|---|---|
| 0% | 0.00% | 0.00% | 0.00% | 0.00% |
| 0.1% | 5.25% | 0.00% | 0.01% | 0.02% |
| 0.2% | 10.74% | 0.03% | 0.08% | 0.15% |
| 0.5% | 24.66% | 0.36% | 0.59% | 0.54% |
| 1% | 42.37% | 1.31% | 1.78% | 2.31% |
| 1.5% | 55.99% | 2.98% | 4.78% | 4.42% |
| 2% | 65.43% | 4.91% | 7.57% | 8.06% |
| 5% | 89.23% | 25.71% | 36.05% | 35.76% |
| 10% | 97.45% | 64.75% | 74.02% | 75.84% |

## Video frames not shown, loss pattern from the radio runs

| loss pattern from run (radio frames lost) | no FEC | FEC k=4 | FEC k=8 | FEC k=8, encrypted |
|---|---|---|---|---|
| r300_off (1.50%) | 46.99% | 8.55% | 9.71% | 9.60% |
| r300_on (1.56%) | 53.33% | 8.55% | 9.34% | 9.61% |
| r350_off (2.56%) | 70.23% | 12.27% | 14.68% | 15.01% |
| r350_on (2.94%) | 75.17% | 15.41% | 18.83% | 18.55% |

## What it shows

* **Dropping the rest of the GOP is expensive without FEC.** Losing one radio frame in a thousand
  costs one video frame in twenty. One lost radio frame takes out, on average, half a second of
  picture.
* **One parity frame per eight fixes most of that for random loss.** At 1% loss, 42% of frames
  not shown becomes 1.8%, for 17% more air time.
* **Real loss is worse than random loss at the same rate.** The radio runs lost 1.5% of their
  frames and about 10% of the events were two or more frames in a row
  ([`traces.csv`](traces.csv)). One XOR parity frame can't repair two losses in its block, so
  the real pattern costs about 9.6% of the picture where random loss at 1.5% costs 4.8%.
  Going from k=8 to k=4 barely helps, because neighbours still land in the same block.
* **Past about 2% loss this scheme is not enough.** A third of the picture is gone at 5%.
* **Encryption does not change the picture.** The encrypted and plain columns agree within the
  spread between replays.
* **A freeze is about 0.6 s.** That is the mean with FEC k=8 from 0.5% to 2% loss, and on the
  radio patterns. The worst single freeze at 1% loss with FEC k=8 was 1.1 s. The full numbers are in
  [`sweep.csv`](sweep.csv).

## How far to trust it

* The replay reproduces the radio. Laying the loss pattern of each run over a new recording
  gives 9.7%, 9.6%, 14.7% and 18.6% of frames not shown (the plain or encrypted k=8 column,
  whichever matches the run). The radio runs themselves measured 10.3%, 8.5%, 14.9% and 14.9%.
  The last is the furthest off.
* The no-FEC column matches arithmetic. If each radio frame is lost on its own, a video frame is
  shown only when every radio frame before it in its GOP arrived. Working that out from the
  sender's log gives 5.7%, 10.9%, 24.8%, 42.5%, 64.9%, 89.3% and 97.5% for the rows it was worked out for
  (all but 1.5%).
* Every delivered video frame was compared, byte for byte, with what the sender produced, in 60
  replays at 2%, 5% and 10% loss. All 162,504 matched.
* Only the four runs towards the Pi (`r*`) are used as loss patterns. The serial logs of the four
  runs towards the laptop don't mark which lines failed their frame check, so the pattern can't
  be rebuilt from them. The script checks each log against the receiver's own count and refuses
  one that disagrees by more than 5%.
* The radio runs were on a bench, antennas about a metre apart. Nothing here says what loss looks
  like at range.
* Latency is not measured here. A file has no air time.
* In the encrypted column, `auth_fail` in `sweep.csv` counts frames that arrived before the first
  intact keyframe of the run. They can't be opened until then. None was a damaged frame that got
  through.

## Run it again

```sh
cargo build --release
S=$PWD/target/release/moq-subghz-send
mkdir -p /tmp/rec
(umask 077; head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > /tmp/rec/key.hex)
(cd /tmp/rec
 $S --loopback k0.air    --secs 300 --fec-k 0 > k0.send_summary.json &
 $S --loopback k4.air    --secs 300 --fec-k 4 > k4.send_summary.json &
 $S --loopback k8.air    --secs 300 --fec-k 8 > k8.send_summary.json &
 $S --loopback k8enc.air --secs 300 --fec-k 8 --key-file key.hex > k8enc.send_summary.json &
 wait)
python3 scripts/loss_sweep.py sweep --recv target/release/moq-subghz-recv --rec /tmp/rec \
  --runs results/video/*/ --out results/loopback
```

A new recording differs a little from the one measured here, so expect the tables to move a
little. On the same recording the script gives the same numbers every time.
