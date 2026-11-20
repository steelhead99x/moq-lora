"""Semtech UDP packet forwarder protocol, version 2 (the "legacy" gateway protocol).

Header: version(1)=2 | token(2) | identifier(1) | [gateway EUI(8)] | [JSON]
"""

from __future__ import annotations

import json
import struct

VERSION = 2
PUSH_DATA, PUSH_ACK, PULL_DATA, PULL_RESP, PULL_ACK, TX_ACK = 0, 1, 2, 3, 4, 5


def push_data(token: int, eui: bytes, rxpk: list[dict]) -> bytes:
    body = json.dumps({"rxpk": rxpk}, separators=(",", ":")).encode()
    return struct.pack(">BHB", VERSION, token, PUSH_DATA) + eui + body


def pull_data(token: int, eui: bytes) -> bytes:
    return struct.pack(">BHB", VERSION, token, PULL_DATA) + eui


def ack(token: int, ident: int) -> bytes:
    return struct.pack(">BHB", VERSION, token, ident)


def pull_resp(token: int, txpk: dict) -> bytes:
    body = json.dumps({"txpk": txpk}, separators=(",", ":")).encode()
    return struct.pack(">BHB", VERSION, token, PULL_RESP) + body


def tx_ack(token: int, eui: bytes) -> bytes:
    return struct.pack(">BHB", VERSION, token, TX_ACK) + eui


def parse(pkt: bytes) -> tuple[int, int, bytes | None, dict | None]:
    """Return (identifier, token, gateway EUI or None, JSON body or None)."""
    if len(pkt) < 4 or pkt[0] != VERSION:
        raise ValueError("not a GWMP v2 packet")
    _, token, ident = struct.unpack_from(">BHB", pkt, 0)
    if ident in (PUSH_DATA, PULL_DATA, TX_ACK):
        eui = pkt[4:12]
        rest = pkt[12:]
    else:
        eui = None
        rest = pkt[4:]
    body = json.loads(rest) if rest else None
    return ident, token, eui, body
