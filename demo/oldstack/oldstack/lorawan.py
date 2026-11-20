"""LoRaWAN 1.0.x data frames: build, parse, encrypt and MIC (AES-128, AES-CMAC).

Only what the demo needs: unconfirmed data up and down with ABP session keys, no
FOpts, no MAC commands. Layout of a PHYPayload:

    MHDR(1) | DevAddr(4, LE) | FCtrl(1) | FCnt(2, LE) | FPort(1) | FRMPayload(N) | MIC(4)

so a data frame costs 13 bytes on top of the application payload.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

from cryptography.hazmat.primitives.ciphers import Cipher, algorithms, modes
from cryptography.hazmat.primitives.cmac import CMAC

UNCONFIRMED_UP = 0x40
UNCONFIRMED_DOWN = 0x60
OVERHEAD = 13
# US915 DR4 (SF8/500 kHz): max FRMPayload with no FOpts (LoRaWAN Regional Parameters)
MAX_FRM_US915_DR4 = 242


def _aes_ecb(key: bytes, block: bytes) -> bytes:
    enc = Cipher(algorithms.AES(key), modes.ECB()).encryptor()
    return enc.update(block) + enc.finalize()


def crypt_frm(key: bytes, devaddr: int, fcnt: int, downlink: bool, data: bytes) -> bytes:
    """Encrypt or decrypt FRMPayload (the same XOR keystream both ways)."""
    out = bytearray()
    for i in range(0, len(data), 16):
        a = struct.pack("<B4xBIIBB", 0x01, int(downlink), devaddr, fcnt, 0, i // 16 + 1)
        s = _aes_ecb(key, a)
        out.extend(x ^ y for x, y in zip(data[i : i + 16], s))
    return bytes(out)


def mic(key: bytes, devaddr: int, fcnt: int, downlink: bool, msg: bytes) -> bytes:
    b0 = struct.pack("<B4xBIIBB", 0x49, int(downlink), devaddr, fcnt, 0, len(msg))
    c = CMAC(algorithms.AES(key))
    c.update(b0 + msg)
    return c.finalize()[:4]


@dataclass
class Frame:
    mtype: int
    devaddr: int
    fctrl: int
    fcnt: int
    fport: int | None
    frm: bytes  # still encrypted
    mic: bytes

    @property
    def downlink(self) -> bool:
        return self.mtype in (UNCONFIRMED_DOWN, 0xA0)


def build(mhdr: int, devaddr: int, fcnt: int, fport: int, payload: bytes,
          nwkskey: bytes, appskey: bytes) -> bytes:
    downlink = mhdr in (UNCONFIRMED_DOWN, 0xA0)
    enc = crypt_frm(appskey, devaddr, fcnt, downlink, payload)
    msg = struct.pack("<BIBH", mhdr, devaddr, 0, fcnt & 0xFFFF) + bytes([fport]) + enc
    return msg + mic(nwkskey, devaddr, fcnt, downlink, msg)


def parse(phy: bytes) -> Frame:
    if len(phy) < 12:
        raise ValueError("frame too short")
    mhdr, devaddr, fctrl, fcnt = struct.unpack_from("<BIBH", phy, 0)
    fopts_len = fctrl & 0x0F
    body = phy[8 + fopts_len : -4]
    fport = body[0] if body else None
    return Frame(mhdr & 0xE0, devaddr, fctrl, fcnt, fport, body[1:], phy[-4:])


def verify_and_decrypt(phy: bytes, nwkskey: bytes, appskey: bytes, fcnt32: int) -> tuple[Frame, bytes]:
    """Check the MIC with the full 32-bit counter, then decrypt. Raises on a bad MIC."""
    f = parse(phy)
    if mic(nwkskey, f.devaddr, fcnt32, f.downlink, phy[:-4]) != f.mic:
        raise ValueError("bad MIC")
    return f, crypt_frm(appskey, f.devaddr, fcnt32, f.downlink, f.frm)


def full_fcnt(last: int | None, fcnt16: int) -> int:
    """Rebuild the 32-bit frame counter from the 16 bits on air."""
    if last is None:
        return fcnt16
    base = last & ~0xFFFF
    cand = base | fcnt16
    if cand < last - 0x8000:
        cand += 0x10000
    return cand
