import base64

import pytest

from oldstack import airtime, gwmp, lorawan
from oldstack.common import Rng, read_payloads, write_payloads
from oldstack.device import chunks
from oldstack import workload

# Public test frame from the lora-packet project (Node.js LoRaWAN decoder)
VEC = bytes.fromhex("40F17DBE4900020001954378762B11FF0D")
NWK = bytes.fromhex("44024241ed4ce9a68c6a8bc055233fd3")
APP = bytes.fromhex("ec925802ae430ca77fd3dd73cb2cc588")


def test_known_frame_decrypts_and_rebuilds():
    f, plain = lorawan.verify_and_decrypt(VEC, NWK, APP, 2)
    assert (f.devaddr, f.fcnt, f.fport, plain) == (0x49BE7DF1, 2, 1, b"test")
    assert lorawan.build(lorawan.UNCONFIRMED_UP, f.devaddr, 2, 1, b"test", NWK, APP) == VEC


def test_tampered_frame_fails_mic():
    bad = bytearray(VEC)
    bad[10] ^= 1
    with pytest.raises(ValueError):
        lorawan.verify_and_decrypt(bytes(bad), NWK, APP, 2)


def test_downlink_roundtrip_and_overhead():
    phy = lorawan.build(lorawan.UNCONFIRMED_DOWN, 1, 7, 10, b"cmd", NWK, APP)
    assert len(phy) == lorawan.OVERHEAD + 3
    f, plain = lorawan.verify_and_decrypt(phy, NWK, APP, 7)
    assert f.downlink and plain == b"cmd"


def test_fcnt_rollover():
    assert lorawan.full_fcnt(None, 5) == 5
    assert lorawan.full_fcnt(0xFFFE, 1) == 0x10001
    assert lorawan.full_fcnt(0x10005, 6) == 0x10006


def test_gwmp_push_data_roundtrip():
    eui = bytes(8)
    pkt = gwmp.push_data(9, eui, [{"data": base64.b64encode(VEC).decode()}])
    ident, tok, e, body = gwmp.parse(pkt)
    assert (ident, tok, e) == (gwmp.PUSH_DATA, 9, eui)
    assert base64.b64decode(body["rxpk"][0]["data"]) == VEC
    assert gwmp.parse(gwmp.ack(9, gwmp.PUSH_ACK))[:2] == (gwmp.PUSH_ACK, 9)


def test_fragments_fit_and_reassemble():
    data = bytes(range(256)) * 20
    parts = chunks(3, data, lorawan.MAX_FRM_US915_DR4)
    assert all(len(p) <= lorawan.MAX_FRM_US915_DR4 for p in parts)
    assert b"".join(p[4:] for p in parts) == data
    assert chunks(3, b"small", 242) == [b"small"]


def test_airtime_matches_known_values():
    # 255 B at SF7/125 kHz is about 400 ms; moq-lora's 255 B FSK frame about 7 ms
    assert airtime.lora_ms(255, sf=7, bw=125e3) == pytest.approx(399.6, abs=0.1)
    assert airtime.fsk_ms(255) == pytest.approx(7.09, abs=0.01)


def test_rng_matches_rust():
    r = Rng(1)
    assert r.next_f64() == 0.28083505005035947
    assert r.next_f64() == 0.6711372530266764


def test_payload_file_roundtrip(tmp_path):
    pls = workload.video(secs=1)
    p = tmp_path / "w.payloads"
    write_payloads(str(p), pls)
    assert read_payloads(str(p)) == pls
