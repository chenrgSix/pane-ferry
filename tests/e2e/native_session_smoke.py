#!/usr/bin/env python3
"""Opt-in real SSH/X11 test. Starts and closes only its own approved xterm."""
import argparse
import base64
import json
import os
import selectors
import shlex
import struct
import subprocess
import time
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--host", required=True)
parser.add_argument("--user", required=True)
parser.add_argument("--port", type=int, default=22)
parser.add_argument("--control-path")
parser.add_argument("--reconnects", type=int, default=3)
parser.add_argument("--detached-seconds", type=float, default=1)
args = parser.parse_args()
ssh = ["/usr/bin/ssh", "-F", "/dev/null", "-T", "-oBatchMode=yes",
       "-oStrictHostKeyChecking=yes", "-oConnectTimeout=8", "-oForwardAgent=no",
       "-oClearAllForwardings=yes", "-p", str(args.port), "-l", args.user]
if args.control_path:
    ssh += ["-S", args.control_path]
ssh += ["--", args.host]


class Peer:
    def __init__(self):
        self.process = subprocess.Popen(ssh + [".local/bin/remoteapp-agent bridge"],
                                        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL)
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.buffer = bytearray()
        self.send(type="hello")
        assert self.read()["type"] == "hello"

    def send(self, **message):
        self.process.stdin.write(json.dumps({**message, "version": 1}).encode() + b"\n")
        self.process.stdin.flush()

    def read(self, timeout=15):
        deadline = time.monotonic() + timeout
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            if remaining <= 0 or not self.selector.select(remaining):
                raise TimeoutError("Timed out waiting for an Agent event")
            chunk = os.read(self.process.stdout.fileno(), 65536)
            if not chunk:
                raise EOFError("SSH/Agent connection closed")
            self.buffer.extend(chunk)
            if len(self.buffer) > 16 * 1024 * 1024 + 65536:
                raise ValueError("Agent event exceeds the protocol limit")
        line, _, rest = self.buffer.partition(b"\n")
        self.buffer = bytearray(rest)
        event = json.loads(line)
        assert event["version"] == 1
        return event

    def until(self, predicate, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            event = self.read(max(.01, deadline - time.monotonic()))
            if event["type"] == "error":
                raise AssertionError("Agent rejected a test request: " + event["code"])
            if predicate(event):
                return event
        raise TimeoutError("Expected Agent event did not arrive")

    def close(self):
        self.selector.close()
        self.process.terminate()
        try:
            self.process.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=5)
        self.process.stdin.close()
        self.process.stdout.close()


def remote(command):
    return subprocess.run(ssh + [command], check=True, capture_output=True, timeout=15)


def map_window(display, window_id, visible):
    # Only the test session's numeric display and window ID enter this helper.
    assert str(display).isdigit() and isinstance(window_id, int)
    script = f"""import ctypes, os
os.environ['XAUTHORITY'] = os.path.expanduser('~/.local/state/lan-remoteapp/Xauthority')
x = ctypes.CDLL('libX11.so.6')
x.XOpenDisplay.argtypes = [ctypes.c_char_p]
x.XOpenDisplay.restype = ctypes.c_void_p
d = x.XOpenDisplay(b':{int(display)}')
assert d
x.XMapRaised.argtypes = x.XUnmapWindow.argtypes = [ctypes.c_void_p, ctypes.c_ulong]
x.XSync.argtypes = [ctypes.c_void_p, ctypes.c_int]
x.XCloseDisplay.argtypes = [ctypes.c_void_p]
x.{'XMapRaised' if visible else 'XUnmapWindow'}(d, {window_id})
x.XSync(d, 0)
x.XCloseDisplay(d)
"""
    remote("python3 -c " + shlex.quote(script))


def assert_png(frame):
    png = base64.b64decode(frame["png"], validate=True)
    assert png.startswith(b"\x89PNG\r\n\x1a\n")
    assert struct.unpack(">II", png[16:24]) == (frame["width"], frame["height"])


def input_state(display, window_id):
    script = f"""import ctypes, json, os
os.environ['XAUTHORITY'] = os.path.expanduser('~/.local/state/lan-remoteapp/Xauthority')
x = ctypes.CDLL('libX11.so.6')
x.XOpenDisplay.argtypes = [ctypes.c_char_p]
x.XOpenDisplay.restype = ctypes.c_void_p
d = x.XOpenDisplay(b':{int(display)}')
assert d
x.XQueryPointer.argtypes = [ctypes.c_void_p, ctypes.c_ulong, ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_ulong), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_int), ctypes.POINTER(ctypes.c_uint)]
root, child = ctypes.c_ulong(), ctypes.c_ulong()
rx, ry, wx, wy, mask = ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_int(), ctypes.c_uint()
x.XQueryPointer(d, {window_id}, ctypes.byref(root), ctypes.byref(child), ctypes.byref(rx), ctypes.byref(ry), ctypes.byref(wx), ctypes.byref(wy), ctypes.byref(mask))
print(json.dumps(dict(x=wx.value, y=wy.value, buttons=mask.value)))
x.XCloseDisplay.argtypes = [ctypes.c_void_p]
x.XCloseDisplay(d)
"""
    return json.loads(remote("python3 -c " + shlex.quote(script)).stdout)


peer = None
session = None
window_id = None
marker = "/tmp/lan-remoteapp-native-" + uuid.uuid4().hex
request_id = uuid.uuid4().hex
try:
    peer = Peer()
    peer.send(type="app.launch", app_id="xterm", request_id=request_id,
              backend="native", attach=False)
    started = peer.until(lambda e: e["type"] == "session.started")
    session = started["session_id"]
    peer.send(type="session.resume", session_id=session)
    peer.until(lambda e: e["type"] == "session.snapshot")
    frame = peer.until(lambda e: e["type"] == "frame")
    assert_png(frame)
    window_id = frame["window_id"]

    retry = Peer()
    try:
        retry.send(type="app.launch", app_id="xterm", request_id=request_id,
                   backend="native", attach=False)
        assert retry.until(lambda e: e["type"] == "session.started")["session_id"] == session
        retry.send(type="session.resume", session_id=session)
        assert retry.read()["type"] == "error", "A second controller must be rejected"
    finally:
        retry.close()

    for character in "touch " + marker:
        peer.send(type="input.key", window_id=window_id, keysym=ord(character), pressed=True)
        peer.send(type="input.key", window_id=window_id, keysym=ord(character), pressed=False)
    peer.send(type="input.key", window_id=window_id, keysym=0xff0d, pressed=True)
    peer.send(type="input.key", window_id=window_id, keysym=0xff0d, pressed=False)
    # A fresh frame after input plus an actual file proves remote execution.
    peer.until(lambda e: e["type"] == "frame")
    for attempt in range(40):
        result = subprocess.run(ssh + ["test -f " + shlex.quote(marker)], timeout=5)
        if result.returncode == 0:
            break
        time.sleep(.1)
    assert result.returncode == 0, "Keyboard input did not execute in the remote xterm"

    peer.send(type="window.configure", window_id=window_id, width=960, height=600)
    geometry = peer.until(lambda e: e["type"] == "window.metadata" and
                          e["window_id"] == window_id and
                          e["geometry_revision"] > frame["geometry_revision"])
    map_window(started["display"], window_id, False)
    hidden = peer.until(lambda e: e.get("window_id") == window_id and
                        e["type"] in ("window.metadata", "window.destroyed") and
                        (e["type"] == "window.destroyed" or not e.get("visible", True)))
    assert hidden["type"] == "window.metadata", "Unmapping must preserve window identity"
    map_window(started["display"], window_id, True)
    shown = peer.until(lambda e: e["type"] == "window.metadata" and
                       e.get("window_id") == window_id and e.get("visible", True))
    assert shown["geometry_revision"] >= geometry["geometry_revision"]
    assert_png(peer.until(lambda e: e["type"] == "frame" and e["window_id"] == window_id))

    for connection_index in range(args.reconnects):
        if connection_index == 0:
            peer.send(type="input.pointer", window_id=window_id, x=17, y=23, button=1, pressed=True)
            peer.send(type="input.key", window_id=window_id, keysym=0xffe1, pressed=True)
            for attempt in range(20):
                state = input_state(started["display"], window_id)
                if state["x"] == 17 and state["y"] == 23 and state["buttons"] & 257 == 257:
                    break
                time.sleep(.1)
            assert state["buttons"] & 257 == 257, "Pointer button and Shift must reach X11"
        peer.close()  # Abrupt disconnect, with no session.detach message.
        peer = None
        time.sleep(args.detached_seconds)
        if connection_index == 0:
            assert input_state(started["display"], window_id)["buttons"] & 257 == 0, "Disconnect must release held inputs"
        peer = Peer()
        peer.send(type="session.resume", session_id=session)
        snapshot = peer.until(lambda e: e["type"] == "session.snapshot")
        assert any(w["window_id"] == window_id for w in snapshot["windows"])
        assert_png(peer.until(lambda e: e["type"] == "frame" and e["window_id"] == window_id))
        remote("test -f " + shlex.quote(marker))

    peer.send(type="window.close.request", window_id=window_id)
    peer.until(lambda e: e["type"] == "session.exited")
    session = None
    print(json.dumps({"result": "PASS", "checks": ["real SSH/X11 PNG", "idempotent launch",
          "exclusive controller", "remote keyboard/pointer input", "held input release", "resize", "hide/remap",
          "close/exited"], "reconnects": args.reconnects,
          "detached_seconds_each": args.detached_seconds}))
finally:
    if peer is not None:
        if session and window_id:
            try:
                peer.send(type="window.close.request", window_id=window_id)
            except (BrokenPipeError, OSError):
                pass
        peer.close()
    remote("rm -f -- " + shlex.quote(marker))
