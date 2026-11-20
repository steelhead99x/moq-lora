"""Time on air, from the Semtech formulas. This is a model, not a measurement."""

from __future__ import annotations

import math


def lora_ms(pl: int, sf: int = 8, bw: float = 500e3, cr: int = 1, preamble: int = 8,
            crc: bool = True, implicit: bool = False) -> float:
    """LoRa packet time on air (SX1276 datasheet / AN1200.13). Defaults: US915 DR4."""
    tsym = (2 ** sf) / bw * 1000
    de = 1 if tsym > 16 else 0
    n = 8 + max(math.ceil((8 * pl - 4 * sf + 28 + 16 * int(crc) - 20 * int(implicit))
                          / (4 * (sf - 2 * de))) * (cr + 4), 0)
    return (preamble + 4.25) * tsym + n * tsym


def fsk_ms(pl: int, bitrate: int = 300_000, preamble: int = 6, sync: int = 2) -> float:
    """FSK packet: preamble, sync word, length byte, payload, CRC-16 (moq-lora's profile)."""
    return (preamble + sync + 1 + pl + 2) * 8 * 1000 / bitrate
