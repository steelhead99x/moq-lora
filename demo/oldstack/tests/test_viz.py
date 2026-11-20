"""The infographics and the HTML demo are built from results.json; check they are and stay self-contained."""
import json
import os
import re
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
VIZ = os.path.join(HERE, "..", "viz")


def test_figures_match_results_and_html_is_self_contained():
    subprocess.run([sys.executable, os.path.join(VIZ, "make_figures.py")], check=True, capture_output=True)
    res = json.load(open(os.path.join(HERE, "..", "results", "results.json")))
    v1 = next(r for r in res["paced"] if r["workload"] == "video" and r["loss"] == 0.01)
    svg = open(os.path.join(VIZ, "img", "frames-shown.svg")).read()
    assert f"{v1['old']['decodable_pct']:.1f}%" in svg and f"{v1['moq']['decodable_pct']:.1f}%" in svg
    html = open(os.path.join(VIZ, "index.html")).read()
    assert not re.search(r'(src|href)\s*=\s*["\']https?:', html), "the demo must not load anything from the web"
    assert "<script src" not in html and "@import" not in html
    for name in os.listdir(os.path.join(VIZ, "img")):
        assert "\u2014" not in open(os.path.join(VIZ, "img", name)).read()
