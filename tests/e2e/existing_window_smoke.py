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
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
parser.add_argument('--controller-text', action='store_true', help='also exercise Unicode input and explicit clipboard copy/paste using GTK3')
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
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            event = self.read()
            assert event['type'] != 'error' or kind == 'error', event
            if event['type'] == kind:
                return event
        raise TimeoutError('Expected Agent event did not arrive: ' + kind)

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
        env = {**os.environ, 'DISPLAY': f':{display}', 'XAUTHORITY': str(authority), 'HOME': str(home),
               'XMODIFIERS': '@im=none', 'GTK_IM_MODULE': 'simple', 'QT_IM_MODULE': 'compose', 'GDK_BACKEND': 'x11'}
        env.pop('WAYLAND_DISPLAY', None)
        env.pop('SESSION_MANAGER', None)
        env.pop('DBUS_SESSION_BUS_ADDRESS', None)
        start(['Xvfb', f':{display}', '-screen', '0', '1280x800x24',
               '-nolisten', 'tcp', '-auth', str(authority)], env)
        wait_for(lambda: Path(f'/tmp/.X11-unix/X{display}').exists(), 'Xvfb startup')
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
        x.XSetInputFocus.argtypes = [C.c_void_p, C.c_ulong, C.c_int, C.c_ulong]
        x.XGetInputFocus.argtypes = [C.c_void_p, C.POINTER(C.c_ulong), C.POINTER(C.c_int)]
        x.XSync.argtypes = [C.c_void_p, C.c_int]
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

        wm = start(['openbox'], env)
        wait_for(lambda: bool(numbers(x.XDefaultRootWindow(d), b'_NET_SUPPORTING_WM_CHECK')), 'window manager ready')
        target = start(['/usr/bin/xterm', '-title', 'AllowedExistingWindow'], env)
        other = start(['/usr/bin/xterm', '-title', 'UnrelatedWindow'], env)

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
        editor = None
        if args.controller_text:
            editor_state = root / 'editor-text'
            editor = start([sys.executable, str(Path(__file__).with_name('clipboard_editor_fixture.py')), str(editor_state)], env)
            wait_for(lambda: editor_state.exists() and editor.pid in windows_by_pid(), 'GTK editor ready')
            editor_window = windows_by_pid()[editor.pid]
        existing = dict(display=display, xauthority=str(authority), process_id=target.pid)
        approved = [
            dict(id='existing', name='Existing', executable='/usr/bin/xterm', existing=existing),
            dict(id='wrong-exe', name='Wrong', executable='/usr/bin/true', existing=existing),
        ]
        if editor:
            approved.append(dict(id='controller-text', name='Controller text', executable=str(Path(f'/proc/{editor.pid}/exe').resolve()),
                existing=dict(display=display, xauthority=str(authority), process_id=editor.pid)))
        (config / 'apps.json').write_text(json.dumps(approved))
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
        peer.until('error')
        # Hovering a forwarded surface must not repeatedly activate the remote
        # application or take focus away from another window on the desktop.
        x.XSetInputFocus(d, ids[other.pid], 1, 0)
        x.XSync(d, 0)
        peer.send(type='input.pointer', window_id=ids[target.pid], x=20, y=20, button=0, pressed=False)
        peer.send(type='session.list')
        peer.until('session.list')
        focused, revert = C.c_ulong(), C.c_int()
        x.XGetInputFocus(d, C.byref(focused), C.byref(revert))
        assert focused.value == ids[other.pid], 'pointer motion must not activate the controlled window'
        marker = root / 'input-reached-existing-window'
        for char in 'touch ' + str(marker) + '\r':
            # tempfile suffixes can contain '_'. input.key represents physical
            # key/modifier events, so this symbol needs Shift on the test US map.
            if char == '_':
                peer.send(type='input.key', window_id=ids[target.pid], keysym=0xffe1, pressed=True)
            for pressed in (True, False):
                peer.send(type='input.key', window_id=ids[target.pid],
                          keysym=0xff0d if char == '\r' else ord(char), pressed=pressed)
            if char == '_':
                peer.send(type='input.key', window_id=ids[target.pid], keysym=0xffe1, pressed=False)
        wait_for(marker.exists, 'input must reach the pre-existing process')
        peer.close()
        if editor:
            peer = Peer(sock)
            peer.send(type='app.launch', app_id='controller-text', request_id='text', backend='native')
            peer.until('session.started')
            peer.until('frame')
            first, second, pasted = '控制端输入🙂', '/ordered', ' /粘贴内容'
            peer.send(type='input.text.commit', window_id=editor_window, text=first)
            peer.send(type='input.text.commit', window_id=editor_window, text=second)
            peer.send(type='clipboard.paste', window_id=editor_window, text=pasted)
            expected = first + second + pasted
            wait_for(lambda: editor_state.read_text() == expected, 'ordered Unicode commit and controller paste')
            def select_all():
                for symbol, pressed in [(0xffe3, True), (ord('a'), True), (ord('a'), False), (0xffe3, False)]:
                    peer.send(type='input.key', window_id=editor_window, keysym=symbol, pressed=pressed)
            select_all()
            peer.send(type='clipboard.copy', window_id=editor_window, request_id='explicit-copy')
            copied = peer.until('clipboard.data')
            assert copied['request_id'] == 'explicit-copy' and copied['window_id'] == editor_window
            assert copied['text'] == expected, copied
            # Replace the selection with a multiline controller clipboard payload.
            peer.send(type='clipboard.paste', window_id=editor_window, text='第一行\nsecond 🙂')
            wait_for(lambda: editor_state.read_text() == '第一行\nsecond 🙂', 'multiline Unicode paste')
            # Nothing is selected now. Copy must fail instead of returning an
            # earlier clipboard payload, even though that payload came from us.
            peer.send(type='clipboard.copy', window_id=editor_window, request_id='no-selection')
            assert peer.until('error')['code'] == 'clipboard_copy_failed'
            peer.send(type='clipboard.paste', window_id=editor_window, text='/after-timeout')
            wait_for(lambda: editor_state.read_text() == '第一行\nsecond 🙂/after-timeout', 'input resumes after failed copy')
            peer.close()
        agent.terminate()
        assert agent.wait(timeout=10) == 0
        assert target.poll() is None and other.poll() is None and (editor is None or editor.poll() is None), 'Agent must not terminate existing processes'
        agent = start([str(agent_exe), 'serve'], agent_env)
        wait_for(sock.exists, 'Agent restart')
        peer = Peer(sock)
        peer.send(type='app.launch', app_id='existing', request_id='after-restart', backend='native')
        peer.until('session.started')
        assert peer.until('frame')['window_id'] == ids[target.pid]
        peer.close()
        x.XCloseDisplay(d)
        print('PASS: process-scoped capture/input, unrelated-window rejection, executable verification, session reuse, existing processes survive Agent restart')
        if editor:
            print('PASS: ordered controller Unicode commit, explicit clipboard paste/copy, multiline UTF-8 round trip, no-selection copy rejection and recovery')
finally:
    for child in reversed(children):
        if child.poll() is None:
            os.killpg(child.pid, signal.SIGTERM)
            try:
                child.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(child.pid, signal.SIGKILL)
                child.wait(timeout=5)
