"""The shared workloads. Both stacks read the same payload file byte for byte."""

from __future__ import annotations

import random

from .common import Payload


def sensor(n: int = 400, hz: float = 20.0, size: int = 12, seed: int = 7) -> list[Payload]:
    """Small telemetry readings, the traffic LoRaWAN was designed for."""
    r = random.Random(seed)
    return [Payload(int(i * 1e6 / hz), i % 64 == 0, r.randbytes(size)) for i in range(n)]


def video(secs: int = 20, fps: int = 15, gop: int = 15, key: int = 4500, delta: int = 1300,
          seed: int = 11) -> list[Payload]:
    """Access-unit sizes shaped like moq-lora's 180 kbit/s, GOP 15 stream (one keyframe
    then 14 smaller frames per second). Random bytes, not real H.264."""
    r = random.Random(seed)
    out = []
    for i in range(secs * fps):
        k = i % gop == 0
        base = key if k else delta
        size = max(64, int(r.gauss(base, base * 0.15)))
        out.append(Payload(int(i * 1e6 / fps), k, r.randbytes(size)))
    return out
