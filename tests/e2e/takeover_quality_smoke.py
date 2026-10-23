#!/usr/bin/env python3
"""Compare actual existing-process and private-launch paths using identical text.

Opt-in Ubuntu test. Own HOME, Xvfb, authority, Agent and GTK processes only.
Exported images contain generated fixture text, never user application content.
"""
import argparse
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


def fixture(record):
    import gi
    gi.require_version('Gtk', '3.0')
    gi.require_version('GdkX11', '3.0')
    from gi.repository import Gtk, GdkX11, GLib, Pango
    window = Gtk.Window(title='Quality fixture')
    window.set_default_size(900, 700)
    view = Gtk.TextView()
    view.set_wrap_mode(Gtk.WrapMode.WORD_CHAR)
    text = ('中文 English 0123456789 — 同样的逻辑字号与换行宽度。\n' * 12)
    view.get_buffer().set_text(text)
    window.add(view)
    window.show_all()
    def metrics(*_):
        font = view.get_pango_context().get_metrics(None, None)
        record.write_text(json.dumps(dict(pid=os.getpid(), xid=GdkX11.X11Window.get_xid(window.get_window()),
            scale=window.get_scale_factor(), width=window.get_allocated_width(), height=window.get_allocated_height(),
            font_height=(font.get_ascent()+font.get_descent())/Pango.SCALE,
            text=view.get_buffer().get_text(view.get_buffer().get_start_iter(), view.get_buffer().get_end_iter(), True))))
        return False
    window.connect('size-allocate', metrics)
    GLib.idle_add(metrics)
    window.connect('destroy', Gtk.main_quit)
    Gtk.main()


def wait(read):
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        try:
            result = read()
            if result: return result
        except (FileNotFoundError, json.JSONDecodeError): pass
        time.sleep(.05)
    raise AssertionError('fixture timed out')


class Peer:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.settimeout(15)
        self.sock.connect(str(path))
        self.wire = self.sock.makefile('rb')
        self.send(type='hello'); self.until('hello')
        self.send(type='video.enable', codec='h264'); self.until('video.ready')
    def send(self, **body):
        self.sock.sendall(json.dumps(dict(body, version=1)).encode()+b'\n')
    def until(self, kind):
        deadline = time.monotonic()+20
        while time.monotonic() < deadline:
            line = self.wire.readline(16*1024*1024+1)
            assert line.endswith(b'\n') and len(line) <= 16*1024*1024
            event = json.loads(line)
            assert event['type'] != 'error', event
            if event['type'] == kind: return event
        raise AssertionError('missing '+kind)
    def close(self):
        self.wire.close(); self.sock.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agent', required=True, type=Path)
    parser.add_argument('--probe', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(mode=0o700)  # Reject reusing/overwriting an old result.
    children = []
    with tempfile.TemporaryDirectory(prefix='paneferry-takeover-') as directory:
        home = Path(directory)
        env = dict(os.environ, HOME=str(home), GDK_BACKEND='x11', GDK_SCALE='1', GDK_DPI_SCALE='1',
                   GTK_IM_MODULE='gtk-im-context-simple', XMODIFIERS='@im=none')
        for key in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER'):
            env.pop(key, None)
        def start(argv, environment=env):
            child = subprocess.Popen(argv, env=environment, start_new_session=True,
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            children.append(child)
            return child
        try:
            display = next(n for n in range(1100, 2000) if not Path(f'/tmp/.X11-unix/X{n}').exists())
            authority = home/'authority'; authority.touch(mode=0o600)
            env.update(DISPLAY=f':{display}', XAUTHORITY=str(authority))
            subprocess.run(['xauth', '-f', str(authority), 'add', env['DISPLAY'], 'MIT-MAGIC-COOKIE-1', secrets.token_hex(16)], check=True)
            start(['Xvfb', env['DISPLAY'], '-screen', '0', '1920x1080x24', '-dpi', '96', '-nolisten', 'tcp', '-auth', str(authority)])
            wait(lambda: Path(f'/tmp/.X11-unix/X{display}').exists())
            start(['openbox'])
            # Wait for the private WM, not an arbitrary delay or the user's WM.
            wait(lambda: subprocess.run(['xprop', '-root', '_NET_SUPPORTING_WM_CHECK'], env=env,
                capture_output=True, text=True).stdout.find('window id #') >= 0)
            old = start([sys.executable, __file__, '--fixture', str(home/'existing.json')])
            before = wait(lambda: json.loads((home/'existing.json').read_text()))
            config = home/'.config/lan-remoteapp'; config.mkdir(parents=True)
            (config/'apps.json').write_text(json.dumps([
                dict(id='existing', name='Existing', executable=str(Path(sys.executable).resolve()),
                     existing=dict(display=display, xauthority=str(authority), process_id=old.pid)),
                dict(id='new', name='New', executable=sys.executable,
                     argv=[str(Path(__file__).resolve()), '--fixture', str(home/'new.json')])]))
            start([str(args.agent.resolve()), 'serve'])
            path = home/'.local/state/lan-remoteapp/agent.sock'; wait(path.exists)
            report = []
            for kind, scale in [('existing', 1), ('new', 2)]:
                peer = Peer(path)
                try:
                    peer.send(type='app.launch', app_id=kind, request_id=kind, backend='native', scale_factor=2, attach=False)
                    started = peer.until('session.started')
                    assert started['scale_factor'] == scale
                    assert (started['display'] == display) == (kind == 'existing')
                    metrics = wait(lambda: json.loads((home/(kind+'.json')).read_text()))
                    assert metrics['scale'] == scale and (metrics['width'], metrics['height']) == (900, 700)
                    assert metrics['text'] == before['text']
                    if kind == 'existing': assert metrics['pid'] == old.pid and old.poll() is None
                    else: assert metrics['pid'] != old.pid
                    peer.send(type='session.resume', session_id=started['session_id'])
                    snapshot = peer.until('session.snapshot')
                    window = next(w for w in snapshot['windows'] if w['window_id'] == metrics['xid'])
                    frame = peer.until('frame')
                    assert frame['window_id'] == window['window_id'] and frame['geometry_revision'] == window['geometry_revision']
                    assert (frame['width'], frame['height']) == (900*scale, 700*scale)
                    assert window['scale_mode'] == ('source' if kind == 'existing' else 'requested')
                    capture_env = dict(env, DISPLAY=f":{started['display']}", XAUTHORITY=str(authority if kind == 'existing' else home/'.local/state/lan-remoteapp/Xauthority'))
                    subprocess.run([str(args.probe.resolve()), str(metrics['xid']), str(metrics['pid']), str(output/kind)], env=capture_env, check=True)
                    # A diagnostic must not sample an XID owned by another PID
                    # or silently overwrite an earlier (possibly private) export.
                    for candidate_pid, destination in [(os.getpid(), output/(kind+'-wrong-pid')), (metrics['pid'], output/kind)]:
                        rejected = subprocess.run([str(args.probe.resolve()), str(metrics['xid']), str(candidate_pid), str(destination)],
                            env=capture_env, capture_output=True)
                        assert rejected.returncode != 0
                    assert not (output/(kind+'-wrong-pid')).exists()
                    (output/kind/'agent-frame.json').write_text(json.dumps(frame))
                    report.append(dict(origin=kind, **{k:v for k,v in metrics.items() if k != 'text'},
                        source_pixels=[frame['width'], frame['height']], geometry_revision=frame['geometry_revision'],
                        process_preserved=(kind == 'existing' and old.poll() is None)))
                finally: peer.close()
            assert abs(report[0]['font_height']-report[1]['font_height']) <= 1
            assert json.loads((home/'existing.json').read_text()) == before and old.poll() is None
            (output/'comparison.json').write_text(json.dumps(report, indent=2)+'\n')
            print('PASS: same text/logical layout/font height; existing process stays 1x, actual new process gets 2x; geometry revisions match')
        finally:
            for child in reversed(children):
                if child.poll() is None:
                    os.killpg(child.pid, signal.SIGTERM)
                    try: child.wait(timeout=10)
                    except subprocess.TimeoutExpired: os.killpg(child.pid, signal.SIGKILL); child.wait(timeout=5)


if __name__ == '__main__':
    if len(sys.argv) == 3 and sys.argv[1] == '--fixture': fixture(Path(sys.argv[2]))
    else: main()
