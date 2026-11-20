#!/usr/bin/env python3
"""Replay loopback recordings through moq-subghz-recv with radio frames removed, and tabulate
what the viewer would have seen. No radio is involved: see results/loopback/README.md.

  loss_sweep.py traces RUN_DIR...            loss pattern of each measured radio run (serial.log.gz)
  loss_sweep.py sweep --recv BIN --rec DIR --runs RUN_DIR... --out DIR
"""
import argparse
import csv
import gzip
import json
import random
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

FPS = 15
IID_RATES = [0.0, 0.001, 0.002, 0.005, 0.01, 0.015, 0.02, 0.05, 0.10]
SEEDS = 20
NEAR = 32


def trace(run_dir):
    """Loss mask of one radio run, in sender order: True where the frame never arrived intact.

    The serial log names the first two bytes (the 16-bit sequence number) of every line the radio
    handed over, so the gaps are the frames that were lost. It does not say which lines failed the
    frame CRC. A line is trusted when its number follows the last trusted one closely, or when the
    next line continues from it; anything else is noise and is skipped.

    Fails if the result disagrees with the receiver's own count for that run by more than 5%.
    """
    seqs = []
    with gzip.open(Path(run_dir) / "serial.log.gz", "rt") as f:
        for line in f:
            p = line.split()
            if len(p) == 7 and p[1] == "R" and len(p[6]) == 8:
                seqs.append(int(p[6][4:], 16))
    mask, last = [], None
    for i, seq in enumerate(seqs):
        if last is None:
            near = True
        else:
            gap = (seq - last) % 65536
            near = 1 <= gap <= NEAR
        confirmed = i + 1 < len(seqs) and 1 <= (seqs[i + 1] - seq) % 65536 <= 4
        if not near and not (confirmed and last is not None and 1 <= (seq - last) % 65536 < 32768):
            continue
        if last is not None:
            mask.extend([True] * ((seq - last) % 65536 - 1))
        mask.append(False)
        last = seq
    counted = json.loads((Path(run_dir) / "summary.json").read_text())["radio_frames_missing"]
    if abs(sum(mask) - counted) > 0.05 * counted:
        raise ValueError(f"{run_dir}: log gives {sum(mask)} lost frames, the receiver counted {counted}")
    return mask


def bursts(mask):
    out, run = [], 0
    for lost in mask:
        if lost:
            run += 1
        elif run:
            out.append(run)
            run = 0
    if run:
        out.append(run)
    return out


def trace_stats(name, mask):
    b = bursts(mask)
    lost = sum(mask)
    return {
        "run": name,
        "frames_sent": len(mask),
        "frames_lost": lost,
        "loss_pct": round(100 * lost / len(mask), 3),
        "loss_events": len(b),
        "mean_burst": round(statistics.mean(b), 2) if b else 0,
        "p95_burst": sorted(b)[int(0.95 * (len(b) - 1))] if b else 0,
        "max_burst": max(b) if b else 0,
        "single_frame_events_pct": round(100 * sum(1 for x in b if x == 1) / len(b), 1) if b else 0,
    }


def replay(recv, lines, drop, key):
    """Run the receiver over the recording minus the dropped lines; return what was shown."""
    with tempfile.TemporaryDirectory() as tmp:
        air = Path(tmp) / "air"
        air.write_text("".join(l for l, d in zip(lines, drop) if not d))
        cmd = [recv, "--replay", str(air), "--out", str(Path(tmp) / "out")]
        if key:
            cmd += ["--key-file", str(key)]
        subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        s = json.loads((Path(tmp) / "out" / "summary.json").read_text())
        shown = sorted(
            o["frame_no"]
            for o in map(json.loads, (Path(tmp) / "out" / "objects.jsonl").read_text().splitlines())
            if o.get("passed")
        )
    # A freeze is a run of video frames that were never shown, between two that were.
    freezes = [b - a - 1 for a, b in zip(shown, shown[1:]) if b - a > 1]
    return {
        "radio_frame_loss_pct": 100 * s["radio_frame_loss"],
        "video_frames_not_shown_pct": 100 * s["video_frame_loss"],
        "fec_recovered": s["fec_recovered"],
        "objects_lost": s["objects_lost"],
        "groups_cut": s["groups_cut"],
        "auth_fail": s["auth_fail"],
        "freezes": len(freezes),
        "mean_freeze_ms": 1000 * statistics.mean(freezes) / FPS if freezes else 0.0,
        "worst_freeze_ms": 1000 * max(freezes) / FPS if freezes else 0.0,
    }


def sweep(args):
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    traces = {}
    for r in args.runs:
        try:
            traces[Path(r).name] = trace(r)
        except ValueError as e:
            print(f"not used as a loss trace: {e}", file=sys.stderr)
    with open(out / "traces.csv", "w", newline="") as f:
        rows = [trace_stats(n, m) for n, m in traces.items()]
        w = csv.DictWriter(f, fieldnames=list(rows[0]))
        w.writeheader()
        w.writerows(rows)

    rec = Path(args.rec)
    results = []
    for air in sorted(rec.glob("*.air")):
        name = air.stem
        lines = air.read_text().splitlines(keepends=True)
        key = rec / "key.hex" if name.endswith("enc") else None
        cases = [("iid", f"{100 * p:g}%", p, None) for p in IID_RATES]
        cases += [("radio trace", n, None, m) for n, m in traces.items()]
        for model, label, p, mask in cases:
            seeds = 1 if p == 0.0 else SEEDS
            runs = []
            for seed in range(seeds):
                rng = random.Random(f"{name}/{model}/{label}/{seed}")
                if mask is None:
                    drop = [rng.random() < p for _ in lines]
                else:
                    # The measured pattern, started at a random point and wrapped.
                    at = rng.randrange(len(mask))
                    drop = [mask[(at + i) % len(mask)] for i in range(len(lines))]
                runs.append(replay(args.recv, lines, drop, key))
            row = {"recording": name, "loss_model": model, "case": label, "replays": seeds}
            for k in runs[0]:
                row[k] = round(statistics.mean(r[k] for r in runs), 3)
            row["worst_freeze_ms"] = round(max(r["worst_freeze_ms"] for r in runs), 1)
            results.append(row)
            print(row, file=sys.stderr)
    with open(out / "sweep.csv", "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=list(results[0]))
        w.writeheader()
        w.writerows(results)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    t = sub.add_parser("traces")
    t.add_argument("runs", nargs="+")
    s = sub.add_parser("sweep")
    s.add_argument("--recv", required=True)
    s.add_argument("--rec", required=True)
    s.add_argument("--runs", nargs="+", required=True)
    s.add_argument("--out", required=True)
    args = ap.parse_args()
    if args.cmd == "traces":
        for r in args.runs:
            try:
                print(json.dumps(trace_stats(Path(r).name, trace(r))))
            except ValueError as e:
                print(f"unusable: {e}")
    else:
        sweep(args)


if __name__ == "__main__":
    main()
