"""Gateway: the job of the Semtech UDP packet forwarder.

Receives LoRa frames from the simulated air, wraps each in a PUSH_DATA (JSON, base64)
to the network server, keeps a PULL_DATA keepalive so the server can reach it, and
transmits PULL_RESP downlinks at the requested concentrator time (tmst), which is how
the RX1 window one second after an uplink is hit.
"""

from __future__ import annotations

import argparse
import base64
import heapq
import json
import os
import select
import socket
import time
from datetime import datetime, timezone

from . import gwmp
from .common import GATEWAY_EUI, touch

T0 = time.monotonic()


def tmst() -> int:
    return int((time.monotonic() - T0) * 1e6) & 0xFFFFFFFF


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--air-port", type=int, required=True)
    ap.add_argument("--ns-port", type=int, required=True)
    ap.add_argument("--down-port", type=int, default=0, help="device downlink air port")
    ap.add_argument("--ready", required=True)
    ap.add_argument("--summary", required=True)
    ap.add_argument("--idle-exit-s", type=float, default=0.0)
    ap.add_argument("--stop", help="exit when this file appears")
    a = ap.parse_args()

    air = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    air.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1 << 22)
    air.bind(("127.0.0.1", a.air_port))
    up = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    up.connect(("127.0.0.1", a.ns_port))
    down_air = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)

    token = 0
    stats = {"rx_frames": 0, "push_data_bytes": 0, "push_acks": 0, "pull_resp": 0,
             "pull_resp_bytes": 0, "downlinks_tx": 0}
    pending: list[tuple[int, int, bytes]] = []  # (tmst due, seq, phy)
    up.send(gwmp.pull_data(token, GATEWAY_EUI))
    last_pull = time.monotonic()
    last_rx = None
    touch(a.ready)
    while True:
        timeout = 0.02
        if pending:
            wait = ((pending[0][0] - tmst()) & 0xFFFFFFFF) / 1e6
            timeout = max(0.0, min(timeout, wait if wait < 2000 else 0.0))
        r, _, _ = select.select([air, up], [], [], timeout)
        if air in r:
            phy = air.recv(512)
            stats["rx_frames"] += 1
            last_rx = time.monotonic()
            token = (token + 1) & 0xFFFF
            rxpk = {"tmst": tmst(), "time": datetime.now(timezone.utc).isoformat(),
                    "chan": 0, "rfch": 0, "freq": 902.3, "stat": 1, "modu": "LORA",
                    "datr": "SF8BW500", "codr": "4/5", "rssi": -60, "lsnr": 9.5,
                    "size": len(phy), "data": base64.b64encode(phy).decode()}
            pkt = gwmp.push_data(token, GATEWAY_EUI, [rxpk])
            stats["push_data_bytes"] += len(pkt)
            up.send(pkt)
        if up in r:
            pkt = up.recv(4096)
            ident, tok, _, body = gwmp.parse(pkt)
            if ident == gwmp.PUSH_ACK:
                stats["push_acks"] += 1
            elif ident == gwmp.PULL_RESP and body:
                stats["pull_resp"] += 1
                stats["pull_resp_bytes"] += len(pkt)
                txpk = body["txpk"]
                phy = base64.b64decode(txpk["data"])
                due = tmst() if txpk.get("imme") else int(txpk["tmst"])
                heapq.heappush(pending, (due, stats["pull_resp"], phy))
                up.send(gwmp.tx_ack(tok, GATEWAY_EUI))
        while pending and ((tmst() - pending[0][0]) & 0xFFFFFFFF) < 0x80000000:
            _, _, phy = heapq.heappop(pending)
            if a.down_port:
                down_air.sendto(phy, ("127.0.0.1", a.down_port))
            stats["downlinks_tx"] += 1
        if time.monotonic() - last_pull > 5:
            up.send(gwmp.pull_data(token, GATEWAY_EUI))
            last_pull = time.monotonic()
        if a.stop and os.path.exists(a.stop):
            break
        if a.idle_exit_s and last_rx and time.monotonic() - last_rx > a.idle_exit_s:
            break
    with open(a.summary, "w") as f:
        json.dump(stats, f)


if __name__ == "__main__":
    main()
