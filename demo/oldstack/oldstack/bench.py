"""Run both stacks on the same payloads and write results/results.{md,json}.

    python -m oldstack.bench            # full run, about four minutes
    python -m oldstack.bench --quick    # short smoke run (used by the tests)
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import shutil
import socket
import subprocess
import sys
import time

from . import airtime, lorawan
from .common import FRAG_HDR, Payload, read_delivered, read_payloads, write_payloads
from . import workload

HERE = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = os.path.dirname(os.path.dirname(HERE))
PY = sys.executable


def free_port() -> int:
    s = socket.socket()
    s.bind(("127.0.0.1", 0))
    p = s.getsockname()[1]
    s.close()
    return p


def wait_file(path: str, procs=(), timeout: float = 10) -> None:
    t = time.monotonic()
    while not os.path.exists(path):
        for p in procs:
            if p.poll() is not None:
                raise RuntimeError(f"{p.args} exited early ({p.returncode})")
        if time.monotonic() - t > timeout:
            raise RuntimeError(f"timed out waiting for {path}")
        time.sleep(0.01)


def wait_port(port: int, timeout: float = 10) -> None:
    t = time.monotonic()
    while time.monotonic() - t < timeout:
        try:
            socket.create_connection(("127.0.0.1", port), 0.2).close()
            return
        except OSError:
            time.sleep(0.02)
    raise RuntimeError(f"nothing listening on {port}")


class Procs:
    """Starts processes and collects their CPU time (user + system) with wait4."""

    def __init__(self, logdir: str):
        self.logdir = logdir
        self.cpu: dict[str, float] = {}
        self.live: dict[str, subprocess.Popen] = {}

    def start(self, name: str, args: list[str]) -> subprocess.Popen:
        err = open(os.path.join(self.logdir, f"{name}.stderr"), "w")
        p = subprocess.Popen(args, cwd=HERE, stdout=err, stderr=err)
        self.live[name] = p
        return p

    def wait(self, name: str, timeout: float = 120) -> None:
        p = self.live.pop(name)
        t = time.monotonic()
        while True:
            pid, status, ru = os.wait4(p.pid, os.WNOHANG)
            if pid:
                p.returncode = os.waitstatus_to_exitcode(status)
                break
            if time.monotonic() - t > timeout:
                p.kill()
                _, status, ru = os.wait4(p.pid, 0)
                p.returncode = -9
                break
            time.sleep(0.01)
        self.cpu[name] = ru.ru_utime + ru.ru_stime
        if p.returncode not in (0, -15):
            raise RuntimeError(f"{name} exited with {p.returncode}, see {self.logdir}/{name}.stderr")

    def stop(self, name: str) -> None:
        self.live[name].terminate()
        self.wait(name, 10)

    def kill_all(self) -> None:
        for p in self.live.values():
            p.kill()


def mosquitto_bin() -> str:
    b = os.environ.get("MOSQUITTO") or shutil.which("mosquitto") or "/usr/sbin/mosquitto"
    if not os.path.exists(b) and not shutil.which(b):
        sys.exit("mosquitto not found: install it (apt install mosquitto, brew install "
                 "mosquitto) or set MOSQUITTO=/path/to/mosquitto")
    return b


def moq_bin() -> str:
    return os.environ.get("MOQ_PATH_BIN") or os.path.join(REPO, "target", "release", "moq-path")


def start_broker(pr: Procs, d: str) -> int:
    port = free_port()
    conf = os.path.join(d, "mosquitto.conf")
    with open(conf, "w") as f:
        f.write(f"listener {port} 127.0.0.1\nallow_anonymous true\npersistence false\n"
                "log_dest stderr\nlog_type error\n")
    pr.start("mosquitto", [mosquitto_bin(), "-c", conf])
    wait_port(port)
    return port


def run_old(pl_path: str, d: str, loss: float, seed: int, unpaced: bool) -> dict:
    pr = Procs(d)
    j = lambda n: os.path.join(d, n)
    frag = any(len(p.data) > lorawan.MAX_FRM_US915_DR4 for p in read_payloads(pl_path))
    try:
        mport = start_broker(pr, d)
        ns_port, air_port = free_port(), free_port()
        pr.start("netserver", [PY, "-m", "oldstack.netserver", "--port", str(ns_port),
                               "--mqtt-port", str(mport), "--ready", j("ns.ready"),
                               "--summary", j("ns.json"), "--stop", j("stop")])
        wait_file(j("ns.ready"), pr.live.values())
        pr.start("gateway", [PY, "-m", "oldstack.gateway", "--air-port", str(air_port),
                             "--ns-port", str(ns_port), "--ready", j("gw.ready"),
                             "--summary", j("gw.json"), "--stop", j("stop")])
        wait_file(j("gw.ready"), pr.live.values())
        pr.start("app", [PY, "-m", "oldstack.app", "--mqtt-port", str(mport),
                         "--out", j("delivered.bin"), "--ready", j("app.ready"),
                         "--summary", j("app.json")] + (["--fragmented"] if frag else []))
        wait_file(j("app.ready"), pr.live.values())
        pr.start("device", [PY, "-m", "oldstack.device", "--air-port", str(air_port),
                            "--payloads", pl_path, "--log", j("send.log"),
                            "--summary", j("tx.json"), "--loss", str(loss),
                            "--seed", str(seed)] + (["--unpaced"] if unpaced else []))
        pr.wait("device", 300)
        pr.wait("app", 300)
        open(j("stop"), "w").close()
        pr.wait("gateway")
        pr.wait("netserver")
        pr.stop("mosquitto")
    finally:
        pr.kill_all()
    tx = json.load(open(j("tx.json")))
    gw = json.load(open(j("gw.json")))
    ns = json.load(open(j("ns.json")))
    tx["backhaul_bytes"] = gw["push_data_bytes"] + ns["mqtt_publish_bytes"]
    tx["cpu_s"] = pr.cpu
    return tx


def run_moq(pl_path: str, d: str, loss: float, seed: int, unpaced: bool) -> dict:
    pr = Procs(d)
    j = lambda n: os.path.join(d, n)
    air = f"127.0.0.1:{free_port()}"
    try:
        pr.start("rx", [moq_bin(), "rx", "--air", air, "--out", j("delivered.bin"),
                        "--summary", j("rx.json"), "--ready", j("rx.ready")])
        wait_file(j("rx.ready"), pr.live.values())
        pr.start("tx", [moq_bin(), "tx", "--payloads", pl_path, "--air", air,
                        "--summary", j("tx.json"), "--log", j("send.log"),
                        "--loss", str(loss), "--seed", str(seed)]
                 + (["--unpaced"] if unpaced else []))
        pr.wait("tx", 300)
        pr.wait("rx", 300)
    finally:
        pr.kill_all()
    tx = json.load(open(j("tx.json")))
    tx["backhaul_bytes"] = 0
    tx["cpu_s"] = pr.cpu
    tx["rx"] = json.load(open(j("rx.json")))
    return tx


def pct(xs: list[float], q: float) -> float | None:
    if not xs:
        return None
    s = sorted(xs)
    return s[min(len(s) - 1, max(0, int(round(q / 100 * len(s) + 0.5)) - 1))]


def analyse(pls: list[Payload], d: str, tx: dict) -> dict:
    sent = {}
    for line in open(os.path.join(d, "send.log")):
        i, t = line.split()
        sent[int(i)] = int(t)
    got = read_delivered(os.path.join(d, "delivered.bin"))
    wrong = sum(1 for pid, _, body in got if pid >= len(pls) or pls[pid].data != body)
    ids = {pid for pid, _, _ in got}
    lat = [(t - sent[pid]) / 1e6 for pid, t, _ in got if pid in sent]
    # decodable: every earlier payload of the same group arrived too (the GOP rule)
    dec, ok = 0, False
    for i, p in enumerate(pls):
        ok = (i in ids) if p.group_start else (ok and i in ids)
        dec += ok
    first = min(sent.values()) if sent else 0
    last = max((t for _, t, _ in got), default=first)
    span = max((last - first) / 1e9, 1e-9)
    n = len(pls)
    cpu = sum(tx["cpu_s"].values())
    return {
        "payloads": n, "delivered": len(ids), "wrong_bytes": wrong,
        "delivered_pct": 100 * len(ids) / n, "decodable_pct": 100 * dec / n,
        "lat_ms": {"p50": pct(lat, 50), "p95": pct(lat, 95), "p99": pct(lat, 99),
                   "max": max(lat) if lat else None},
        "payloads_per_s": len(ids) / span,
        "goodput_kbit_s": sum(len(b) for _, _, b in got) * 8 / span / 1000,
        "air_frames": tx["air_frames"], "air_bytes": tx["air_bytes"],
        "air_frames_dropped": tx["air_frames_dropped"],
        "air_bytes_per_payload": tx["air_bytes"] / n,
        "app_bytes_per_payload": sum(len(p.data) for p in pls) / n,
        "backhaul_bytes_per_payload": tx["backhaul_bytes"] / n,
        "cpu_ms_per_payload": 1000 * cpu / max(len(ids), 1),
        "cpu_s_by_process": {k: round(v, 3) for k, v in tx["cpu_s"].items()},
        "fec_recovered": tx.get("rx", {}).get("fec_recovered"),
    }


def model(pls: list[Payload], moq_tx: dict, secs: float) -> dict:
    """Time on air for the lossless frame streams of each stack (a model)."""
    frag = any(len(p.data) > lorawan.MAX_FRM_US915_DR4 for p in pls)
    lora = []
    for pid, p in enumerate(pls):
        if frag:
            step = lorawan.MAX_FRM_US915_DR4 - FRAG_HDR.size
            lora += [lorawan.OVERHEAD + FRAG_HDR.size + len(p.data[i:i + step])
                     for i in range(0, len(p.data), step)]
        else:
            lora.append(lorawan.OVERHEAD + len(p.data))
    lora_ms = sum(airtime.lora_ms(x) for x in lora)
    fsk_ms = (moq_tx["air_frames"] * airtime.fsk_ms(0)
              + moq_tx["air_bytes"] * 8 * 1000 / 300_000)
    return {
        "old": {"frames": len(lora), "airtime_ms_per_payload": lora_ms / len(pls),
                "channel_busy_pct": 100 * lora_ms / 1000 / secs,
                # a Class A device listens in RX1 (1 s) and RX2 (2 s) after every uplink
                # before it may send again, unless RX1 brings a downlink
                "class_a_send_s": sum(airtime.lora_ms(x) / 1000 + 2.0 for x in lora),
                "radio": "LoRa SF8/500 kHz CR4/5, 8-symbol preamble (US915 DR4)"},
        "workload_s": secs,
        "moq": {"frames": moq_tx["air_frames"], "airtime_ms_per_payload": fsk_ms / len(pls),
                "channel_busy_pct": 100 * fsk_ms / 1000 / secs,
                "radio": "FSK 300 kbit/s, 6-byte preamble, 2-byte sync, CRC-16"},
    }


def cmd_test(d: str, interval_s: float, count: int) -> dict:
    os.makedirs(d, exist_ok=True)
    j = lambda n: os.path.join(d, n)
    out = {}
    # old stack: Class A, the command waits for the next uplink, then RX1 one second later
    pr = Procs(d)
    try:
        mport = start_broker(pr, d)
        ns_port, air_port, down_port = free_port(), free_port(), free_port()
        pr.start("netserver", [PY, "-m", "oldstack.netserver", "--port", str(ns_port),
                               "--mqtt-port", str(mport), "--ready", j("ns.ready"),
                               "--summary", j("ns.json"), "--stop", j("stop")])
        wait_file(j("ns.ready"), pr.live.values())
        pr.start("gateway", [PY, "-m", "oldstack.gateway", "--air-port", str(air_port),
                             "--ns-port", str(ns_port), "--down-port", str(down_port),
                             "--ready", j("gw.ready"), "--summary", j("gw.json"),
                             "--stop", j("stop")])
        wait_file(j("gw.ready"), pr.live.values())
        pr.start("device", [PY, "-m", "oldstack.device", "--heartbeat", "--air-port",
                            str(air_port), "--down-port", str(down_port), "--interval-s",
                            str(interval_s), "--commands", str(count), "--ready",
                            j("dev.ready")])
        wait_file(j("dev.ready"), pr.live.values())
        pr.start("app", [PY, "-m", "oldstack.app", "--mqtt-port", str(mport), "--ready",
                         j("app.ready"), "--summary", j("app.json"), "--commands",
                         str(count), "--interval-s", str(interval_s)])
        pr.wait("app", 30 + count * 4 * interval_s)
        pr.wait("device", 10)
        open(j("stop"), "w").close()
        pr.wait("gateway")
        pr.wait("netserver")
        pr.stop("mosquitto")
    finally:
        pr.kill_all()
    out["old"] = json.load(open(j("app.json")))
    # moq-lora: a command object one way, an ack object back, no receive windows
    pr = Procs(d)
    a, b = f"127.0.0.1:{free_port()}", f"127.0.0.1:{free_port()}"
    try:
        pr.start("echo", [moq_bin(), "echo", "--listen", a, "--reply", b, "--ready",
                          j("echo.ready")])
        wait_file(j("echo.ready"), pr.live.values())
        pr.start("ping", [moq_bin(), "ping", "--to", a, "--listen", b, "--count",
                          str(count), "--summary", j("ping.json")])
        pr.wait("ping", 60)
        pr.wait("echo", 30)
    finally:
        pr.kill_all()
    out["moq"] = json.load(open(j("ping.json")))
    for k in out:
        r = out[k]["rtt_ms"]
        out[k]["p50"], out[k]["p95"], out[k]["max"] = pct(r, 50), pct(r, 95), max(r, default=None)
    out["interval_s"] = interval_s
    # modelled airtime to add for a real radio, one command and one ack each way
    out["airtime_add_ms"] = {
        "old": airtime.lora_ms(lorawan.OVERHEAD + 8, sf=7) + airtime.lora_ms(lorawan.OVERHEAD + 2),
        "moq": 2 * airtime.fsk_ms(12 + 17 + 28 + 8),
    }
    return out


def f(x, nd=1):
    return "n/a" if x is None else f"{x:.{nd}f}"


def write_md(res: dict, path: str) -> None:
    L = []
    m = res["machine"]
    L.append(f"Measured {res['date']} on one machine ({m['cpu']}, {m['cores']} cores, "
             f"Python {m['python']}, mosquitto {m['mosquitto']}). Software path only: the radio "
             "is replaced by localhost UDP. Airtime columns are computed, not measured.\n")
    L.append("### Paced runs (payloads sent on their schedule)\n")
    L.append("| workload | air loss | stack | delivered | decodable (GOP rule) | latency p50 / p95 / p99 ms "
             "| air bytes per payload | backhaul bytes per payload | CPU ms per payload |")
    L.append("|---|---|---|---|---|---|---|---|---|")
    for r in res["paced"]:
        for s, name in (("old", "old stack (Python)"), ("moq", "moq-lora (Rust)")):
            a = r[s]
            lat = a["lat_ms"]
            dec = f"{a['decodable_pct']:.1f}%" if r["workload"] == "video" else "n/a"
            L.append(f"| {r['workload']} | {r['loss']*100:.0f}% | {name} | {a['delivered_pct']:.1f}% "
                     f"| {dec} | {f(lat['p50'],2)} / {f(lat['p95'],2)} / {f(lat['p99'],2)} "
                     f"| {a['air_bytes_per_payload']:.0f} | {a['backhaul_bytes_per_payload']:.0f} "
                     f"| {a['cpu_ms_per_payload']:.2f} |")
    L.append("\n### Unpaced runs (as fast as the software goes, no loss)\n")
    L.append("| workload | stack | delivered | payloads/s | goodput kbit/s | latency p50 / p99 ms |")
    L.append("|---|---|---|---|---|---|")
    for r in res["unpaced"]:
        for s, name in (("old", "old stack (Python)"), ("moq", "moq-lora (Rust)")):
            a = r[s]
            L.append(f"| {r['workload']} | {name} | {a['delivered_pct']:.1f}% | {a['payloads_per_s']:.0f} "
                     f"| {a['goodput_kbit_s']:.0f} | {f(a['lat_ms']['p50'],1)} / {f(a['lat_ms']['p99'],1)} |")
    L.append("\n### Time on air (model, lossless)\n")
    L.append("| workload | stack | radio | frames | airtime per payload ms | channel busy at the offered rate "
             "| time to send it all |")
    L.append("|---|---|---|---|---|---|---|")
    for r in res["paced"]:
        if r["loss"]:
            continue
        for s, name in (("old", "old stack"), ("moq", "moq-lora")):
            a = r["model"][s]
            w = r["model"]["workload_s"]
            if s == "old":
                t = f"{a['class_a_send_s']:.0f} s for {w:.0f} s of traffic (Class A receive windows)"
            else:
                t = f"{w:.0f} s (keeps up)" if a["channel_busy_pct"] < 100 else "falls behind"
            L.append(f"| {r['workload']} | {name} | {a['radio']} | {a['frames']} "
                     f"| {a['airtime_ms_per_payload']:.2f} | {a['channel_busy_pct']:.0f}% | {t} |")
    c = res["command"]
    L.append(f"\n### Command round trip (app to device and back, {len(c['old']['rtt_ms'])} and "
             f"{len(c['moq']['rtt_ms'])} commands)\n")
    L.append("| stack | p50 ms | p95 ms | max ms | timeouts | airtime to add on a real radio ms |")
    L.append("|---|---|---|---|---|---|")
    L.append(f"| old stack, Class A, uplink every {c['interval_s']:.0f} s | {f(c['old']['p50'])} "
             f"| {f(c['old']['p95'])} | {f(c['old']['max'])} | {c['old']['timeouts']} "
             f"| {c['airtime_add_ms']['old']:.1f} |")
    L.append(f"| moq-lora | {f(c['moq']['p50'],2)} | {f(c['moq']['p95'],2)} | {f(c['moq']['max'],2)} "
             f"| {c['moq']['timeouts']} | {c['airtime_add_ms']['moq']:.1f} |")
    open(path, "w").write("\n".join(L) + "\n")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--quick", action="store_true", help="short smoke run")
    ap.add_argument("--runs", default=os.path.join(HERE, "runs"))
    ap.add_argument("--results", default=os.path.join(HERE, "results"))
    ap.add_argument("--loss", type=float, default=0.01)
    ap.add_argument("--rewrite-md", action="store_true", help="redo results.md from results.json")
    a = ap.parse_args()
    if a.rewrite_md:
        write_md(json.load(open(os.path.join(a.results, "results.json"))),
                 os.path.join(a.results, "results.md"))
        return
    os.makedirs(a.runs, exist_ok=True)
    os.makedirs(a.results, exist_ok=True)
    if a.quick:
        loads = {"sensor": workload.sensor(n=60, hz=50), "video": workload.video(secs=6)}
        losses, cmd_n, interval = [0.0, a.loss], 3, 1.0
    else:
        loads = {"sensor": workload.sensor(), "video": workload.video()}
        losses, cmd_n, interval = [0.0, a.loss], 10, 2.0
    mos = subprocess.run([mosquitto_bin(), "-h"], capture_output=True, text=True).stdout.split()
    cpu = next((l.split(":", 1)[1].strip() for l in open("/proc/cpuinfo") if l.startswith("model name")),
               platform.processor()) if os.path.exists("/proc/cpuinfo") else platform.processor()
    res = {"date": time.strftime("%Y-%m-%d"),
           "machine": {"cpu": cpu, "cores": os.cpu_count(), "python": platform.python_version(),
                       "mosquitto": mos[2] if len(mos) > 2 else "?"},
           "paced": [], "unpaced": []}
    for name, pls in loads.items():
        pl_path = os.path.join(a.runs, f"{name}.payloads")
        write_payloads(pl_path, pls)
        secs = pls[-1].offset_us / 1e6 + (pls[1].offset_us / 1e6)
        for unpaced in (False, True):
            for loss in (losses if not unpaced else [0.0]):
                row = {"workload": name, "loss": loss}
                for s, fn in (("old", run_old), ("moq", run_moq)):
                    d = os.path.join(a.runs, f"{name}-{s}-{'unpaced' if unpaced else 'paced'}-{loss}")
                    shutil.rmtree(d, ignore_errors=True)
                    os.makedirs(d)
                    print(f"run {os.path.basename(d)}", flush=True)
                    tx = fn(pl_path, d, loss, 1, unpaced)
                    row[s] = analyse(pls, d, tx)
                    row[s + "_tx"] = tx
                if not unpaced:
                    row["model"] = model(pls, row["moq_tx"], secs)
                res["unpaced" if unpaced else "paced"].append(row)
    print("run command round trip", flush=True)
    res["command"] = cmd_test(os.path.join(a.runs, "command"), interval, cmd_n)
    json.dump(res, open(os.path.join(a.results, "results.json"), "w"), indent=1)
    write_md(res, os.path.join(a.results, "results.md"))
    print(open(os.path.join(a.results, "results.md")).read())


if __name__ == "__main__":
    main()
