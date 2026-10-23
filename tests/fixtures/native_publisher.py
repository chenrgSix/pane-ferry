#!/usr/bin/env python3
"""Isolated RWP publisher used only by native_transport_smoke.py."""
import base64
import struct
import zlib
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
def png():
    def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    raw=(b'\x00'+bytes([255,0,0])*640)*480
    return base64.b64encode(b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>2I5B',640,480,8,2,0,0,0))+chunk(b'sRGB',bytes([0]))+chunk(b'IDAT',zlib.compress(raw))+chunk(b'IEND',b'')).decode()
def emit(**value):
    print(json.dumps(dict(value,version=1)),flush=True)
with (root/'requests').open('a',buffering=1) as log:
    while not (n==1 and (root/'cut').exists()):
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
            emit(type='session.snapshot',session_id='retained',seq=100 if n==1 else 1,
                windows=[dict(window_id=42,title='Native reconnect fixture',width=640,height=480,
                    geometry_revision=n,scale_factor=1,visible=True,role='toplevel')])
            emit(type='input.authorization',session_id='retained',seq=101 if n==1 else 2,state='pending')
            emit(type='input.authorization',session_id='retained',seq=102 if n==1 else 3,state='ready')
            frames=json.loads((root/'video.json').read_text())['frames'] if video else [png()]
            for index,frame in enumerate(frames):
                fields=dict(codec='h264',data=frame,stream_id=n,key_frame=index==0,encoded_width=640,encoded_height=480) if video else dict(png=frame)
                emit(type='frame',session_id='retained',seq=104+index,window_id=42,geometry_revision=n,
                    width=640,height=480,**fields)
                time.sleep(.02)
        if item['type']=='session.detach':break
