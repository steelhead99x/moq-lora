#!/usr/bin/env python3
"""Print the old stack and moq-lora side by side from a results.json (default: ../results)."""
import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(HERE, "..", "results", "results.json")
R = json.load(open(path))


def row(label, a, b, note=""):
    print(f"  {label:<38}{a:>16}{b:>16}   {note}")


def paced(w, loss):
    return next(r for r in R["paced"] if r["workload"] == w and r["loss"] == loss)


losses = sorted({r["loss"] for r in R["paced"] if r["workload"] == "video"})
loss = losses[-1]
v, s = paced("video", loss), paced("sensor", 0.0)
vm = paced("video", 0.0)
print(f"\nold stack vs moq-lora  ({R['date']}, simulated radio on this computer)\n")
row("", "old stack", "moq-lora")
print("  " + "-" * 72)
if loss:
    print(f"  Video, {v['old']['payloads']} frames, {loss*100:g}% radio loss")
    row("  frames that arrived", f"{v['old']['delivered_pct']:.1f}%", f"{v['moq']['delivered_pct']:.1f}%")
    row("  frames you can play", f"{v['old']['decodable_pct']:.1f}%", f"{v['moq']['decodable_pct']:.1f}%",
        "(a lost frame spoils its group)")
    if v["old"]["payloads"] < 100:
        print("    (short clip, so the numbers jump around between runs; the full run uses 300 frames)")
print("  Video, no loss")
row("  software delay p50", f"{vm['old']['lat_ms']['p50']:.2f} ms", f"{vm['moq']['lat_ms']['p50']:.2f} ms", "(localhost)")
print("  Small sensor readings")
row("  bytes on air per reading", f"{s['old']['air_bytes_per_payload']:.0f}", f"{s['moq']['air_bytes_per_payload']:.0f}",
    "LoRaWAN is smaller here")
c = R["command"]
print(f"  Command round trip (old stack, Class A, uplink every {c['interval_s']:g} s)")
row("  median", f"{c['old']['p50']:.1f} ms", f"{c['moq']['p50']:.2f} ms", "(p50)")
mo, mm = vm["model"]["old"], vm["model"]["moq"]
print("  Channel time for the video (computed, not measured)")
row("  share of one channel", f"{mo['channel_busy_pct']:,.0f}%", f"{mm['channel_busy_pct']:.0f}%", "over 100% = cannot keep up")
print("\n  Radio is simulated; range is not compared. Full run: demo/oldstack/run.sh\n")
