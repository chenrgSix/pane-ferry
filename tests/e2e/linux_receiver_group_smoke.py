#!/usr/bin/env python3
"""Real GTK decode/draw and the launcher's group IPC on an isolated X display."""
import base64
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='pf-group-') as temporary:
    os.environ['HOME'] = temporary
    sys.path.insert(0, str(root / 'apps/linux-client'))
    from wire import STATE, private, save
    import gi
    gi.require_version('Gst', '1.0')
    from gi.repository import Gst
    Gst.init(None)
    pipeline = Gst.parse_launch('videotestsrc num-buffers=1 ! video/x-raw,format=I420,width=640,height=480,framerate=30/1 ! openh264enc ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=out sync=false')
    pipeline.set_state(Gst.State.PLAYING)
    sample = pipeline.get_by_name('out').emit('try-pull-sample', 5 * Gst.SECOND)
    assert sample, 'fixture encoder unavailable'
    data = sample.get_buffer(); frame = base64.b64encode(data.extract_dup(0, data.get_size())).decode()
    pipeline.set_state(Gst.State.NULL)
    peer = str(uuid.uuid4()); group = uuid.uuid4().hex + uuid.uuid4().hex
    private(STATE / 'network')
    save(STATE / 'network/peers.json', {peer: dict(device_id=peer, name='Fixture', address='127.0.0.1:1')})
    listener = socket.socket(socket.AF_UNIX)
    listener.bind(str(STATE / 'network' / f'r-{peer}.sock')); listener.listen()
    events = []; sockets = []; stop = threading.Event()
    def publisher(client):
        source = client.makefile('rb'); session = None; sequence = 0
        def send(kind, **fields):
            nonlocal sequence
            sequence += 1
            body = dict(type=kind, version=1, seq=sequence, session_id=session or '')
            body.update(fields); client.sendall((json.dumps(body) + '\n').encode())
        try:
            for line in source:
                event = json.loads(line); kind = event['type']; events.append((session, event))
                if kind == 'hello': send('hello', capabilities=['native.h264', 'input.key.logical', 'video.qos'], video_max_fps=60)
                elif kind == 'video.enable': send('video.ready', codec='h264')
                elif kind == 'session.resume':
                    session = event['session_id']
                    send('session.snapshot', session_id=session, windows=[dict(window_id=42, title='Fixture ' + session, width=640, height=480, geometry_revision=1, scale_factor=1)])
                    send('frame', window_id=42, width=640, height=480, encoded_width=640, encoded_height=480, geometry_revision=1, stream_id=1, frame_id=1, key_frame=True, codec='h264', data=frame)
        except OSError: pass
        finally: source.close(); client.close()
    def serve():
        while not stop.is_set():
            try: client, _ = listener.accept()
            except OSError: return
            sockets.append(client); threading.Thread(target=publisher, args=(client,), daemon=True).start()
    threading.Thread(target=serve, daemon=True).start()
    log = open(Path(temporary) / 'receiver.log', 'w+')
    command = [sys.executable, '-I', str(root / 'apps/linux-client/receiver_main.py'), '--native-peer', peer,
               '--native-state', str(STATE / 'network'), '--native-helper', '/unused-in-fixture', '--session', 'one',
               '--application-group', group, '--name', 'Fixture', '--fps', '30', '--picture-mode', 'source', '--video-color', '420']
    child = subprocess.Popen(command, stdout=log, stderr=log, env=dict(os.environ, GDK_BACKEND='x11'))
    path = Path('/tmp') / f'pf-app-{os.getuid()}' / (group + '.sock')
    watchers = []
    def until(predicate, seconds=10):
        deadline = time.monotonic() + seconds
        while not predicate():
            if child.poll() is not None:
                log.seek(0); raise AssertionError(log.read())
            assert time.monotonic() < deadline, 'receiver did not make progress'
            time.sleep(.02)
    def exchange(op, **fields):
        client = socket.socket(socket.AF_UNIX); client.settimeout(5)
        deadline = time.monotonic() + 3
        while True:
            try: client.connect(str(path)); break
            except (FileNotFoundError, ConnectionRefusedError):
                assert child.poll() is None and time.monotonic() < deadline, 'broker did not start listening'
                time.sleep(.02)
        client.sendall((json.dumps(dict(version=1, group=group, op=op, **fields)) + '\n').encode())
        source = client.makefile('rb'); result = json.loads(source.readline())
        if op == 'attach': watchers.append((fields['session'], client, source))
        else: source.close(); client.close()
        return result
    try:
        until(path.exists)
        for session, fps in [('one', 30), ('two', 60)]:
            response = exchange('attach', session=session, target='org.example.App.desktop', fps=fps,
                                background=False, picture_mode='source', video_color='420')
            assert response['pid'] == child.pid and response['session'] == session
            until(lambda: any(s == session and e['type'] == 'video.feedback' for s, e in events))
        assert [e['max_fps'] for _, e in events if e['type'] == 'video.enable'] == [30, 60]
        assert len(sockets) == 2
        exchange('attach', session='one', target='org.example.App.desktop', fps=30,
                 background=True, picture_mode='source', video_color='420')
        assert len(sockets) == 2, 're-attach created another video receiver'
        assert len(exchange('list')['sessions']) == 2
        exchange('detach', session='one')
        for session, _, source in watchers:
            if session == 'one': assert json.loads(source.readline())['closed']
        assert [row['session'] for row in exchange('list')['sessions']] == ['two']
        exchange('detach', session='two'); assert child.wait(timeout=5) == 0
        assert not path.exists()
        assert not any(e['type'] in ('app.launch', 'window.close.request') for _, e in events)
        print('PASS real GTK group: decoded/drew two sessions with frame feedback, 30/60 FPS, re-attach reuse, independent detach, source apps untouched and clean process exit')
    except BaseException:
        log.flush(); log.seek(0); print(log.read(), file=sys.stderr)
        print('Fixture events:', [(s, e['type']) for s, e in events[-12:]], file=sys.stderr)
        raise
    finally:
        stop.set(); listener.close()
        if child.poll() is None: child.terminate(); child.wait(timeout=5)
        for _, client, source in watchers: source.close(); client.close()
        for client in sockets:
            try: client.shutdown(socket.SHUT_RDWR)
            except OSError: pass
        log.close()
