"""Application: a Python service on paho-mqtt, the usual end of a LoRaWAN deployment.

Subscribes to the device's uplink events, base64-decodes them, puts fragmented payloads
back together, and writes every delivered payload with its arrival time. In command
mode it also publishes downlink commands and times the device's ack.
"""

from __future__ import annotations

import argparse
import base64
import json
import random
import struct
import threading
import time

import paho.mqtt.client as mqtt

from .common import (DOWN_TOPIC, FPORT_ACK, FPORT_DATA, FRAG_HDR, UP_TOPIC, now_ns, touch,
                     write_delivered)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--mqtt-port", type=int, required=True)
    ap.add_argument("--out")
    ap.add_argument("--ready", required=True)
    ap.add_argument("--summary", required=True)
    ap.add_argument("--fragmented", action="store_true")
    ap.add_argument("--idle-exit-s", type=float, default=1.5)
    ap.add_argument("--commands", type=int, default=0)
    ap.add_argument("--seed", type=int, default=1)
    ap.add_argument("--interval-s", type=float, default=2.0)
    a = ap.parse_args()

    lock = threading.Lock()
    out = open(a.out, "wb") if a.out else None
    parts: dict[int, dict[int, bytes]] = {}
    st = {"events": 0, "delivered": 0, "bytes_in": 0, "incomplete": 0}
    last = [None]
    acks: dict[int, int] = {}
    ack_evt = threading.Event()

    def on_msg(c, u, m):
        t = now_ns()
        ev = json.loads(m.payload)
        data = base64.b64decode(ev["data"])
        with lock:
            st["events"] += 1
            st["bytes_in"] += len(m.payload) + len(m.topic)
            last[0] = time.monotonic()
            if ev["fPort"] == FPORT_ACK:
                acks[struct.unpack("<H", data[:2])[0]] = t
                ack_evt.set()
                return
            if ev["fPort"] != FPORT_DATA or out is None:
                return
            if not a.fragmented:
                write_delivered(out, ev["fCnt"], t, data)
                st["delivered"] += 1
                return
            pid, idx, n = FRAG_HDR.unpack_from(data)
            got = parts.setdefault(pid, {})
            got[idx] = data[FRAG_HDR.size:]
            if len(got) == n:
                write_delivered(out, pid, t, b"".join(got[i] for i in range(n)))
                st["delivered"] += 1
                del parts[pid]

    cli = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id="app")
    cli.on_message = on_msg
    subscribed = threading.Event()
    cli.on_subscribe = lambda *x: subscribed.set()
    cli.connect("127.0.0.1", a.mqtt_port)
    cli.subscribe(UP_TOPIC, qos=0)
    cli.loop_start()
    subscribed.wait(5)
    touch(a.ready)

    rtts, timeouts = [], 0
    if a.commands:
        rnd = random.Random(a.seed)
        time.sleep(a.interval_s * 1.5)  # let the device's uplinks start
        for cid in range(a.commands):
            ack_evt.clear()
            t = now_ns()
            body = {"fPort": 10, "data": base64.b64encode(struct.pack("<H", cid) + bytes(6)).decode()}
            cli.publish(DOWN_TOPIC, json.dumps(body), qos=0)
            deadline = time.monotonic() + 3 * a.interval_s + 5
            while cid not in acks and time.monotonic() < deadline:
                ack_evt.wait(0.05)
            if cid in acks:
                rtts.append((acks[cid] - t) / 1e6)
            else:
                timeouts += 1
            # land the next command at a random point in the uplink cycle
            time.sleep(rnd.uniform(0, a.interval_s))
    else:
        while True:
            time.sleep(0.05)
            with lock:
                if last[0] and time.monotonic() - last[0] > a.idle_exit_s:
                    break
    cli.loop_stop()
    cli.disconnect()
    if out:
        out.close()
    st["incomplete"] = len(parts)
    if a.commands:
        st.update({"count": a.commands, "timeouts": timeouts, "rtt_ms": rtts})
    with open(a.summary, "w") as f:
        json.dump(st, f)


if __name__ == "__main__":
    main()
