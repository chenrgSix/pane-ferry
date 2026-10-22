#!/usr/bin/env python3
"""Isolated Linux video negotiation, real IDR/P-frames, odd resize and reconnect."""
import argparse
import base64
import json
import os
from pathlib import Path
import socket
import statistics
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
parser.add_argument('--export', type=Path)
parser.add_argument('--network', type=Path, help='route RWP through two isolated native QUIC nodes')
args = parser.parse_args()
records = []
with tempfile.TemporaryDirectory(prefix='remoteapp-video-') as temporary:
    home = Path(temporary)
    config = home / '.config/lan-remoteapp'
    config.mkdir(parents=True)
    state = home / 'key.json'
    fixture = str(Path(__file__).with_name('latency_fixture.py').resolve())
    (config / 'apps.json').write_text(json.dumps([
        dict(id='video', name='Video', executable=sys.executable, argv=[fixture, str(state)]),
        dict(id='static', name='Static', executable=sys.executable, argv=[fixture, str(state), '--static'])]))
    env = dict(os.environ, HOME=str(home), XMODIFIERS='@im=none', GTK_IM_MODULE='simple')
    for name in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER'):
        env.pop(name, None)
    agent = subprocess.Popen([str(args.agent.resolve()), 'serve'], env=env,
                             stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    path = home / '.local/state/lan-remoteapp/agent.sock'
    networks = []
    def manage(directory, **body):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(20); client.connect(str(directory / 'control.sock'))
            client.sendall(json.dumps(body).encode() + b'\n')
            result = json.loads(client.makefile('rb').readline(131073))
            assert 'error' not in result, result
            return result
    class Peer:
        def __init__(self):
            self.socket = socket.socket(socket.AF_UNIX)
            self.socket.settimeout(15)
            self.socket.connect(str(path))
            self.wire = self.socket.makefile('rb', buffering=65536)
            self.send(type='hello')
            assert 'native.h264' in self.until('hello')['capabilities']
            self.send(type='video.enable', codec='h264')
            assert self.until('video.ready')['codec'] == 'h264'
        def send(self, **data):
            self.socket.sendall((json.dumps(dict(data, version=1)) + '\n').encode())
        def until(self, kind, predicate=lambda event: True):
            deadline = time.monotonic() + 20
            while time.monotonic() < deadline:
                line = self.wire.readline(16 * 1024 * 1024 + 1)
                assert line, 'Agent disconnected'
                event = json.loads(line)
                assert event['type'] != 'error', event
                if event['type'] == 'frame':
                    assert event.get('codec') == 'h264' and 'png' not in event, 'video fell back to PNG'
                if event['type'] == kind and predicate(event): return event
            raise AssertionError('missing ' + kind)
        def close(self):
            self.socket.shutdown(socket.SHUT_RDWR)
            self.wire.close(); self.socket.close()
    def frame(peer, connection=0, predicate=lambda event: True):
        event = peer.until('frame', predicate)
        raw = base64.b64decode(event['data'], validate=True)
        assert raw.startswith((b'\0\0\0\1', b'\0\0\1'))
        assert event['encoded_width'] == event['width'] + event['width'] % 2
        assert event['encoded_height'] == event['height'] + event['height'] % 2
        records.append(dict(event, connection=connection))
        return event
    try:
        deadline = time.monotonic() + 10
        while not path.exists():
            assert time.monotonic() < deadline
            time.sleep(.02)
        if args.network:
            directories = [home / 'receiver', home / 'publisher']
            for directory in directories:
                networks.append(subprocess.Popen([str(args.network.resolve()), 'serve', '--state', str(directory),
                    '--publisher', str(path), '--listen', '127.0.0.1:0'],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
            for directory in directories:
                deadline = time.monotonic() + 10
                while not (directory / 'control.sock').exists():
                    assert time.monotonic() < deadline
                    time.sleep(.02)
            remote = manage(directories[1], op='status')
            manage(directories[0], op='pair', invitation=manage(directories[1], op='invite')['invitation'], address=remote['listen'])
            path = directories[0] / ('r-' + remote['device_id'] + '.sock')
            deadline = time.monotonic() + 5
            while not path.exists():
                assert time.monotonic() < deadline
                time.sleep(.02)
        peer = Peer()
        try:
            peer.send(type='app.launch', app_id='video', request_id='video', backend='native')
            sid = peer.until('session.started')['session_id']
            window = peer.until('window.created', lambda event: event['title'] == 'RemoteAppLatencyFixture')
            wid = window['window_id']
            first = frame(peer)
            assert first['key_frame'] and (first['width'], first['height']) == (2560, 1664)
            samples = [first] + [frame(peer) for _ in range(9)]
            assert any(not sample['key_frame'] for sample in samples), 'no inter-frame compression'
            peer.send(type='input.key', window_id=wid, keysym=ord('x'), pressed=True)
            peer.send(type='input.key', window_id=wid, keysym=ord('x'), pressed=False)
            deadline = time.monotonic() + 2
            while not state.exists():
                assert time.monotonic() < deadline
                time.sleep(.01)
            assert json.loads(state.read_text())['count'] == 1
            frame(peer)
            peer.send(type='window.configure', window_id=wid, width=641, height=483)
            resized = peer.until('window.metadata', lambda event: event['window_id'] == wid and event['width'] == 641 and event['height'] == 483)
            changed = frame(peer, predicate=lambda event: event['geometry_revision'] == resized['geometry_revision'])
            assert changed['key_frame'] and changed['stream_id'] != first['stream_id']
            frame(peer)
        finally:
            peer.close()
        time.sleep(.15)
        resumed = Peer()
        try:
            resumed.send(type='session.resume', session_id=sid)
            resumed.until('session.snapshot')
            assert frame(resumed, connection=1)['key_frame']
            frame(resumed, connection=1)
        finally:
            resumed.close()
        time.sleep(.15)
        static = Peer()
        try:
            static.send(type='app.launch', app_id='static', request_id='static', backend='native')
            static_sid = static.until('session.started')['session_id']
            static.until('window.created', lambda event: event['title'] == 'RemoteAppLatencyFixture')
            static_first = frame(static, connection=2)
            assert static_first['key_frame']
            time.sleep(.3)
        finally:
            static.close()
        time.sleep(.15)
        static_resumed = Peer()
        try:
            static_resumed.send(type='session.resume', session_id=static_sid)
            static_resumed.until('session.snapshot')
            assert frame(static_resumed, connection=3)['key_frame'], 'idle reconnect needs an initial frame'
        finally:
            static_resumed.close()
        if args.export:
            args.export.write_text(json.dumps(records))
        print(json.dumps(dict(result='PASS', transport='quic' if args.network else 'socket', codec='h264', samples=len(records),
            median_wire_bytes=round(statistics.median(len(json.dumps(sample)) for sample in samples)),
            checks=['IDR and P-frames', 'input during streaming', 'odd resize new stream', 'reconnect keyframe', 'idle reconnect snapshot'])))
    finally:
        for process in networks:
            process.terminate(); process.wait(timeout=5)
        agent.terminate(); agent.wait(timeout=10)
