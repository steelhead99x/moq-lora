#!/bin/sh
F=results/lora-2015/frames
for i in 1 2 3; do
  python3 results/factory-firmware/tools/factory_bench.py --out results/factory-firmware/data --label small-160x120-q20-run$i --jpeg $F/f160x120-q20.jpg --passes 4 > logs-small-$i.txt 2>&1
done
for i in 1 2 3; do
  python3 results/factory-firmware/tools/factory_bench.py --out results/factory-firmware/data --label large-320x240-q10-run$i --jpeg $F/f320x240-q10.jpg --passes 4 > logs-large-$i.txt 2>&1
done
echo done > ALLDONE
