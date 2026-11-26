#!/usr/bin/env python3
"""The #402 masking session, then what the app holds and does at rest (issue #436).

Starts the desktop app with the control server, a fresh profile and library, opens one raw in
Masking and runs the session #402 measured: 12 masks (5 brush masks of 4 strokes and an erase,
2 linear, 2 radial, a luminance range, a colour range, and one AI mask, Subject), zoom 100 % and
200 % with pans and a stroke each, 12 more pans with a stroke, mask slider ticks, back to Fit.
Then it leaves the app alone for `--idle` seconds (no control requests, which draw frames) and
records:

- CPU at rest: the app's CPU seconds over `--cpu` more seconds (`ps`), `top`'s CPU %, idle
  wake-ups and power per second, and the GPU's whole-device use (`ioreg`, every app's);
- memory at rest: `footprint -v` and `vmmap --summary` (by category), then `ui.inspect` → `memory`
  (each cache) and `perf`, and `mask.aiModels` for the AI mask (which models are loaded);
- the footprint every 2 s through the session (its peak);
- the footprint again after a windowed screenshot: egui frees textures only once a frame reaches the screen, which a
  covered window never does, so the difference is GPU memory that waited for the window to be shown (issue #436).

Everything goes to `--out` (raw outputs and `summary.json`); the summary is also printed.

Run (release build; one app per port; models come from the shared ArtCraft models folder):

    cargo build --release -p lightcraft
    python3 fork/rest-memory.py --app target/release/lightcraft --port 7990 \
        --photo corpus/raw/arw-sony-ilce-7m5.arw --out /tmp/rest-memory

`--no-ai` leaves the Subject mask out (no model loads); `--keep` leaves the app running.
"""

import argparse
import json
import os
import re
import socket
import subprocess
import sys
import threading
import time


class Control:
    """One JSON-lines connection to the app's control server."""

    def __init__(self, port):
        self.port = port
        self.sock = None
        self.buf = b""
        self.next_id = 0

    def connect(self, wait=120):
        end = time.time() + wait
        while time.time() < end:
            try:
                self.sock = socket.create_connection(("127.0.0.1", self.port), timeout=5)
                self.sock.settimeout(90)
                return
            except OSError:
                time.sleep(0.5)
        raise SystemExit(f"no control server on port {self.port}")

    def call(self, method, params=None):
        self.next_id += 1
        line = json.dumps({"id": self.next_id, "method": method, "params": params or {}}) + "\n"
        try:
            self.sock.sendall(line.encode())
            reply = self._read()
        except OSError:
            # an idle close (5 min without requests): reconnect once and send again
            self.connect()
            self.sock.sendall(line.encode())
            reply = self._read()
        if not reply.get("ok"):
            raise RuntimeError(f"{method} {params}: {reply.get('error')}")
        return reply.get("result")

    def _read(self):
        while b"\n" not in self.buf:
            chunk = self.sock.recv(1 << 20)
            if not chunk:
                raise OSError("closed")
            self.buf += chunk
        line, self.buf = self.buf.split(b"\n", 1)
        return json.loads(line)

    def ex(self, command, params=None):
        return self.call("engine.execute", {"command": command, "params": params or {}})


def footprint_bytes(pid):
    """The process's footprint (bytes) from `footprint -p`, or None."""
    out = subprocess.run(["footprint", "-p", str(pid)], capture_output=True, text=True).stdout
    m = re.search(r"Footprint:\s+([\d.]+)\s+(KB|MB|GB)", out)
    if not m:
        return None
    return float(m.group(1)) * {"KB": 1 << 10, "MB": 1 << 20, "GB": 1 << 30}[m.group(2)]


class Sampler(threading.Thread):
    """The footprint every `every` seconds until stopped."""

    def __init__(self, pid, every=2.0):
        super().__init__(daemon=True)
        self.pid, self.every, self.samples, self.stop = pid, every, [], threading.Event()

    def run(self):
        t0 = time.time()
        while not self.stop.is_set():
            b = footprint_bytes(self.pid)
            if b is not None:
                self.samples.append((round(time.time() - t0, 1), b))
            self.stop.wait(self.every)


def settle(c, timeout=120):
    """Wait until the loupe shows a finished render and no render is queued or running."""
    end = time.time() + timeout
    while time.time() < end:
        try:
            s = c.call("ui.inspect")
        except RuntimeError as e:
            # (a window the system stops drawing for a moment: asked again)
            if "drew no frame" not in str(e):
                raise
            time.sleep(1)
            continue
        p, lo = s.get("perf", {}), s.get("loupe") or {}
        region = lo.get("region") or {}
        if (
            not p.get("renderQueue")
            and not p.get("rendersInFlight")
            and not p.get("pendingSlots")
            and not lo.get("pending")
            and not region.get("pending")
        ):
            return s
        time.sleep(0.25)
    return None


def stroke(c, x, y, size=0.03, erase=False, n=12):
    pts = [[x + 0.06 * i / n, y + 0.02 * ((i % 3) - 1) / 2] for i in range(n)]
    c.ex("mask.brushStroke", {"points": pts, "size": size, "erase": erase})


def session(c, ai):
    """The #402 masking session (about a minute on a 33 MP raw)."""
    c.call("ui.resize", {"width": 1600, "height": 1000})
    c.call("ui.set", {"view": "detail"})
    c.ex("panel.masking")
    settle(c)
    # five brush masks: four strokes and an erase each
    for k in range(5):
        c.ex("tool.brush", {"new": True})
        y = 0.15 + 0.15 * k
        for j in range(4):
            stroke(c, 0.1 + 0.18 * j, y)
        stroke(c, 0.2, y, erase=True)
        settle(c)
    c.ex("mask.add", {"kind": "linear", "start": [0.5, 0.0], "end": [0.5, 0.4]})
    c.ex("mask.add", {"kind": "linear", "start": [0.5, 1.0], "end": [0.5, 0.7]})
    c.ex("mask.add", {"kind": "radial", "center": [0.3, 0.5]})
    c.ex("mask.add", {"kind": "radial", "center": [0.7, 0.4], "rx": 0.15, "ry": 0.2})
    c.ex("mask.add", {"kind": "luminanceRange", "lo": 0.5, "hi": 1.0})
    c.ex("mask.add", {"kind": "colorRange", "samples": [[0.4, 0.5, 0.7]]})
    ai_id = None
    if ai:
        before = len(masks(c))
        c.ex("mask.add", {"kind": "subject"})
        # in the app an AI mask is found off the UI thread and appears when found
        end = time.time() + 300
        while time.time() < end and len(masks(c)) <= before:
            time.sleep(1)
        ms = masks(c)
        ai_id = ms[-1]["id"] if len(ms) > before else None
        while ai_id is not None and time.time() < end and c.ex("mask.aiModels", {"id": ai_id}).get("working"):
            time.sleep(1)
        print(f"  subject mask: id {ai_id}", flush=True)
    for k in range(4):
        c.ex("mask.adjust", {"values": {"exposure": 0.2 * (k + 1)}})
    settle(c)
    # zoom 100 % and 200 %, four pans with a stroke each
    for pct in (100, 200):
        for (x, y) in ((0.3, 0.3), (0.7, 0.3), (0.7, 0.7), (0.3, 0.7)):
            c.ex("view.navigate", {"zoom": {"percent": pct}, "pan": [x, y]})
            settle(c)
            c.ex("mask.select", {"id": 1})
            stroke(c, x - 0.03, y, size=0.01)
            settle(c)
    # twelve more pans with a stroke each, at 100 %
    for k in range(12):
        x, y = 0.15 + 0.06 * k, 0.25 + 0.04 * (k % 6)
        c.ex("view.navigate", {"zoom": {"percent": 100}, "pan": [x, y]})
        settle(c)
        stroke(c, x - 0.02, y, size=0.008)
        settle(c)
    # mask slider ticks at 100 % and at Fit
    for k in range(3):
        c.ex("mask.adjust", {"values": {"exposure": 0.3 + 0.1 * k}})
        settle(c)
    c.ex("view.navigate", {"zoom": "fit"})
    for k in range(3):
        c.ex("mask.adjust", {"values": {"contrast": 10 * (k + 1)}})
        settle(c)
    return ai_id


def masks(c):
    return (c.ex("develop.get") or {}).get("masks") or []


def cpu_seconds(pid):
    out = subprocess.run(["ps", "-o", "time=", "-p", str(pid)], capture_output=True, text=True).stdout.strip()
    parts = [float(p) for p in re.split(r"[:]", out.replace("-", ":")) if p]
    secs = 0.0
    for p in parts:
        secs = secs * 60 + p
    return secs


def gpu_ns(pid):
    """The GPU time (ns) the process's Metal clients have used since they opened (`ioreg`, AGX user clients)."""
    out = subprocess.run(["ioreg", "-l", "-w0", "-r", "-c", "AGXDeviceUserClient"], capture_output=True, text=True).stdout
    total, mine = 0, False
    for line in out.splitlines():
        m = re.search(r'"IOUserClientCreator" = "pid (\d+),', line)
        if m:
            mine = int(m.group(1)) == pid
        elif mine and '"AppUsage"' in line:
            total += sum(int(x) for x in re.findall(r'"accumulatedGPUTime"=(\d+)', line))
    return total


def rest_cpu(pid, seconds, out):
    """CPU, wake-ups and power over `seconds` at rest, with no control request in between."""
    c0, g0, t0 = cpu_seconds(pid), gpu_ns(pid), time.time()
    top = subprocess.Popen(
        ["top", "-l", str(seconds + 1), "-s", "1", "-pid", str(pid), "-stats", "pid,cpu,idlew,power"],
        stdout=subprocess.PIPE,
        text=True,
    )
    text = top.communicate()[0]
    c1, g1, t1 = cpu_seconds(pid), gpu_ns(pid), time.time()
    with open(os.path.join(out, "top.txt"), "w") as f:
        f.write(text)
    rows = [l.split() for l in text.splitlines() if l.strip().startswith(str(pid))]
    rows = rows[1:]  # top's first sample has no CPU % yet
    def avg(i):
        vals = [float(r[i]) for r in rows if len(r) > i and re.match(r"^[\d.]+$", r[i])]
        return round(sum(vals) / len(vals), 2) if vals else None
    return {
        "seconds": round(t1 - t0, 1),
        "cpuPercent": round(100 * (c1 - c0) / (t1 - t0), 2),
        "topCpuPercent": avg(1),
        "idleWakeupsPerSecond": avg(2),
        "power": avg(3),
        "gpuPercent": round(100 * (g1 - g0) / 1e9 / (t1 - t0), 3),
    }


def vmmap_summary(pid, out):
    text = subprocess.run(["vmmap", "--summary", str(pid)], capture_output=True, text=True).stdout
    with open(os.path.join(out, "vmmap-summary.txt"), "w") as f:
        f.write(text)
    return text


def footprint_verbose(pid, out):
    text = subprocess.run(["footprint", "-v", "-p", str(pid)], capture_output=True, text=True).stdout
    with open(os.path.join(out, "footprint-v.txt"), "w") as f:
        f.write(text)
    # the category table: "<dirty> <clean> <reclaimable> <regions> <category>"
    cats = {}
    unit = {"B": 1, "KB": 1 << 10, "MB": 1 << 20, "GB": 1 << 30}
    for line in text.splitlines():
        m = re.match(r"^\s*([\d.]+)\s+(B|KB|MB|GB)\s+([\d.]+)\s+(B|KB|MB|GB)\s+([\d.]+)\s+(B|KB|MB|GB)\s+(\d+)\s+(.+?)\s*$", line)
        if m:
            cats[m.group(8)] = round(float(m.group(1)) * unit[m.group(2)] / (1 << 20), 1)
    total = re.search(r"Footprint:\s+([\d.]+)\s+(KB|MB|GB)", text)
    return {
        "footprintMB": round(float(total.group(1)) * unit[total.group(2)] / (1 << 20), 1) if total else None,
        "dirtyMBByCategory": dict(sorted(cats.items(), key=lambda kv: -kv[1])[:25]),
    }


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--app", required=True)
    ap.add_argument("--port", type=int, default=7990)
    ap.add_argument("--photo", required=True)
    ap.add_argument("--out", required=True)
    ap.add_argument("--idle", type=int, default=60)
    ap.add_argument("--cpu", type=int, default=30)
    ap.add_argument("--no-ai", action="store_true")
    ap.add_argument("--keep", action="store_true")
    a = ap.parse_args()

    os.makedirs(a.out, exist_ok=True)
    try:
        socket.create_connection(("127.0.0.1", a.port), timeout=1).close()
        raise SystemExit(f"port {a.port} is in use: pick another --port")
    except OSError:
        pass
    env = dict(os.environ, LIGHTCRAFT_NO_UPDATE_CHECK="1", LIGHTCRAFT_CONFIG_DIR=os.path.join(a.out, "config"))
    log = open(os.path.join(a.out, "app.log"), "w")
    app = subprocess.Popen(
        [a.app, "--control", str(a.port), "--library", os.path.join(a.out, "library")], env=env, stdout=log, stderr=subprocess.STDOUT
    )
    with open(os.path.join(a.out, "app.pid"), "w") as f:
        f.write(str(app.pid))
    print(f"app pid {app.pid}", flush=True)
    c = Control(a.port)
    c.connect()
    # the desktop app replaces itself once at start (MallocLargeCache=0, #304; the same process): connect again after it
    time.sleep(2)
    c.connect()
    pid = int(subprocess.run(["lsof", "-t", "-sTCP:LISTEN", f"-iTCP:{a.port}"], capture_output=True, text=True).stdout.split()[0])
    print(f"serving pid {pid}", flush=True)
    summary = {"pid": pid}
    try:
        c.ex("library.import", {"paths": [os.path.abspath(a.photo)]})
        end = time.time() + 120
        while time.time() < end and not c.call("ui.inspect").get("active"):
            time.sleep(0.5)
        sampler = Sampler(pid)
        sampler.start()
        t0 = time.time()
        ai_id = session(c, not a.no_ai)
        summary["sessionSeconds"] = round(time.time() - t0, 1)
        st = settle(c)
        summary["settled"] = st is not None
        print(f"session done in {summary['sessionSeconds']} s; idle {a.idle} s", flush=True)
        time.sleep(a.idle)
        sampler.stop.set()
        summary["peakFootprintMB"] = round(max((b for _, b in sampler.samples), default=0) / (1 << 20), 1)
        with open(os.path.join(a.out, "footprint-samples.json"), "w") as f:
            json.dump(sampler.samples, f)
        summary["restCpu"] = rest_cpu(pid, a.cpu, a.out)
        summary["rest"] = footprint_verbose(pid, a.out)
        vm = vmmap_summary(pid, a.out)
        m = re.search(r"Physical footprint:\s+(\S+)", vm)
        summary["vmmapFootprint"] = m.group(1) if m else None
        insp = c.call("ui.inspect")
        with open(os.path.join(a.out, "inspect.json"), "w") as f:
            json.dump(insp, f, indent=1)
        summary["memory"] = insp.get("memory")
        summary["repaintCauses"] = (insp.get("perf") or {}).get("repaintCauses")
        # egui frees textures only after a frame reaches the screen: a covered window shows none, so the frees wait (issue
        # #436). A windowed screenshot draws one even then; what the footprint loses is what waited.
        c.call("ui.screenshot", {"path": os.path.join(a.out, "after-rest.png")})
        time.sleep(3)
        summary["footprintAfterAFrameShownMB"] = round((footprint_bytes(pid) or 0) / (1 << 20), 1)
        if ai_id is not None:
            summary["aiModels"] = c.ex("mask.aiModels", {"id": ai_id})
    finally:
        with open(os.path.join(a.out, "summary.json"), "w") as f:
            json.dump(summary, f, indent=1)
        print(json.dumps(summary, indent=1))
        if not a.keep:
            try:
                c.ex("app.quit")
            except Exception:
                pass
            time.sleep(3)
            for p in {pid, app.pid}:
                try:
                    os.kill(p, 15)
                except OSError:
                    pass


if __name__ == "__main__":
    sys.exit(main())
