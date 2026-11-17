# Edit for your setup. Sourced by the run scripts.
: "${PORT:=/dev/cu.usbmodem21301}"      # CatWAN serial port on the laptop
: "${PI:=pi2b}"                          # ssh alias of the Pi
: "${PI_DIR:=moq-lora}"                  # where moq-lora-send / moq-lora-recv live on the Pi
: "${BIN:=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/target/release}"
: "${RUNS:=$(pwd)/runs}"
mkdir -p "$RUNS"
