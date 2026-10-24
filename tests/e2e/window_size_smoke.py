#!/usr/bin/env python3
"""Real private Xvfb/Openbox/H.264: tall hints, bottom input, failures and recovery."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', type=Path, required=True)
args = parser.parse_args()

class Peer:
    def __init__(self, path, video=False):
        self.socket = socket.socket(socket.AF_UNIX); self.socket.settimeout(15)
        self.socket.connect(str(path)); self.file = self.socket.makefile('rwb')
        self.send(type='hello'); self.until('hello')
        if video:
            self.send(type='video.enable', codec='h264', max_fps=30); self.until('video.ready')
    def send(self, **fields):
        self.file.write(json.dumps(dict(fields, version=1)).encode()+b'\n'); self.file.flush()
    def until(self, kind, predicate=lambda _event: True):
        for _ in range(256):
            line = self.file.readline(16*1024*1024+1)
            assert line.endswith(b'\n') and len(line) <= 16*1024*1024
            event = json.loads(line)
            assert event['type'] != 'error' or kind == 'error', event
            if event['type'] == kind and predicate(event): return event
        raise AssertionError('missing '+kind)
    def close(self):
        self.socket.shutdown(socket.SHUT_RDWR); self.file.close(); self.socket.close()

def wait(predicate):
    deadline = time.monotonic()+10
    while time.monotonic() < deadline:
        value = predicate()
        if value: return value
        time.sleep(.03)
    raise AssertionError('fixture state timeout')

with tempfile.TemporaryDirectory(prefix='paneferry-window-size-') as temporary:
    task_root = Path(temporary); config = task_root/'.config/lan-remoteapp'; config.mkdir(parents=True)
    fixture = str(Path(__file__).with_name('window_size_fixture.py').resolve())
    entries = [dict(id=name, name=name, executable=sys.executable,
                    argv=[fixture, str(task_root/(name+'.json')), str(task_root/(name+'.command')), str(height)])
               for name,height in [('tall',2400),('impossible',3000)]]
    (config/'apps.json').write_text(json.dumps(entries))
    env = dict(os.environ, HOME=temporary, XDG_CONFIG_HOME=str(task_root/'.config'),
               XDG_DATA_HOME=str(task_root/'data'), XDG_DATA_DIRS='/usr/share', GTK_IM_MODULE='simple')
    for key in ('DISPLAY','WAYLAND_DISPLAY','DBUS_SESSION_BUS_ADDRESS','XAUTHORITY'): env.pop(key,None)
    agent = subprocess.Popen([str(args.agent.resolve()),'serve'],env=env,stderr=subprocess.PIPE)
    clients = []
    try:
        path = task_root/'.local/state/lan-remoteapp/agent.sock'; wait(path.exists)
        client = Peer(path,True); clients.append(client)
        client.send(type='app.launch',app_id='tall',request_id='tall',backend='native',scale_factor=2)
        sid = client.until('session.started')['session_id']
        window = client.until('window.created'); wid = window['window_id']
        frame = client.until('frame',lambda e:e['window_id']==wid)
        assert frame['codec']=='h264' and frame['key_frame']
        width,height = frame['width'],frame['height']
        assert 1920 <= width <= 3333 and height==2400 and width*height <= 8_000_000, (width,height)
        client.send(type='input.pointer',window_id=wid,x=width//2,y=height-30,button=1,pressed=True)
        client.send(type='input.pointer',window_id=wid,x=width//2,y=height-30,button=1,pressed=False)
        def clicked():
            d=json.loads((task_root/'tall.json').read_text()); return d if d['clicks'] else None
        clicked = wait(clicked)
        assert clicked['last_click'][1] > 2160, clicked
        client.close(); clients.remove(client)
        time.sleep(.2)
        client=Peer(path,True);clients.append(client)
        client.send(type='session.resume',session_id=sid);client.until('session.snapshot')
        assert client.until('frame')['key_frame']
        client.close();clients.remove(client)

        bad=Peer(path,True);clients.append(bad)
        started=time.monotonic()
        bad.send(type='app.launch',app_id='impossible',request_id='impossible',backend='native',scale_factor=2)
        assert bad.until('error')['code']=='window_size_unavailable'
        assert time.monotonic()-started < 8, 'constraint error was treated as 10-second startup wait'
        before=wait(lambda:json.loads((task_root/'impossible.json').read_text()) if (task_root/'impossible.json').exists() else None)
        inventory=Peer(path);clients.append(inventory)
        inventory.send(type='session.list')
        row=next(s for s in inventory.until('session.list')['sessions'] if s['app_id']=='impossible')
        assert row['window_count'] is None and row['unavailable_reason']=='window_size_unavailable',row
        (task_root/'impossible.command').write_text(json.dumps({'minimum_height':2400}))
        wait(lambda:not (task_root/'impossible.command').exists())
        bad.send(type='app.launch',app_id='impossible',request_id='impossible-retry',backend='native',scale_factor=2)
        assert bad.until('session.started')['session_id']==row['session_id']
        fixed=bad.until('window.created'); assert fixed['window_id']==before['xid']
        assert bad.until('frame')['codec']=='h264'
        after=json.loads((task_root/'impossible.json').read_text()); assert after['pid']==before['pid']
        print(json.dumps({'result':'PASS','tall_frame':[width,height],'bottom_click':clicked['last_click'],
            'checks':['real H.264','bottom input beyond 2160','same-session reconnect','impossible constraints reported',
                      'inventory error visible','hint correction recovers same process and XID']}))
    finally:
        for client in clients: client.close()
        agent.terminate();agent.wait(timeout=10)
