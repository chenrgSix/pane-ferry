#!/usr/bin/env python3
"""Opt-in Linux test: compare actual GTK logical sizes and captured pixels at 1x/2x."""
import argparse
import base64
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
args = parser.parse_args()


def wait_for(predicate):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        try:
            if predicate(): return
        except (FileNotFoundError, json.JSONDecodeError): pass
        time.sleep(.05)
    raise AssertionError('test condition timed out')


with tempfile.TemporaryDirectory(prefix='remoteapp-hidpi-') as directory:
    root = Path(directory)
    config = root / '.config/lan-remoteapp'
    config.mkdir(parents=True)
    apps = []
    for scale in (1, 2):
        apps.append(dict(id=f'gtk-{scale}', name=f'GTK {scale}x', executable=sys.executable,
                         argv=[str(Path(__file__).with_name('clipboard_editor_fixture.py').resolve()),
                               str(root / f'text-{scale}'), str(root / f'metrics-{scale}')]))
    (config / 'apps.json').write_text(json.dumps(apps))
    env = dict(os.environ, HOME=str(root), XMODIFIERS='@im=none', GTK_IM_MODULE='simple')
    for name in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER'):
        env.pop(name, None)
    agent = subprocess.Popen([str(args.agent.resolve()), 'serve'], env=env,
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        path = root / '.local/state/lan-remoteapp/agent.sock'
        wait_for(path.exists)
        for scale in (1, 2):
            with socket.socket(socket.AF_UNIX) as peer:
                peer.settimeout(15)
                peer.connect(str(path))
                wire = peer.makefile('rwb', buffering=0)
                def send(**value): wire.write(json.dumps(dict(value, version=1)).encode() + b'\n')
                def until(kind, predicate=lambda event: True):
                    while True:
                        event = json.loads(wire.readline(16 * 1024 * 1024 + 1))
                        assert event['type'] != 'error', event
                        if event['type'] == 'window.created':
                            assert event['title'] != 'Openbox' and event['visible'], 'WM and never-shown helper windows must not be published'
                        if event['type'] == kind and predicate(event): return event
                send(type='hello')
                until('hello')
                send(type='app.launch', app_id=f'gtk-{scale}', request_id=f'test-{scale}', backend='native', scale_factor=scale)
                assert until('session.started')['scale_factor'] == scale
                window = until('window.created', lambda event: event['title'] == 'ControllerTextFixture')
                assert (window['width'], window['height'], window['scale_factor']) == (600 * scale, 400 * scale, scale), window
                png = base64.b64decode(until('frame', lambda event: event['window_id'] == window['window_id'])['png'])
                assert struct.unpack('>II', png[16:24]) == (600 * scale, 400 * scale)
                metrics = root / f'metrics-{scale}'
                wait_for(lambda: metrics.exists() and json.loads(metrics.read_text()) == dict(scale=scale, width=600, height=400))
                send(type='window.configure', window_id=window['window_id'], width=800 * scale, height=500 * scale)
                changed = until('window.metadata', lambda event: event['window_id'] == window['window_id'])
                assert (changed['width'], changed['height']) == (800 * scale, 500 * scale)
                wait_for(lambda: json.loads(metrics.read_text()) == dict(scale=scale, width=800, height=500))
                send(type='input.pointer', window_id=window['window_id'], x=100 * scale, y=100 * scale, button=1, pressed=True)
                send(type='input.pointer', window_id=window['window_id'], x=100 * scale, y=100 * scale, button=1, pressed=False)
                send(type='input.text.commit', window_id=window['window_id'], text='清晰度🙂')
                wait_for(lambda: (root / f'text-{scale}').read_text() == '清晰度🙂')
                wire.close()
        print('PASS: GTK 1x/2x actual scale, constant logical size, doubled PNG pixels, resize and input mapping')
    finally:
        agent.terminate()
        try: agent.wait(timeout=10)
        except subprocess.TimeoutExpired: agent.kill(); agent.wait(timeout=5)
