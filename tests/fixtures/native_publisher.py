#!/usr/bin/env python3
"""Isolated RWP publisher used only by native_transport_smoke.py."""
import json
import os
from pathlib import Path
import select
import sys
import time

root=Path(__file__).parent
count=root/'connections'
n=int(count.read_text())+1 if count.exists() else 1
count.write_text(str(n))
pending=b''
video=False
attached=False
last_seq=0
def emit(**value):
    global last_seq
    last_seq=max(last_seq,value.get('seq',0))
    print(json.dumps(dict(value,version=1)),flush=True)
with (root/'requests').open('a',buffering=1) as log:
    while not (n==1 and (root/'cut').exists()):
        if attached and (root/'end-session').exists():
            emit(type='session.exited',session_id='retained',seq=last_seq+1)
            break
        if b'\n' not in pending:
            if not select.select([sys.stdin],[],[],.02)[0]:continue
            chunk=os.read(sys.stdin.fileno(),65536)
            if not chunk:break
            pending+=chunk
            if b'\n' not in pending:continue
        line,_,pending=pending.partition(b'\n')
        item=json.loads(line);log.write(json.dumps(item)+'\n')
        if item['type']=='node.info':emit(type='node.info',sharing_enabled=True)
        if item['type']=='hello':emit(type='hello',capabilities=['input.text.commit','native.h264'])
        if item['type']=='video.enable':
            video=True;emit(type='video.ready',codec='h264')
        if item['type']=='session.resume':
            attached=True
            emit(type='session.snapshot',session_id='retained',seq=100 if n==1 else 1,
                windows=[dict(window_id=42,title='Native reconnect fixture',width=640,height=480,
                    geometry_revision=n,scale_factor=1,visible=True,role='toplevel')])
            emit(type='input.authorization',session_id='retained',seq=101 if n==1 else 2,state='pending')
            emit(type='input.authorization',session_id='retained',seq=102 if n==1 else 3,state='ready')
            assert video, 'H.264 negotiation is required before attaching'
            frames=json.loads((root/'video.json').read_text())['frames']
            for index,frame in enumerate(frames):
                fields=dict(codec='h264',data=frame,stream_id=n,key_frame=index==0,encoded_width=640,encoded_height=480)
                emit(type='frame',session_id='retained',seq=104+index,window_id=42,geometry_revision=n,
                    width=640,height=480,**fields)
                time.sleep(.02)
        if item['type']=='session.detach':break
