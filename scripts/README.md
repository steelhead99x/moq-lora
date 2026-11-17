# scripts

Harness for the bench in `results/video/`. These are the scripts that produced the
table; they are not scratch. `run2.sh` and `rev.sh` stay separate because they
drive opposite directions and do not share pacing (the forward path waits 3 s
before the Pi transmits, the reverse path waits 4 s before the laptop transmits).

| script | direction | what |
|---|---|---|
| `env.sh` | both | `PORT`, `PI`, `PI_DIR`, `BIN`, `RUNS`. Sourced, not executed. |
| `run2.sh` | Pi -> CatWAN/laptop | clock check, receiver on the laptop, sender on the Pi |
| `rev.sh` | laptop -> CatWAN -> Pi | clock check, receiver on the Pi, sender on the laptop |
| `clocklog.sh` | both | Pi-minus-laptop offset every 5 s until `$RUNS/clocklog.stop` |
| `bench.sh` | both | the 8-run rind off/on matrix (300k and 350k, 330 s each) |

```sh
export PORT=/dev/ttyACM0          # or /dev/cu.usbmodem* on macOS
export PI=pi                      # ssh host
export LKEY=$HOME/moq-lora-key.hex
export PKEY=$LKEY                 # path of that same file on the Pi
scripts/run2.sh  NAME 300000:100000:6 330 --fec-k 8
scripts/rev.sh   NAME 300000:100000:12 330 --fec-k 8
scripts/bench.sh
```

`RX="--key-file $PKEY"` (forward) or `RX="--key-file $LKEY"` (reverse) adds
receiver arguments. Key paths must not contain spaces. Sender arguments are
quoted for the remote shell; a key path still has to be a single argument.

The published runs used an ssh alias and a macOS `cu.usbmodem*` port. Those
values are not defaults here. Set `PI` and `PORT` to your own.
