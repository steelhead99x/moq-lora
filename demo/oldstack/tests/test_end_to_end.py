"""Both stacks, real processes, a short workload: every payload arrives, byte for byte."""

import os
import shutil

import pytest

from oldstack import bench, workload
from oldstack.common import read_delivered, write_payloads

pytestmark = pytest.mark.skipif(
    not os.path.exists(bench.moq_bin()) or not (os.environ.get("MOSQUITTO") or shutil.which("mosquitto")),
    reason="needs target/release/moq-path (cargo build --release) and mosquitto",
)


@pytest.mark.parametrize("name", ["sensor", "video"])
def test_both_stacks_deliver_identical_payloads(tmp_path, name):
    pls = workload.sensor(n=40, hz=100) if name == "sensor" else workload.video(secs=1)
    pl = str(tmp_path / "w.payloads")
    write_payloads(pl, pls)
    got = {}
    for s, fn in (("old", bench.run_old), ("moq", bench.run_moq)):
        d = tmp_path / s
        d.mkdir()
        fn(pl, str(d), 0.0, 1, False)
        got[s] = {pid: body for pid, _, body in read_delivered(str(d / "delivered.bin"))}
    want = {i: p.data for i, p in enumerate(pls)}
    assert got["old"] == want
    assert got["moq"] == want


def test_loss_is_reported_not_hidden(tmp_path):
    pls = workload.video(secs=1)
    pl = str(tmp_path / "w.payloads")
    write_payloads(pl, pls)
    for s, fn in (("old", bench.run_old), ("moq", bench.run_moq)):
        d = tmp_path / s
        d.mkdir()
        tx = fn(pl, str(d), 0.05, 3, False)
        assert tx["air_frames_dropped"] > 0
        res = bench.analyse(pls, str(d), tx)
        assert res["wrong_bytes"] == 0  # whatever arrives is exact
