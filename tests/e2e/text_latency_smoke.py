#!/usr/bin/env python3
"""Measure real committed-text latency versus physical keys on an isolated GTK editor."""
import argparse
import json
import os
from pathlib import Path
import queue
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
parser.add_argument('--max-text-ms', type=float, default=0)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='paneferry-text-latency-') as temporary:
    home = Path(temporary)
    config = home / '.config/lan-remoteapp'
    config.mkdir(parents=True)
    state = home / 'received.json'
    (config / 'apps.json').write_text(json.dumps([dict(id='editor', name='Editor', executable=sys.executable,
        argv=[str(Path(__file__).with_name('clipboard_editor_fixture.py').resolve()),
              str(home/'text'), str(home/'metrics'), str(state)])]))
    env = dict(os.environ, HOME=str(home), XMODIFIERS='@im=none', GTK_IM_MODULE='simple')
    for name in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER'):
        env.pop(name, None)
    process = subprocess.Popen([str(args.agent.resolve()), 'serve'], env=env,
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        path = home / '.local/state/lan-remoteapp/agent.sock'
        deadline = time.monotonic() + 10
        while not path.exists():
            assert time.monotonic() < deadline
            time.sleep(.02)
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(10)
            connection.connect(str(path))
            wire = connection.makefile('rb', buffering=65536)
            events = queue.Queue()
            running = True
            def reader():
                try:
                    while running:
                        line = wire.readline(16 * 1024 * 1024 + 1)
                        if not line: break
                        event = json.loads(line)
                        if event['type'] != 'frame': events.put(event)
                except OSError: pass
            reader_thread = threading.Thread(target=reader, daemon=True)
            reader_thread.start()
            def send(**data): connection.sendall((json.dumps(dict(data, version=1))+'\n').encode())
            def until(kind):
                while True:
                    event = events.get(timeout=10)
                    assert event['type'] != 'error', event
                    if event['type'] == kind: return event
            def wait_text(expected):
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline:
                    try: value = json.loads(state.read_text())
                    except (FileNotFoundError, json.JSONDecodeError): value = {}
                    if value.get('text') == expected: return value['received']
                    time.sleep(.002)
                raise AssertionError(('text order/content mismatch', expected, value))
            send(type='hello'); hello=until('hello')
            if 'native.h264' in hello['capabilities']:
                send(type='video.enable', codec='h264'); until('video.ready')
            send(type='app.launch', app_id='editor', request_id='editor', backend='native')
            until('session.started')
            window=until('window.created')
            wid=window['window_id']
            send(type='window.focus', window_id=wid)
            text=''
            times={}
            for mode in ('key', 'text'):
                samples=[]
                for _ in range(5):
                    started=time.monotonic()
                    if mode=='key':
                        send(type='input.key', window_id=wid, keysym=ord('x'), pressed=True)
                        send(type='input.key', window_id=wid, keysym=ord('x'), pressed=False)
                        text+='x'
                    else:
                        send(type='input.text.commit', window_id=wid, text='文')
                        text+='文'
                    samples.append((wait_text(text)-started)*1000)
                times[mode+'_ms']=[round(v,1) for v in samples]
                times['median_'+mode+'_ms']=round(statistics.median(samples),1)
            started=time.monotonic()
            for _ in range(30): send(type='input.text.commit', window_id=wid, text='a')
            text+='a'*30
            times['burst_30_ms']=round((wait_text(text)-started)*1000,1)
            send(type='input.text.commit', window_id=wid, text='a')
            send(type='input.text.commit', window_id=wid, text='b')
            send(type='input.key', window_id=wid, keysym=0xff08, pressed=True)
            send(type='input.key', window_id=wid, keysym=0xff08, pressed=False)
            send(type='input.text.commit', window_id=wid, text='中🙂')
            wait_text(text+'a中🙂')
            print(json.dumps(dict(result='PASS', **times, checks=['key vs committed text', '30 ordered text commits', 'backspace barrier', 'Unicode order'])))
            if args.max_text_ms: assert times['median_text_ms'] < args.max_text_ms, times
            running=False
            connection.shutdown(socket.SHUT_RDWR)
            reader_thread.join(timeout=2)
            wire.close()
    finally:
        process.terminate(); process.wait(timeout=10)
