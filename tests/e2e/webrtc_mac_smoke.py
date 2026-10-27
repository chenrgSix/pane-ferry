#!/usr/bin/env python3
"""Real RTP -> current Swift/CoreVideo receiver, with owned Linux encoder packets."""
import argparse
import base64
import ctypes as C
import json
import os
from pathlib import Path
import queue
import subprocess
import tempfile
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', required=True, type=Path)
parser.add_argument('--packets', required=True, type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
packet = next(row for row in json.loads(args.packets.read_text()) if row['key_frame'])
raw = base64.b64decode(packet.pop('data'), validate=True)
api = C.CDLL(str(args.library.resolve()))
callback_type = C.CFUNCTYPE(None, C.c_void_p, C.c_int, C.c_char_p, C.c_void_p, C.c_size_t)
api.ra_rtc_create.argtypes = [C.c_int, C.c_char_p, callback_type, C.c_void_p]
api.ra_rtc_create.restype = C.c_void_p
for name in ('ra_rtc_description', 'ra_rtc_candidate'):
    getattr(api, name).argtypes = [C.c_void_p, C.c_char_p, C.c_char_p]
api.ra_rtc_add_video.argtypes = [C.c_void_p, C.c_uint32]
api.ra_rtc_send.argtypes = [C.c_void_p, C.c_uint32, C.c_void_p, C.c_size_t, C.c_void_p, C.c_size_t]
api.ra_rtc_close.argtypes = [C.c_void_p]
events = queue.Queue()


@callback_type
def callback(_context, kind, label, data, size):
    events.put(('sender', (kind, label, C.string_at(data, size) if size else b'')))


with tempfile.TemporaryDirectory(prefix='paneferry-webrtc-mac-') as directory:
    scratch = Path(directory)
    types = scratch / 'WindowHostTypes.swift'
    types.write_text((source / 'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch / 'receiver'
    files = [str(path) for path in source.glob('*.swift') if path.name != 'main.swift']
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'), *files, str(types),
                    str(root / 'tests/swift/WebRTCMediaSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    env = dict(os.environ, REMOTEAPP_WEBRTC_LIBRARY=str(args.library.resolve()), REMOTEAPP_WEBRTC_BIND='127.0.0.1')
    process = subprocess.Popen([str(binary)], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
    sender = api.ra_rtc_create(1, b'127.0.0.1', callback, None)
    assert sender
    frames, keyframe_requests = [], []
    def read():
        for line in process.stdout:
            events.put(('receiver', json.loads(line)))
    thread = threading.Thread(target=read, daemon=True); thread.start()
    def control(**object):
        process.stdin.write(json.dumps(object) + '\n'); process.stdin.flush()
    def pump(predicate, timeout=10):
        deadline = time.monotonic() + timeout
        while not predicate():
            assert time.monotonic() < deadline, ('receiver timeout', frames, process.poll())
            try: side, value = events.get(timeout=.02)
            except queue.Empty: continue
            if side == 'sender':
                kind, label, data = value
                if kind == 1: control(type='webrtc.offer', sdp=data.decode())
                elif kind == 2: control(type='webrtc.candidate', candidate=data.decode(), mid=label.decode())
                elif kind == 4: keyframe_requests.append(label)
                elif kind == 6: raise AssertionError(label)
            else:
                kind = value['type']
                if kind == 'webrtc.answer': assert api.ra_rtc_description(sender, value['sdp'].encode(), b'answer') == 0
                elif kind == 'webrtc.candidate': assert api.ra_rtc_candidate(sender, value['candidate'].encode(), value['mid'].encode()) == 0
                elif kind == 'test.frame': frames.append(value)
                elif kind == 'test.failed': raise AssertionError(value)
    session = '7' * 32
    window = dict(window_id=7, width=packet['width'], height=packet['height'], geometry_revision=1, visible=True)
    def send(number, media=1, seq=3, revision=1, sid=session):
        metadata = dict(packet, version=1, transport='webrtc', window_id=7, session_id=sid,
                        frame_id=number, media_frame_id=media, seq=seq, geometry_revision=revision)
        header = json.dumps(metadata).encode()
        assert api.ra_rtc_send(sender, 7, raw, len(raw), header, len(header)) == 0
    try:
        control(type='session.started', session_id=session, seq=1)
        control(type='window.created', session_id=session, seq=2, **window)
        assert api.ra_rtc_add_video(sender, 7) == 0
        pump(lambda: api.ra_rtc_add_video(sender, 7) == 1)
        send(100)
        pump(lambda: len(frames) == 1)
        send(200, media=2, seq=4)
        pump(lambda: len(frames) == 2)
        # Control identity must retire a same-XID decoder without an unbounded tombstone set.
        requested = len(keyframe_requests)
        control(type='window.destroyed', session_id=session, seq=10, window_id=7)
        control(type='window.created', session_id=session, seq=11, source_seq=20, **window)
        # RTP can beat lifecycle controls. Wait for the receiver's actual PLI
        # before supplying the replacement IDR, as the real encoder would.
        pump(lambda: len(keyframe_requests) > requested)
        send(201, media=3, seq=5)
        send(202, media=4, seq=21, sid='8' * 32)
        send(300, media=1, seq=22)
        pump(lambda: len(frames) == 3)
        assert [row['frame_id'] for row in frames] == [100, 200, 300]
        # Media can overtake revision metadata; decoding it must preserve the surface.
        send(301, media=2, seq=23, revision=2)
        control(type='window.metadata', session_id=session, seq=24, **dict(window, geometry_revision=2))
        pump(lambda: len(frames) == 4)
        assert frames[-1]['frame_id'] == 301
        assert all(row['pixel_buffer'] and row['hardware_decoder'] for row in frames)
        print('PASS: actual RTP hardware decode retains CoreVideo surfaces, initial session, sparse feedback IDs, reused-window/session rejection and revision handoff')
    finally:
        control(type='test.close')
        process.wait(timeout=10); thread.join(timeout=2)
        api.ra_rtc_close(sender)
        assert process.returncode == 0
