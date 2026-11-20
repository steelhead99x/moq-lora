"""Network server: the job ChirpStack or The Things Stack does, cut down to one device.

GWMP in from the gateway: ack it, check the MIC and frame counter, decrypt, and publish
the uplink as JSON on MQTT (topic and fields in the style of ChirpStack v4). Downlink
commands come in on MQTT, wait in a queue, and go out as a PULL_RESP scheduled for the
device's RX1 window: one second after its next uplink (LoRaWAN Class A).
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import queue
import socket
from datetime import datetime, timezone

import paho.mqtt.client as mqtt

from . import gwmp, lorawan
from .common import (APPSKEY, DEVADDR, DEVEUI, DOWN_TOPIC, NWKSKEY, UP_TOPIC, touch)

RX1_DELAY_US = 1_000_000  # LoRaWAN RECEIVE_DELAY1


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--port", type=int, required=True)
    ap.add_argument("--mqtt-port", type=int, required=True)
    ap.add_argument("--ready", required=True)
    ap.add_argument("--summary", required=True)
    ap.add_argument("--stop", required=True)
    a = ap.parse_args()

    downq: queue.Queue[dict] = queue.Queue()
    cli = mqtt.Client(mqtt.CallbackAPIVersion.VERSION2, client_id="netserver")
    cli.on_message = lambda c, u, m: downq.put(json.loads(m.payload))
    cli.connect("127.0.0.1", a.mqtt_port)
    cli.subscribe(DOWN_TOPIC, qos=0)
    cli.loop_start()

    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_RCVBUF, 1 << 22)
    sock.bind(("127.0.0.1", a.port))
    sock.settimeout(0.05)
    gw_pull_addr = None
    last_fcnt = None
    fcnt_down = 0
    st = {"uplinks": 0, "mic_fail": 0, "replays": 0, "mqtt_publish_bytes": 0,
          "downlinks": 0, "pull_resp_bytes": 0}
    touch(a.ready)
    while not os.path.exists(a.stop):
        try:
            pkt, addr = sock.recvfrom(65535)
        except socket.timeout:
            continue
        ident, token, eui, body = gwmp.parse(pkt)
        if ident == gwmp.PULL_DATA:
            gw_pull_addr = addr
            sock.sendto(gwmp.ack(token, gwmp.PULL_ACK), addr)
            continue
        if ident != gwmp.PUSH_DATA:
            continue
        sock.sendto(gwmp.ack(token, gwmp.PUSH_ACK), addr)
        for rx in (body or {}).get("rxpk", []):
            phy = base64.b64decode(rx["data"])
            f = lorawan.parse(phy)
            if f.devaddr != DEVADDR or f.mtype != lorawan.UNCONFIRMED_UP:
                continue
            fc = lorawan.full_fcnt(last_fcnt, f.fcnt)
            if last_fcnt is not None and fc <= last_fcnt:
                st["replays"] += 1
                continue
            try:
                _, plain = lorawan.verify_and_decrypt(phy, NWKSKEY, APPSKEY, fc)
            except ValueError:
                st["mic_fail"] += 1
                continue
            last_fcnt = fc
            st["uplinks"] += 1
            ev = {
                "time": datetime.now(timezone.utc).isoformat(),
                "deviceInfo": {"applicationId": "demo", "devEui": DEVEUI},
                "devAddr": f"{DEVADDR:08x}",
                "fCnt": fc,
                "fPort": f.fport,
                "data": base64.b64encode(plain).decode(),
                "rxInfo": [{"gatewayId": eui.hex() if eui else "", "rssi": rx.get("rssi"),
                            "snr": rx.get("lsnr")}],
                "txInfo": {"frequency": int(rx["freq"] * 1e6),
                           "modulation": {"lora": {"bandwidth": 500000,
                                                   "spreadingFactor": 8,
                                                   "codeRate": "CR_4_5"}}},
            }
            msg = json.dumps(ev, separators=(",", ":")).encode()
            st["mqtt_publish_bytes"] += len(msg) + len(UP_TOPIC)
            cli.publish(UP_TOPIC, msg, qos=0)
            # Class A: a queued command can only go out in this uplink's RX1 window
            if gw_pull_addr is not None and not downq.empty():
                cmd = downq.get()
                data = base64.b64decode(cmd["data"])
                phy_d = lorawan.build(lorawan.UNCONFIRMED_DOWN, DEVADDR, fcnt_down,
                                      int(cmd.get("fPort", 10)), data, NWKSKEY, APPSKEY)
                fcnt_down += 1
                txpk = {"imme": False, "tmst": (rx["tmst"] + RX1_DELAY_US) & 0xFFFFFFFF,
                        "freq": 923.3, "rfch": 0, "powe": 20, "modu": "LORA",
                        "datr": "SF7BW500", "codr": "4/5", "ipol": True,
                        "size": len(phy_d), "data": base64.b64encode(phy_d).decode()}
                resp = gwmp.pull_resp(token, txpk)
                st["downlinks"] += 1
                st["pull_resp_bytes"] += len(resp)
                sock.sendto(resp, gw_pull_addr)
    cli.loop_stop()
    cli.disconnect()
    with open(a.summary, "w") as fh:
        json.dump(st, fh)


if __name__ == "__main__":
    main()
