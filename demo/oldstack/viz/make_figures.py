#!/usr/bin/env python3
"""Build the infographics (img/*.svg), the replay data and viz/index.html from the measured results.

Every number in the figures is read from ../results/results.json (and, for the replay, from the
per-payload logs in ../runs when they exist; otherwise the committed replay.json is reused).
Nothing is typed in by hand, so the figures cannot drift from the results.

    python3 viz/make_figures.py
"""
from __future__ import annotations

import json
import math
import os
import sys
from xml.sax.saxutils import escape

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
sys.path.insert(0, ROOT)
IMG = os.path.join(HERE, "img")
RES = json.load(open(os.path.join(ROOT, "results", "results.json")))

OLD, MOQ = "#d9822b", "#1b9aaa"

STYLE = """<style>
:root{--bg:#ffffff;--fg:#1d2330;--mut:#5b6475;--grid:#d9dde5;--box:#f3f5f9;--line:#9aa3b5;--old:#d9822b;--moq:#1b9aaa;--bad:#c8402f;--ok:#2e9b57;--warn:#b8860b}
@media (prefers-color-scheme: dark){:root{--bg:#10141c;--fg:#e8ecf4;--mut:#9aa5ba;--grid:#2a3242;--box:#1a2130;--line:#56627a;--old:#f0a04b;--moq:#43c3d2;--bad:#ef6f5e;--ok:#5fcf86;--warn:#e0b84a}}
text{font-family:-apple-system,"Segoe UI",Helvetica,Arial,sans-serif;fill:var(--fg)}
.bg{fill:var(--bg)}.box{fill:var(--box);stroke:var(--line);stroke-width:1.2}
.t{font-size:26px;font-weight:700}.st{font-size:15px;fill:var(--mut)}.h{font-size:16px;font-weight:700}
.b{font-size:14px}.s{font-size:12px;fill:var(--mut)}.xs{font-size:11px;fill:var(--mut)}.v{font-size:15px;font-weight:700}
.old{fill:var(--old)}.moq{fill:var(--moq)}.gl{stroke:var(--grid);stroke-width:1}.ln{stroke:var(--line);stroke-width:1.5;fill:none}
.v,.h{paint-order:stroke;stroke:var(--bg);stroke-width:5px;stroke-linejoin:round}
.badge{font-size:12px;font-weight:700;fill:var(--bg)}
</style>"""

W = 960


def f(x, n=1):
    return f"{x:.{n}f}"


def num(x):
    """Plain human number: 1693 -> 1,693; 43.7 -> 43.7; 0.19 -> 0.19."""
    if x >= 100:
        return f"{x:,.0f}"
    if x >= 10:
        return f"{x:.1f}"
    if x >= 1:
        return f"{x:.2f}".rstrip("0").rstrip(".")
    return f"{x:.2f}"


class S:
    def __init__(self, h, title, sub, desc):
        self.h = h
        self.p = []
        self.late = []
        self.title, self.desc = title, desc
        self.add(f'<rect class="bg" width="{W}" height="{h}"/>')
        self.text(30, 44, title, "t")
        self.text(30, 68, sub, "st")

    def add(self, s):
        self.p.append(s)

    def text(self, x, y, s, cls="b", anchor="start", extra=""):
        self.add(f'<text x="{x}" y="{y}" class="{cls}" text-anchor="{anchor}" {extra}>{escape(s)}</text>')

    def lines(self, x, y, ss, cls="s", dy=15, anchor="start"):
        for i, s in enumerate(ss):
            self.text(x, y + i * dy, s, cls, anchor)

    def rect(self, x, y, w, h, cls="box", rx=8, extra=""):
        self.add(f'<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{rx}" class="{cls}" {extra}/>')

    def line(self, x1, y1, x2, y2, cls="gl", extra=""):
        self.add(f'<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" class="{cls}" {extra}/>')

    def arrow(self, x1, y, x2):
        self.add(f'<path d="M{x1} {y} L{x2-7} {y}" class="ln"/><path d="M{x2-9} {y-5} L{x2} {y} L{x2-9} {y+5}" class="ln"/>')

    def save(self, name):
        svg = (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {W} {self.h}" width="{W}" height="{self.h}" '
               f'role="img" aria-label="{escape(self.title)}"><title>{escape(self.title)}</title>'
               f'<desc>{escape(self.desc)}</desc>{STYLE}' + "".join(self.p) + "".join(self.late) + "</svg>\n")
        open(os.path.join(IMG, name), "w").write(svg)
        return name

    def foot(self, lines):
        y = self.h - 12 - 14 * (len(lines) - 1)
        self.lines(30, y, lines, "xs", 14)


def pick(workload, loss):
    return next(r for r in RES["paced"] if r["workload"] == workload and r["loss"] == loss)


V0, V1, S0, S1 = pick("video", 0.0), pick("video", 0.01), pick("sensor", 0.0), pick("sensor", 0.01)
LOSS = V1["loss"]
N_FR = V1["old"]["payloads"]
SECS = V1["model"]["workload_s"]
CMD = RES["command"]
DATE = RES["date"]
PROV = f"Measured {DATE}, one run, simulated radio on localhost. Source: results/results.json"


# ---------------------------------------------------------------- (a) stack comparison
def fig_stack():
    s = S(430, "Two ways to carry the same data over a radio",
          "The old LoRaWAN path has six steps. moq-lora has four.",
          "Left to right path from device to app for the old stack and for moq-lora.")
    bw, gap = 128, 24
    # old row
    s.text(30, 104, "Old stack (LoRaWAN style, built here in Python)", "h")
    old = [("Device", "sends small", "LoRaWAN frames"),
           ("LoRa radio", "slow, long range", "(US915 DR4)"),
           ("Gateway", "wraps each frame", "in JSON + base64"),
           ("Network server", "checks it, wraps", "it again"),
           ("MQTT broker", "Mosquitto passes", "it on"),
           ("App", "puts the pieces", "back together")]
    for i, (a, b, c) in enumerate(old):
        x = 30 + i * (bw + gap - 4)
        s.rect(x, 118, bw, 92, extra=f'style="stroke:var(--old)"')
        s.text(x + bw / 2, 144, a, "h", "middle")
        s.text(x + bw / 2, 166, b, "s", "middle")
        s.text(x + bw / 2, 181, c, "s", "middle")
        s.add(f'<circle cx="{x+14}" cy="130" r="0"/>')
        if i < len(old) - 1:
            s.arrow(x + bw + 2, 164, x + bw + gap - 4)
    ob = V1["old"]["backhaul_bytes_per_payload"]
    s.text(30, 232, f"Behind the radio: about {ob:,.0f} bytes of wrapping and hops per video frame, across 3 programs.", "s")
    # moq row
    s.text(30, 270, "moq-lora (this repo)", "h")
    moq = [("Pi + camera", "cuts video into", "groups and objects"),
           ("FSK radio", "300 kbit/s, much", "faster than LoRa"),
           ("CatWAN stick", "USB radio on", "the laptop"),
           ("Laptop", "rebuilds the objects,", "repairs lost frames")]
    for i, (a, b, c) in enumerate(moq):
        x = 30 + i * (bw + gap - 4)
        s.rect(x, 284, bw, 92, extra='style="stroke:var(--moq)"')
        s.text(x + bw / 2, 310, a, "h", "middle")
        s.text(x + bw / 2, 332, b, "s", "middle")
        s.text(x + bw / 2, 347, c, "s", "middle")
        if i < len(moq) - 1:
            s.arrow(x + bw + 2, 330, x + bw + gap - 4)
    x = 30 + 4 * (bw + gap - 4)
    s.rect(x, 284, 960 - 30 - x, 92, extra='style="stroke:var(--line);stroke-dasharray:5 4"')
    s.lines(x + 14, 310, ["The MoQ model, from moq.dev:", "one GOP is a group,", "one frame is an object.",
                          "No QUIC, no relay here."], "s", 16)
    s.foot(["Plain version: the old path hands every radio frame through several programs. moq-lora hands it straight to the receiver.",
            "Radio is simulated on localhost in the demo. The old stack here is Python end to end, so it is slower than a production one."])
    return s.save("stack.svg")


# ---------------------------------------------------------------- bar helpers
def hbars(s, x0, y0, w, rows, vmax, unit="%", logscale=False, lo=0.1, hi=100, bh=24, gapy=8, fmt=num):
    """rows: [(label, value, cls)]; returns y after bars."""
    y = y0
    for label, val, cls in rows:
        if logscale:
            frac = (math.log10(val) - math.log10(lo)) / (math.log10(hi) - math.log10(lo))
        else:
            frac = val / vmax
        frac = max(0.004, min(1, frac))
        s.text(x0 - 10, y + bh * 0.68, label, "b", "end")
        s.rect(x0, y, w * frac, bh, cls, 4)
        s.late.append(f'<text x="{x0 + w * frac + 8}" y="{y + bh * 0.7}" class="v">{escape(fmt(val) + unit)}</text>')
        y += bh + gapy
    return y


# ---------------------------------------------------------------- (b) frames shown
def fig_frames():
    s = S(430, f"Video frames that reach the screen at {LOSS*100:.0f}% radio loss",
          f"{N_FR} frames (a {SECS:.0f} second clip), same video and same losses sent through both paths",
          "Bar chart of delivered and decodable frames for the old stack and moq-lora.")
    x0, w = 200, 640
    s.text(30, 112, "Frames that arrived", "h")
    y = hbars(s, x0, 124, w, [("old stack", V1["old"]["delivered_pct"], "old"),
                              ("moq-lora", V1["moq"]["delivered_pct"], "moq")], 100)
    s.text(30, y + 26, "Frames you can actually play (decodable)", "h")
    y = hbars(s, x0, y + 38, w, [("old stack", V1["old"]["decodable_pct"], "old"),
                                 ("moq-lora", V1["moq"]["decodable_pct"], "moq")], 100)
    for p in (0, 25, 50, 75, 100):
        xx = x0 + w * p / 100
        s.p.insert(1, f'<line x1="{xx}" y1="104" x2="{xx}" y2="{y-4}" class="gl"/>')
        s.text(xx, y + 12, f"{p}%", "xs", "middle")
    s.lines(30, y + 40, [
        "Why 'playable' is lower: video frames lean on the frames before them. Once one is lost, the rest of that one-second group",
        "cannot be shown until the next keyframe (the GOP rule, same check for both stacks).",
        f"The old stack sends each frame as about {V1['old']['air_frames']/N_FR:.0f} separate uplinks with no repair. moq-lora adds one repair frame per 8 and rebuilt {V1['moq']['fec_recovered']} lost ones.",
    ], "s", 16)
    s.foot([PROV + ". One run: other seeds give somewhat different playable numbers."])
    return s.save("frames-shown.svg")


# ---------------------------------------------------------------- (c) delay, log scale
def fig_delay():
    s = S(430, "Software delay per message (log scale)",
          "Each gridline is 10 times the one before. Shorter is better.",
          "Log scale bar chart of median software latency, old stack versus moq-lora.")
    x0, w, lo, hi = 200, 620, 0.1, 100
    rows = []
    s.text(30, 108, f"Video frame (p50, run with no loss)", "h")
    y = hbars(s, x0, 120, w, [("old stack", V0["old"]["lat_ms"]["p50"], "old"),
                              ("moq-lora", V0["moq"]["lat_ms"]["p50"], "moq")], 0, " ms", True, lo, hi)
    s.text(30, y + 22, "Small sensor reading (p50)", "h")
    y = hbars(s, x0, y + 34, w, [("old stack", S0["old"]["lat_ms"]["p50"], "old"),
                                 ("moq-lora", S0["moq"]["lat_ms"]["p50"], "moq")], 0, " ms", True, lo, hi)
    for v in (0.1, 1, 10, 100):
        xx = x0 + w * (math.log10(v) - math.log10(lo)) / (math.log10(hi) - math.log10(lo))
        s.line(xx, 104, xx, y - 2, "gl")
        s.text(xx, y + 14, f"{v:g} ms", "xs", "middle")
    ratio = V0["old"]["lat_ms"]["p50"] / V0["moq"]["lat_ms"]["p50"]
    s.lines(30, y + 44, [
        f"For video that is about {ratio:,.0f} times less software delay. This is the time inside the programs, not on the radio:",
        "the radio is replaced by localhost here, so real radio time (milliseconds per frame) comes on top for both.",
        "The old stack is Python, so a C/Go production stack would do better than this bar suggests. Sensor readings are close.",
    ], "s", 16)
    s.foot([PROV + ". Both stacks send the same payloads on the same schedule."])
    return s.save("delay.svg")


# ---------------------------------------------------------------- (d) command timeline
def fig_command():
    s = S(480, "Sending a command back: why Class A waits",
          "A LoRaWAN Class A device only listens for a short time after it speaks.",
          "Timeline showing the receive windows of a Class A device versus an immediate moq-lora command.")
    x0, w, tmax = 250, 660, 3.0
    X = lambda t: x0 + w * t / tmax
    iv = CMD["interval_s"]
    # axis
    for t in (0, 0.5, 1, 1.5, 2, 2.5, 3):
        s.line(X(t), 96, X(t), 380, "gl")
        s.text(X(t), 396, f"{t:g} s", "xs", "middle")
    s.text(30, 128, "Old stack", "h")
    s.text(30, 146, "Class A", "s")
    s.text(30, 162, f"(talks every {iv:g} s)", "s")
    # uplinks at 0 and iv, window 1 s later
    for t in (0, iv):
        s.rect(X(t) - 3, 112, 6, 30, "old", 2)
        s.text(X(t), 106, "device sends", "xs", "middle")
        if t + 1 <= tmax:
            s.rect(X(t + 1), 112, X(t + 1.0 + 0.12) - X(t + 1), 30, "box", 2, 'style="stroke:var(--old)"')
            s.text(X(t + 1) + 4, 106, "listens", "xs")
    # command arrives at 0.45 s (illustration)
    tc = 0.45
    s.add(f'<path d="M{X(tc)} 186 L{X(tc)} 150" class="ln" style="stroke:var(--bad)"/>')
    s.text(X(tc), 202, "app queues a command", "xs", "middle", 'style="fill:var(--bad)"')
    s.rect(X(tc), 214, X(1 + iv) - X(tc), 16, "old", 3, 'style="opacity:.25"')
    s.text(X(tc) + 6, 226, "waits: the device is not listening", "xs")
    s.text(X(1 + iv), 250, "heard", "xs", "middle")
    s.text(30, 262, "(positions of this one example are illustrative)", "xs")
    # measured dots, old
    s.text(30, 296, "Measured round trips", "h")
    s.text(30, 312, f"{CMD['old']['count']} commands, old stack", "s")
    for i, r in enumerate(CMD["old"]["rtt_ms"]):
        s.add(f'<circle cx="{X(r/1000):.1f}" cy="{300 + (i%3)*10}" r="5" class="old" style="opacity:.85"/>')
    p50o = CMD["old"]["p50"] / 1000
    s.line(X(p50o), 286, X(p50o), 336, "ln", 'style="stroke:var(--old);stroke-dasharray:4 3"')
    s.text(X(p50o) + 6, 346, f"p50 {CMD['old']['p50']/1000:.2f} s", "v", "start", 'style="fill:var(--old)"')
    s.text(30, 372, f"{CMD['moq']['count']} commands, moq-lora", "s")
    for i, r in enumerate(CMD["moq"]["rtt_ms"]):
        s.add(f'<circle cx="{X(r/1000):.1f}" cy="{362 + (i%3)*6}" r="5" class="moq" style="opacity:.85"/>')
    s.text(X(0.35), 372, f"p50 {CMD['moq']['p50']:.2f} ms: a command is just an object, nothing to wait for", "b", "start", 'style="fill:var(--moq)"')
    s.foot([f"One slow moq-lora sample ({CMD['moq']['max']:.0f} ms) is the first command of the run, released after the receiver's 150 ms idle rule.",
            PROV + ". Old stack delay scales with the uplink interval (here every " + f"{iv:g} s)."])
    return s.save("command.svg")


# ---------------------------------------------------------------- (e) airtime budget, computed
def fig_airtime():
    s = S(440, "Radio channel time the traffic needs",
          f"{SECS:.0f} seconds of traffic. 100% means the channel is full. Above 100% it cannot keep up.",
          "Bar chart of computed channel use for video and sensor traffic on LoRa DR4 versus the FSK link.")
    s.rect(660, 22, 270, 30, "box", 15, 'style="stroke:var(--warn)"')
    s.text(795, 42, "COMPUTED, NOT MEASURED", "h", "middle", 'style="fill:var(--warn)"')
    x0, w, vmax = 200, 640, 1800
    vo, vm = V0["model"]["old"]["channel_busy_pct"], V0["model"]["moq"]["channel_busy_pct"]
    so, sm = S0["model"]["old"]["channel_busy_pct"], S0["model"]["moq"]["channel_busy_pct"]
    s.text(30, 108, "Video, 180 kbit/s stream", "h")
    y = hbars(s, x0, 120, w, [("LoRa DR4", vo, "old"), ("FSK 300k", vm, "moq")], vmax, "%")
    s.text(30, y + 22, "Sensor readings, 20 per second", "h")
    y = hbars(s, x0, y + 34, w, [("LoRa DR4", so, "old"), ("FSK 300k", sm, "moq")], vmax, "%")
    xf = x0 + w * 100 / vmax
    s.p.insert(1, f'<line x1="{xf}" y1="104" x2="{xf}" y2="{y}" class="ln" style="stroke:var(--bad);stroke-dasharray:5 4"/>')
    s.text(xf + 5, y + 14, "100% = full", "xs", "start", 'style="fill:var(--bad)"')
    s.lines(30, y + 40, [
        f"Video needs about {vo/100:.0f} times what one LoRa DR4 channel can carry, and it fits on the FSK link at {vm:.0f}%.",
        f"A frame takes about {V0['model']['moq']['airtime_ms_per_payload']/ (V0['model']['moq']['frames']/N_FR):.0f} ms on air with FSK and about {V0['model']['old']['airtime_ms_per_payload']/(V0['model']['old']['frames']/N_FR):.0f} ms on LoRa DR4 (its fastest US uplink rate).",
        "Both radios are modelled from their formulas (Semtech airtime), lossless. Nothing here was sent over a real radio.",
    ], "s", 16)
    s.foot([f"Radio settings: old = {V0['model']['old']['radio']}; moq-lora = {V0['model']['moq']['radio']}.",
            "Source: model block in results/results.json."])
    return s.save("airtime.svg")


# ---------------------------------------------------------------- (f) where each wins
def fig_wins():
    s = S(520, "Where each one wins", "Pick by the job. This demo does not say one is better at everything.",
          "Two column panel comparing where moq-lora wins and where LoRaWAN wins.")
    cw = 440
    sa, sm_ = S0["old"]["air_bytes_per_payload"], S0["moq"]["air_bytes_per_payload"]
    app_b = S0["old"]["app_bytes_per_payload"]
    cols = [
        (30, "moq-lora wins", MOQ, "moq", [
            ("Video", f"{V1['moq']['decodable_pct']:.0f}% playable vs {V1['old']['decodable_pct']:.0f}% at {LOSS*100:.0f}% loss"),
            ("Fits the channel", f"video uses {vm_busy():.0f}% of the FSK link; LoRa DR4 would need {vo_busy():,.0f}% (computed)"),
            ("Less software delay", f"video {V0['moq']['lat_ms']['p50']:.2f} ms vs {V0['old']['lat_ms']['p50']:.1f} ms (p50, localhost)"),
            ("Commands", f"{CMD['moq']['p50']:.2f} ms vs {CMD['old']['p50']/1000:.1f} s round trip (Class A, uplink every {CMD['interval_s']:g} s)"),
            ("Fewer hops", "radio to receiver. No gateway, network server or broker"),
        ]),
        (490, "LoRaWAN wins", OLD, "old", [
            ("Tiny sensor readings", f"{sa:.0f} bytes on air per {app_b:.0f}-byte reading vs {sm_:.0f} for moq-lora"),
            ("Range", "LoRa at this setting hears much weaker signals: kilometres. moq-lora's runs were a metre apart"),
            ("Not tested here, either side", "range, real radio loss, battery life. This demo cannot rank those"),
        ]),
    ]
    for x, head, color, cls, items in cols:
        s.rect(x, 92, cw, 36, cls, 8)
        s.text(x + 16, 116, head, "badge", extra='style="font-size:17px"')
        y = 154
        for a, b in items:
            s.text(x + 6, y, a, "h")
            words, line, out = b.split(), "", []
            for wd in words:
                if len(line) + len(wd) + 1 > 70:
                    out.append(line); line = wd
                else:
                    line = (line + " " + wd).strip()
            out.append(line)
            s.lines(x + 6, y + 18, out, "s", 15)
            y += 34 + 15 * len(out)
    s.foot(["moq-lora's defaults are tuned for video (a 17 byte header, 16 byte tag, a repair frame and padding), which costs it on tiny readings.",
            PROV + "."])
    return s.save("where-each-wins.svg")


def vm_busy():
    return V0["model"]["moq"]["channel_busy_pct"]


def vo_busy():
    return V0["model"]["old"]["channel_busy_pct"]


# ---------------------------------------------------------------- replay data + HTML demo
def build_replay():
    """Per-frame outcomes of the measured video runs, for the HTML replay.

    Reads the run logs in ../runs when present (a full run.sh leaves them there) and keeps a
    copy in replay.json so the demo can be rebuilt without rerunning anything."""
    path = os.path.join(HERE, "replay.json")
    runs = os.path.join(ROOT, "runs")
    if os.path.exists(os.path.join(runs, "video-old-paced-0.0", "delivered.bin")):
        from oldstack.common import read_delivered
        from oldstack.workload import video
        pls = video()
        out = {"n": len(pls), "fps": 15, "key": [1 if p.group_start else 0 for p in pls], "runs": {}}
        for loss in sorted({r["loss"] for r in RES["paced"] if r["workload"] == "video"}):
            ent = {}
            for st in ("old", "moq"):
                d = os.path.join(runs, f"video-{st}-paced-{loss}")
                got = read_delivered(os.path.join(d, "delivered.bin"))
                ids = sorted({pid for pid, _, _ in got})
                ent[st] = ids
            out["runs"][str(loss)] = ent
        # frames per payload: old is exact (LoRaWAN fragments of 238 B), moq-lora is a model
        # (sealed size / 243 B frames plus one repair frame per 8) checked against the measured total
        old_f = [math.ceil(len(p.data) / 238) if len(p.data) > 242 else 1 for p in pls]
        moq_f = [math.ceil((len(p.data) + 38) / 243) for p in pls]
        moq_tot = sum(n + math.ceil(n / 8) for n in moq_f)
        out["frames_old"], out["frames_moq"] = old_f, moq_f
        out["check"] = {"old_model_total": sum(old_f), "old_measured_total": V0["old"]["air_frames"],
                        "moq_model_total": moq_tot, "moq_measured_total": V0["moq"]["air_frames"]}
        assert out["check"]["old_model_total"] == out["check"]["old_measured_total"], out["check"]
        assert abs(moq_tot - out["check"]["moq_measured_total"]) <= 0.005 * moq_tot, out["check"]
        json.dump(out, open(path, "w"), separators=(",", ":"))
    return json.load(open(path))


def build_html():
    rp = build_replay()
    data = {
        "replay": rp,
        "date": DATE,
        "video": {"loss": LOSS, "old": {k: V1["old"][k] for k in ("delivered_pct", "decodable_pct")},
                  "moq": {k: V1["moq"][k] for k in ("delivered_pct", "decodable_pct", "fec_recovered")}},
        "lat_p50": {"old": V0["old"]["lat_ms"]["p50"], "moq": V0["moq"]["lat_ms"]["p50"]},
        "cmd": {"old_p50_ms": CMD["old"]["p50"], "moq_p50_ms": CMD["moq"]["p50"], "interval_s": CMD["interval_s"]},
        "busy": {"old": V0["model"]["old"]["channel_busy_pct"], "moq": V0["model"]["moq"]["channel_busy_pct"]},
        "sensor_air": {"old": S0["old"]["air_bytes_per_payload"], "moq": S0["moq"]["air_bytes_per_payload"]},
    }
    tpl = open(os.path.join(HERE, "demo.template.html")).read()
    html = tpl.replace("/*__DATA__*/null", json.dumps(data, separators=(",", ":")))
    open(os.path.join(HERE, "index.html"), "w").write(html)
    return "index.html"


if __name__ == "__main__":
    os.makedirs(IMG, exist_ok=True)
    names = [fig_stack(), fig_frames(), fig_delay(), fig_command(), fig_airtime(), fig_wins()]
    if os.path.exists(os.path.join(HERE, "demo.template.html")):
        names.append(build_html())
    print("wrote", ", ".join(names))
