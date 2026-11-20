"""Shared bits: payload files, the loss generator, demo keys and logs."""

from __future__ import annotations

import os
import struct
import time
from dataclasses import dataclass

# Demo-only ABP session. Never use published keys for anything real.
DEVADDR = 0x26011BDA
DEVEUI = "70b3d57ed0000001"
GATEWAY_EUI = bytes.fromhex("aa555a0000000001")
NWKSKEY = bytes(range(16))
APPSKEY = bytes(range(16, 32))
APP_ID = "demo"
UP_TOPIC = f"application/{APP_ID}/device/{DEVEUI}/event/up"
DOWN_TOPIC = f"application/{APP_ID}/device/{DEVEUI}/command/down"

FPORT_DATA = 1
FPORT_ACK = 2
FRAG_HDR = struct.Struct("<HBB")  # payload id, fragment index, fragment count


@dataclass
class Payload:
    offset_us: int
    group_start: bool
    data: bytes


def write_payloads(path: str, pls: list[Payload]) -> None:
    with open(path, "wb") as f:
        for p in pls:
            f.write(struct.pack("<IBI", p.offset_us, int(p.group_start), len(p.data)))
            f.write(p.data)


def read_payloads(path: str) -> list[Payload]:
    b = open(path, "rb").read()
    out, i = [], 0
    while i < len(b):
        off, fl, n = struct.unpack_from("<IBI", b, i)
        i += 9
        out.append(Payload(off, bool(fl & 1), b[i : i + n]))
        i += n
    return out


def write_delivered(f, pid: int, t_ns: int, body: bytes) -> None:
    f.write(struct.pack("<IQI", pid, t_ns, len(body)))
    f.write(body)


def read_delivered(path: str) -> list[tuple[int, int, bytes]]:
    b = open(path, "rb").read() if os.path.exists(path) else b""
    out, i = [], 0
    while i < len(b):
        pid, t, n = struct.unpack_from("<IQI", b, i)
        i += 16
        out.append((pid, t, b[i : i + n]))
        i += n
    return out


class Rng:
    """xorshift64*, the same generator as the Rust side (moq-path)."""

    def __init__(self, seed: int):
        self.s = max(seed, 1) & 0xFFFFFFFFFFFFFFFF

    def next_f64(self) -> float:
        x = self.s
        x ^= x >> 12
        x ^= (x << 25) & 0xFFFFFFFFFFFFFFFF
        x ^= x >> 27
        self.s = x
        return (((x * 0x2545F4914F6CDD1D) & 0xFFFFFFFFFFFFFFFF) >> 11) / float(1 << 53)


def now_ns() -> int:
    return time.time_ns()


def touch(path: str) -> None:
    with open(path, "w") as f:
        f.write("ok\n")
