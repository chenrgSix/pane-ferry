#!/usr/bin/env python3
"""Opt-in Linux test. Creates its own X11 desktop/processes; never uses the user's desktop."""
import argparse
import base64
import ctypes as C
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
args = parser.parse_args()
agent_exe = args.agent.resolve()
children = []


def start(argv, env):
    child = subprocess.Popen(argv, env=env, start_new_session=True,
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
    children.append(child)
    return child


def wait_for(predicate, message):
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if predicate():
            return
        time.sleep(.05)
    raise AssertionError(message)


class Peer:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(10)
        self.socket.connect(str(path))
        self.file = self.socket.makefile('rwb', buffering=0)
        self.send(type='hello')
        assert self.read()['type'] == 'hello'

    def send(self, **event):
        self.file.write(json.dumps({**event, 'version': 1}).encode() + b'\n')

    def read(self):
        line = self.file.readline(16 * 1024 * 1024 + 1)
        assert line.endswith(b'\n') and len(line) <= 16 * 1024 * 1024
        return json.loads(line)

    def until(self, kind):
        while True:
            event = self.read()
            assert event['type'] != 'error', event
            if event['type'] == kind:
                return event

    def close(self):
        self.file.close()
        self.socket.close()


try:
    with tempfile.TemporaryDirectory(prefix='remoteapp-existing-') as tmp:
        root = Path(tmp)
        home = root / 'home'
        config = home / '.config/lan-remoteapp'
        config.mkdir(parents=True)
        authority = root / 'Xauthority'
        authority.touch(mode=0o600)
        display = next(n for n in range(1100, 2000) if not Path(f'/tmp/.X11-unix/X{n}').exists())
        subprocess.run(['xauth', '-f', str(authority), 'add', f':{display}',
                        'MIT-MAGIC-COOKIE-1', secrets.token_hex(16)], check=True)
        env = {**os.environ, 'DISPLAY': f':{display}', 'XAUTHORITY': str(authority), 'HOME': str(home)}
        env.pop('WAYLAND_DISPLAY', None)
        env.pop('SESSION_MANAGER', None)
        env.pop('DBUS_SESSION_BUS_ADDRESS', None)
        start(['Xvfb', f':{display}', '-screen', '0', '1280x800x24',
               '-nolisten', 'tcp', '-auth', str(authority)], env)
        wait_for(lambda: Path(f'/tmp/.X11-unix/X{display}').exists(), 'Xvfb startup')
        wm = start(['openbox'], env)
        target = start(['/usr/bin/xterm', '-title', 'AllowedExistingWindow'], env)
        other = start(['/usr/bin/xterm', '-title', 'UnrelatedWindow'], env)
        # Inventory this test-owned display to prove the excluded window really exists.
        x = C.CDLL('libX11.so.6')
        x.XOpenDisplay.argtypes = [C.c_char_p]
        x.XOpenDisplay.restype = C.c_void_p
        x.XDefaultRootWindow.argtypes = [C.c_void_p]
        x.XDefaultRootWindow.restype = C.c_ulong
        x.XInternAtom.argtypes = [C.c_void_p, C.c_char_p, C.c_int]
        x.XInternAtom.restype = C.c_ulong
        x.XGetWindowProperty.argtypes = [C.c_void_p, C.c_ulong, C.c_ulong, C.c_long,
            C.c_long, C.c_int, C.c_ulong, C.POINTER(C.c_ulong), C.POINTER(C.c_int),
            C.POINTER(C.c_ulong), C.POINTER(C.c_ulong), C.POINTER(C.POINTER(C.c_ubyte))]
        x.XFree.argtypes = [C.c_void_p]
        x.XCloseDisplay.argtypes = [C.c_void_p]
        os.environ['XAUTHORITY'] = str(authority)
        d = x.XOpenDisplay(f':{display}'.encode())
        assert d

        def numbers(window, atom):
            actual, fmt, count, rest = C.c_ulong(), C.c_int(), C.c_ulong(), C.c_ulong()
            data = C.POINTER(C.c_ubyte)()
            x.XGetWindowProperty(d, window, x.XInternAtom(d, atom, 0), 0, 4096, 0, 0,
                                C.byref(actual), C.byref(fmt), C.byref(count), C.byref(rest), C.byref(data))
            try:
                return list(C.cast(data, C.POINTER(C.c_ulong))[:count.value]) if fmt.value == 32 else []
            finally:
                if data:
                    x.XFree(data)

        def windows_by_pid():
            return {numbers(w, b'_NET_WM_PID')[0]: w
                    for w in numbers(x.XDefaultRootWindow(d), b'_NET_CLIENT_LIST')
                    if numbers(w, b'_NET_WM_PID')}

        try:
            wait_for(lambda: len(numbers(x.XDefaultRootWindow(d), b'_NET_CLIENT_LIST')) >= 2, 'both xterm windows ready')
            assert target.pid in windows_by_pid() and other.pid in windows_by_pid(), 'window PID properties unavailable'
        except AssertionError as error:
            raise AssertionError(f'{error}: expected {target.pid}/{other.pid}; clients {numbers(x.XDefaultRootWindow(d), b"_NET_CLIENT_LIST")}; discovered {windows_by_pid()}; exit status {target.poll()}/{other.poll()}; wm {wm.poll()}') from error
        ids = windows_by_pid()
        existing = dict(display=display, xauthority=str(authority), process_id=target.pid)
        (config / 'apps.json').write_text(json.dumps([
            dict(id='existing', name='Existing', executable='/usr/bin/xterm', existing=existing),
            dict(id='wrong-exe', name='Wrong', executable='/usr/bin/true', existing=existing),
        ]))
        agent_env = {**env, 'HOME': str(home)}
        sock = home / '.local/state/lan-remoteapp/agent.sock'
        agent = start([str(agent_exe), 'serve'], agent_env)
        wait_for(sock.exists, 'Agent startup')
        peer = Peer(sock)
        peer.send(type='app.launch', app_id='existing', request_id='first', backend='native', attach=False)
        session = peer.until('session.started')['session_id']
        peer.send(type='app.launch', app_id='existing', request_id='another', backend='native', attach=False)
        assert peer.until('session.started')['session_id'] == session
        peer.send(type='app.launch', app_id='wrong-exe', request_id='bad', backend='native', attach=False)
        assert peer.read()['type'] == 'error'
        peer.send(type='session.resume', session_id=session)
        snapshot = peer.until('session.snapshot')
        assert [w['window_id'] for w in snapshot['windows']] == [ids[target.pid]], snapshot
        frame = peer.until('frame')
        assert base64.b64decode(frame['png']).startswith(b'\x89PNG\r\n\x1a\n')
        peer.send(type='window.focus', window_id=ids[other.pid])
        while peer.read()['type'] != 'error':
            pass
        marker = root / 'input-reached-existing-window'
        for char in 'touch ' + str(marker) + '\r':
            for pressed in (True, False):
                peer.send(type='input.key', window_id=ids[target.pid],
                          keysym=0xff0d if char == '\r' else ord(char), pressed=pressed)
        wait_for(marker.exists, 'input must reach the pre-existing process')
        peer.close()
        agent.terminate()
        assert agent.wait(timeout=10) == 0
        assert target.poll() is None and other.poll() is None, 'Agent must not terminate existing processes'
        agent = start([str(agent_exe), 'serve'], agent_env)
        wait_for(sock.exists, 'Agent restart')
        peer = Peer(sock)
        peer.send(type='app.launch', app_id='existing', request_id='after-restart', backend='native')
        peer.until('session.started')
        assert peer.until('frame')['window_id'] == ids[target.pid]
        peer.close()
        x.XCloseDisplay(d)
        print('PASS: process-scoped capture/input, unrelated-window rejection, executable verification, session reuse, existing processes survive Agent restart')
finally:
    for child in reversed(children):
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
