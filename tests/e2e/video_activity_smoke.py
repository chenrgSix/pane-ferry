#!/usr/bin/env python3
"""Opt-in Linux test: disposable GTK/Xvfb session through two private QUIC nodes.

No installed services, saved identities or user windows are used. Tests real
capture/encoding, pause acknowledgement, live input and keyframe recovery.
"""
import argparse
import json
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import time
import uuid

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', type=Path, required=True)
parser.add_argument('--network', type=Path, required=True)
args = parser.parse_args()
agent, network = args.agent.resolve(), args.network.resolve()

with tempfile.TemporaryDirectory(prefix='pf-activity-', dir='/tmp') as folder:
    root = Path(folder)
    processes = []
    client = None
    log = (root / 'test.log').open('wb')
    def start(command, **kw):
        process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=log, stderr=log, **kw)
        processes.append(process)
        return process
    def manage(state, **request):
        with socket.socket(socket.AF_UNIX) as control:
            control.settimeout(10)
            control.connect(str(state / 'control.sock'))
            control.sendall(json.dumps(request).encode() + b'\n')
            result = json.loads(control.makefile('rb').readline(131073))
            assert 'error' not in result, result
            return result
    def ready(state):
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            try:
                return manage(state, op='status')
            except (OSError, ValueError):
                time.sleep(.05)
        raise AssertionError('private network did not start')
    try:
        fixture = root / 'fixture.py'
        marker = root / 'input.json'
        fixture.write_text('''import gi, json, sys
from pathlib import Path
gi.require_version('Gtk','3.0')
from gi.repository import Gtk, GLib
window=Gtk.Window(title='Disposable video activity fixture')
window.set_default_size(480,270)
label=Gtk.Label();window.add(label)
counter=0
def tick():
 global counter
 counter+=1;label.set_text(str(counter));return True
def key(_window,event):
 Path(sys.argv[1]).write_text(json.dumps({'keysym':event.keyval,'draw':counter}))
 return False
window.connect('key-press-event',key)
window.connect('destroy',Gtk.main_quit)
GLib.timeout_add(16,tick);window.show_all();Gtk.main()
''')
        home = root / 'home'
        config = home / '.config/lan-remoteapp'
        config.mkdir(parents=True)
        (config / 'apps.json').write_text(json.dumps([dict(id='activity-fixture', name='Activity fixture', executable='/usr/bin/python3', argv=[str(fixture), str(marker)])]))
        start([str(agent), 'serve'], env={**os.environ, 'HOME':str(home)})
        publisher = home / '.local/state/lan-remoteapp/agent.sock'
        deadline = time.monotonic() + 10
        while not publisher.exists():
            assert time.monotonic() < deadline
            time.sleep(.05)
        states = [root / 'a', root / 'b']
        for state in states:
            start([str(network), 'serve', '--state', str(state), '--publisher', str(publisher), '--listen', '127.0.0.1:0', '--name', 'activity-fixture', '--bulk-agent', str(agent)])
        a, b = map(ready, states)
        invitation = manage(states[1], op='invite')['invitation']
        manage(states[0], op='pair', invitation=invitation, address=b['listen'])
        route = states[0] / ('r-' + b['device_id'] + '.sock')
        deadline = time.monotonic() + 10
        while not route.exists():
            assert time.monotonic() < deadline, 'paired route did not appear'
            time.sleep(.02)
        client = socket.socket(socket.AF_UNIX)
        client.connect(str(route))
        buffer = bytearray()
        def send(kind, **body):
            client.sendall(json.dumps(dict(type=kind, version=1, **body)).encode() + b'\n')
        def read(timeout=10):
            deadline = time.monotonic() + timeout
            while b'\n' not in buffer:
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([client], [], [], remaining)[0]:
                    raise TimeoutError('no protocol message')
                chunk = client.recv(65536)
                assert chunk, 'protocol disconnected'
                buffer.extend(chunk)
                assert len(buffer) <= 16*1024*1024 + 65536
            line, _, rest = buffer.partition(b'\n')
            buffer[:] = rest
            event = json.loads(line)
            assert event['type'] != 'error', event
            return event
        def until(kind):
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                event = read(max(.01, deadline-time.monotonic()))
                if event['type'] == kind:
                    return event
            raise TimeoutError(kind)
        def collect(seconds):
            frames = []
            deadline = time.monotonic() + seconds
            while time.monotonic() < deadline:
                try:
                    event = read(deadline-time.monotonic())
                    if event['type'] == 'frame':
                        frames.append(event)
                except TimeoutError:
                    break
            return frames
        def activity(state):
            send('video.activity', state=state)
            assert until('video.activity.changed')['state'] == state
        send('hello')
        assert 'video.activity' in until('hello')['capabilities']
        send('video.enable', codec='h264', max_fps=30)
        until('video.ready')
        send('app.launch', app_id='activity-fixture', backend='native', scale_factor=1, request_id=uuid.uuid4().hex)
        until('session.started')
        created = until('window.created')
        first = until('frame')
        assert first['key_frame'] and first['width'] == created['width']
        active = collect(1)
        assert len(active) >= 8, ('insufficient active fixture frames', len(active))
        activity('background')
        background = collect(1.2)
        assert len(background) <= 8, ('background exceeded 5 FPS plus transition allowance', len(background))
        activity('suspended')
        assert not collect(.4), 'video continued after suspension acknowledgement'
        send('window.focus', window_id=created['window_id'])
        send('input.key', window_id=created['window_id'], keysym=120, pressed=True)
        send('input.key', window_id=created['window_id'], keysym=120, pressed=False)
        deadline = time.monotonic() + 3
        while not marker.exists():
            assert time.monotonic() < deadline, 'fixture input blocked by suspended video'
            time.sleep(.01)
        assert json.loads(marker.read_text())['keysym'] == 120
        send('session.list')
        until('session.list')
        assert not collect(.4), 'input implicitly resumed hidden video'
        resumed_at = time.monotonic()
        activity('foreground')
        resumed = until('frame')
        assert resumed['key_frame'] and resumed['stream_id'] != first['stream_id']
        assert resumed['width'] == first['width'] and resumed['height'] == first['height']
        result = dict(test='private-X11-real-QUIC-video-activity', foreground_frames_1s=len(active), background_frames_1_2s=len(background), paused_video_frames=0, input_while_paused='passed', resumed_with_new_keyframe=True, resume_to_frame_ms=round((time.monotonic()-resumed_at)*1000, 3), selected_cap_fps=30, user_data_used=False)
        send('session.detach')
        print(json.dumps(result), flush=True)
    except Exception:
        log.flush()
        print((root / 'test.log').read_text(errors='replace')[-4000:], flush=True)
        raise
    finally:
        if client:
            client.close()
        for process in reversed(processes):
            process.terminate()
        for process in reversed(processes):
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill(); process.wait()
        log.close()
