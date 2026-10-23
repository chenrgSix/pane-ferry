#!/usr/bin/env python3
"""Opt-in Linux test: verify a new private X11 session's DPI and GTK pixel density."""
import argparse
import json
import os
from pathlib import Path
import re
import socket
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


def x_window_size(name, display_env):
    result = subprocess.run(['xwininfo', '-name', name], env=display_env,
                            text=True, capture_output=True)
    if result.returncode:
        return None
    size = tuple(int(re.search(rf'^  {axis}: (\d+)$', result.stdout, re.MULTILINE).group(1))
                 for axis in ('Width', 'Height'))
    return size


with tempfile.TemporaryDirectory(prefix='paneferry-hidpi-') as directory:
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
        authority = root / '.local/state/lan-remoteapp/Xauthority'
        wait_for(path.exists)
        logical_font_heights = {}
        terminal_sizes = {}
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
                send(type='app.launch', app_id=f'gtk-{scale}', request_id=f'test-{scale}', backend='native', attach=False, scale_factor=scale)
                started = until('session.started')
                assert started['scale_factor'] == scale
                display_env = dict(env, DISPLAY=f":{started['display']}", XAUTHORITY=str(authority))
                display = subprocess.check_output(['xdpyinfo'], env=display_env, text=True)
                resolution = re.search(r'resolution:\s+(\d+)x(\d+) dots per inch', display)
                assert resolution and tuple(map(int, resolution.groups())) == (96 * scale, 96 * scale), resolution
                resources = subprocess.check_output(['xrdb', '-query'], env=display_env, text=True)
                assert re.search(rf'^Xft\.dpi:\s*{96 * scale}\s*$', resources, re.MULTILINE), resources
                metrics = root / f'metrics-{scale}'
                wait_for(lambda: metrics.exists() and all(json.loads(metrics.read_text())[key] == value for key, value in dict(scale=scale, width=600, height=400).items()))
                logical_font_heights[scale] = json.loads(metrics.read_text())['font_height']
                size = x_window_size('ControllerTextFixture', display_env)
                assert size == (600 * scale, 400 * scale), (scale, size)
                terminal = subprocess.Popen(['xterm', '-fa', 'DejaVu Sans Mono', '-fs', '12',
                                             '-geometry', '60x12', '-T', 'PaneFerry DPI fixture',
                                             '-hold', '-e', '/bin/true'], env=display_env,
                                            stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                            stderr=subprocess.DEVNULL)
                try:
                    wait_for(lambda: x_window_size('PaneFerry DPI fixture', display_env))
                    terminal_sizes[scale] = x_window_size('PaneFerry DPI fixture', display_env)
                finally:
                    terminal.terminate()
                    terminal.wait(timeout=5)
                wire.close()
        assert abs(logical_font_heights[1] - logical_font_heights[2]) <= 1, logical_font_heights
        assert all(1.8 <= terminal_sizes[2][axis] / terminal_sizes[1][axis] <= 2.2 for axis in (0, 1)), terminal_sizes
        print(f'PASS: private X11 96/192 DPI, GTK logical font heights {logical_font_heights}, GTK and Xft pixels 1x/2x ({terminal_sizes})')
    finally:
        agent.terminate()
        try: agent.wait(timeout=10)
        except subprocess.TimeoutExpired: agent.kill(); agent.wait(timeout=5)
