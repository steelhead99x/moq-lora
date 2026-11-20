#!/bin/sh
# Pi side: one invocation of the stock Dragino app per text chunk. Prints "<start> <end> <header>" per chunk.
F="$HOME/factory-bench/$(basename "$1")"; cd "$HOME/factory-bench/dragino" || exit 1
while IFS= read -r line; do
  s=$(date +%s.%N)
  timeout 1.3 ./dragino_lora_app sender "$line" >/dev/null 2>&1
  e=$(date +%s.%N)
  echo "$s $e $(echo "$line" | cut -d: -f1,2)"
done < "$F"
