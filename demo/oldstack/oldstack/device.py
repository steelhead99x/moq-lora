"""End device: turns payloads into LoRaWAN uplinks and puts them on the simulated air.

Payloads that fit one uplink (<= 242 B at US915 DR4) are sent as-is and the app uses
FCnt as the payload id. Larger payloads are cut by the application into fragments with a
4-byte header (payload id, index, count), the usual do-it-yourself approach, since
LoRaWAN has no uplink fragmentation of its own.
"""

from __future__ import annotations

import argparse
import json
import socket
import time

from . import lorawan
from .common import (APPSKEY, DEVADDR, FPORT_ACK, FPORT_DATA, FRAG_HDR, NWKSKEY, Rng,
                     now_ns, read_payloads, touch)


class Device:
    def __init__(self, air: tuple[str, int], loss: float, seed: int):
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.sock.connect(air)
        self.fcnt = 0
        self.rng = Rng(seed)
        self.loss = loss
        self.frames = self.bytes = self.dropped = 0

    def uplink(self, fport: int, data: bytes) -> None:
        phy = lorawan.build(lorawan.UNCONFIRMED_UP, DEVADDR, self.fcnt, fport, data, NWKSKEY, APPSKEY)
        self.fcnt += 1
        self.frames += 1
        self.bytes += len(phy)
        if self.loss > 0 and self.rng.next_f64() < self.loss:
            self.dropped += 1
            return
        self.sock.send(phy)


def chunks(pid: int, data: bytes, maxfrm: int) -> list[bytes]:
    if len(data) <= maxfrm:
        return [data]
    step = maxfrm - FRAG_HDR.size
    parts = [data[i : i + step] for i in range(0, len(data), step)]
    return [FRAG_HDR.pack(pid & 0xFFFF, i, len(parts)) + c for i, c in enumerate(parts)]


def run_payloads(a) -> None:
    pls = read_payloads(a.payloads)
    frag = any(len(p.data) > lorawan.MAX_FRM_US915_DR4 for p in pls)
    dev = Device(("127.0.0.1", a.air_port), a.loss, a.seed)
    t0 = time.monotonic()
    with open(a.log, "w") as log:
        for pid, p in enumerate(pls):
            if not a.unpaced:
                due = t0 + p.offset_us / 1e6
                d = due - time.monotonic()
                if d > 0:
                    time.sleep(d)
            log.write(f"{pid} {now_ns()}\n")
            if frag:
                for c in chunks(pid, p.data, lorawan.MAX_FRM_US915_DR4):
                    dev.uplink(FPORT_DATA, c)
            else:
                dev.uplink(FPORT_DATA, p.data)
    wall = time.monotonic() - t0
    with open(a.summary, "w") as f:
        json.dump({"stack": "old", "payloads": len(pls), "fragmented": frag,
                   "air_frames": dev.frames, "air_bytes": dev.bytes,
                   "air_frames_dropped": dev.dropped, "send_wall_s": round(wall, 4)}, f)


def run_heartbeat(a) -> None:
    """Class A device for the command test: a 12-byte uplink every --interval-s, and an
    ack uplink right after any downlink command it receives."""
    dev = Device(("127.0.0.1", a.air_port), 0.0, a.seed)
    rx = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    rx.bind(("127.0.0.1", a.down_port))
    rx.settimeout(0.005)
    touch(a.ready)
    fcnt_down = None
    nxt = time.monotonic()
    got = 0
    while got < a.commands:
        if time.monotonic() >= nxt:
            dev.uplink(FPORT_DATA, bytes(12))
            nxt += a.interval_s
        try:
            phy = rx.recv(512)
        except socket.timeout:
            continue
        f = lorawan.parse(phy)
        if f.devaddr != DEVADDR:
            continue
        fcnt_down = lorawan.full_fcnt(fcnt_down, f.fcnt)
        _, body = lorawan.verify_and_decrypt(phy, NWKSKEY, APPSKEY, fcnt_down)
        got += 1
        dev.uplink(FPORT_ACK, body[:2])  # ack carries the command id


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--air-port", type=int, required=True)
    ap.add_argument("--payloads")
    ap.add_argument("--log")
    ap.add_argument("--summary")
    ap.add_argument("--loss", type=float, default=0.0)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--unpaced", action="store_true")
    ap.add_argument("--heartbeat", action="store_true")
    ap.add_argument("--down-port", type=int)
    ap.add_argument("--interval-s", type=float, default=2.0)
    ap.add_argument("--commands", type=int, default=10)
    ap.add_argument("--ready")
    a = ap.parse_args()
    (run_heartbeat if a.heartbeat else run_payloads)(a)


if __name__ == "__main__":
    main()
