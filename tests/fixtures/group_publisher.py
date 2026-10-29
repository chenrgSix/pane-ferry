#!/usr/bin/env python3
"""Private receiver fixture: each bridge is a separate RWP session with ID 42."""
import json
import os
from pathlib import Path
import select
import sys
import time

root = Path(__file__).parent
pending = b''
session = None
seq = 0
generation = 0
video = False

def emit(**value):
    print(json.dumps(dict(value, version=1)), flush=True)

def record(item):
    fd = os.open(root/'requests', os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
    try:
        os.write(fd, json.dumps(dict(item, fixture_session=session, bridge_pid=os.getpid())).encode()+b'\n')
    finally:
        os.close(fd)

while True:
    if session and generation == 1 and (root/f'cut-{session}').exists():
        break
    if session and (root/f'end-{session}').exists():
        emit(type='session.exited', session_id=session, seq=seq+1)
        break
    if b'\n' not in pending:
        if not select.select([sys.stdin], [], [], .02)[0]:
            continue
        chunk = os.read(sys.stdin.fileno(), 65536)
        if not chunk:
            break
        pending += chunk
        if b'\n' not in pending:
            continue
    line, _, pending = pending.partition(b'\n')
    item = json.loads(line)
    if item['type'] == 'session.resume':
        session = item['session_id']
    record(item)
    if item['type'] == 'hello':
        emit(type='hello', capabilities=['native.h264', 'input.text.commit', 'video.qos', 'clipboard.copy'], video_max_fps=60)
    elif item['type'] == 'video.enable':
        video = True
        emit(type='video.ready', codec='h264')
    elif item['type'] == 'session.resume':
        assert video
        count = root/f'connections-{session}'
        generation = int(count.read_text())+1 if count.exists() else 1
        count.write_text(str(generation))
        seq = 1
        emit(type='session.snapshot', session_id=session, seq=seq, windows=[dict(
            window_id=42, title=f'Group fixture {session}', width=640, height=480,
            geometry_revision=generation, scale_factor=1, visible=True, role='toplevel')])
        for index, frame in enumerate(json.loads((root/'video.json').read_text())['frames']):
            seq += 1
            emit(type='frame', session_id=session, seq=seq, window_id=42, geometry_revision=generation,
                 width=640, height=480, codec='h264', data=frame, stream_id=generation,
                 key_frame=index == 0, encoded_width=640, encoded_height=480)
            time.sleep(.01)
    elif item['type'] == 'clipboard.copy':
        seq += 1
        emit(type='clipboard.data', session_id=session, seq=seq, window_id=42,
             request_id=item['request_id'], text=f'clipboard-{session}')
    elif item['type'] == 'session.detach':
        break
