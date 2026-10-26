#!/usr/bin/env python3
"""Opt-in: real GTK PNG/INCR and files both ways, private HOME/Xvfb only."""
import argparse
import hashlib
import io
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import zlib
parser=argparse.ArgumentParser(description=__doc__);parser.add_argument('--agent',required=True,type=Path);args=parser.parse_args()
children=[]
def start(argv,env):
    p=subprocess.Popen(argv,env=env,start_new_session=True,stdin=subprocess.DEVNULL,stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL);children.append(p);return p

def wait(check,label):
    limit=time.monotonic()+15
    while time.monotonic()<limit:
        if check():return
        time.sleep(.03)
    raise AssertionError(label)

class Peer:
    def __init__(self,path):
        self.s=socket.socket(socket.AF_UNIX);self.s.settimeout(35);self.s.connect(str(path));self.f=self.s.makefile('rwb',buffering=0);self.send(type='hello');assert self.read()['type']=='hello'
        self.send(type='video.enable',codec='h264',max_fps=30);self.until('video.ready')
    def send(self,**event):self.f.write(json.dumps(dict(event,version=1)).encode()+b'\n')
    def read(self):return json.loads(self.f.readline(16*1024*1024+1))
    def until(self,kind):
        while True:
            e=self.read();assert e['type']!='error' or kind=='error',e
            if e['type']==kind:return e
    def close(self):self.f.close();self.s.close()

def png():
    def chunk(kind,data):return struct.pack('>I',len(data))+kind+data+struct.pack('>I',zlib.crc32(kind+data))
    pixels=b''.join(b'\0'+secrets.token_bytes(1024*3) for _ in range(512))
    return b'\x89PNG\r\n\x1a\n'+chunk(b'IHDR',struct.pack('>IIBBBBB',1024,512,8,2,0,0,0))+chunk(b'IDAT',zlib.compress(pixels))+chunk(b'IEND',b'')

try:
 with tempfile.TemporaryDirectory(prefix='paneferry-rich-') as directory:
    root=Path(directory);home=root/'home';home.mkdir();config=home/'.config/lan-remoteapp';config.mkdir(parents=True)
    authority=root/'Xauthority';authority.touch(mode=0o600)
    display=next(i for i in range(1100,2000) if not Path(f'/tmp/.X11-unix/X{i}').exists())
    subprocess.run(['xauth','-f',str(authority),'add',f':{display}','MIT-MAGIC-COOKIE-1',secrets.token_hex(16)],check=True)
    env=dict(os.environ,HOME=str(home),DISPLAY=f':{display}',XAUTHORITY=str(authority),GTK_IM_MODULE='gtk-im-context-simple',XMODIFIERS='@im=none',GDK_BACKEND='x11')
    for name in ('WAYLAND_DISPLAY','SESSION_MANAGER','DBUS_SESSION_BUS_ADDRESS'):env.pop(name,None)
    start(['Xvfb',f':{display}','-screen','0','1000x700x24','-nolisten','tcp','-auth',str(authority)],env)
    wait(lambda:Path(f'/tmp/.X11-unix/X{display}').exists(),'Xvfb');start(['openbox'],env);time.sleep(.25)
    image=png();assert len(image)>1024*1024;(root/'image.png').write_bytes(image)
    (root/'command.json').write_text(json.dumps(dict(mime='image/png',source='image.png',request='image/png')))
    fixture=start([sys.executable,str(Path(__file__).with_name('rich_clipboard_fixture.py')),str(root)],env)
    wait(lambda:(root/'state.json').exists(),'GTK fixture');wid=json.loads((root/'state.json').read_text())['window']
    (config/'apps.json').write_text(json.dumps([dict(id='rich',name='rich',executable=str(Path(f'/proc/{fixture.pid}/exe').resolve()),existing=dict(display=display,xauthority=str(authority),process_id=fixture.pid))]))
    binary=str(args.agent.resolve());start([binary,'serve'],env);sock=home/'.local/state/lan-remoteapp/agent.sock';wait(sock.exists,'Agent')
    peer=Peer(sock);peer.send(type='app.launch',app_id='rich',request_id='rich',backend='native');peer.until('session.started');peer.until('frame')
    def upload(manifest,data):
        result=subprocess.run([binary,'clipboard-upload'],input=json.dumps(manifest).encode()+b'\n'+data,env=env,check=True,stdout=subprocess.PIPE,timeout=30);return result.stdout.decode().strip()
    def download(transfer):return subprocess.run([binary,'clipboard-download',transfer],env=env,check=True,stdout=subprocess.PIPE,timeout=30).stdout
    def paste(transfer,request):
        (root/'command.json').write_text(json.dumps(dict(mime=request,source='image.png',request=request)))
        (root/'result.json').unlink(missing_ok=True);peer.send(type='clipboard.paste.bundle',window_id=wid,transfer_id=transfer)
        wait(lambda:(root/'result.json').exists(),'native paste');return json.loads((root/'result.json').read_text())
    manifest=dict(kind='image',entries=[dict(path='image.png',size=len(image),directory=False)])
    tid=upload(manifest,image);answer=paste(tid,'image/png');assert answer['sha256']==hashlib.sha256(image).hexdigest(),answer
    print('PASS: >1MiB PNG uploaded via bounded binary stream and pasted through X11 INCR byte-for-byte',flush=True)
    peer.send(type='clipboard.copy.bundle',window_id=wid,request_id='image-copy');copied=peer.until('clipboard.bundle');data=download(copied['transfer_id']);meta,body=data.split(b'\n',1);assert body==image
    print('PASS: GTK PNG copied via X11 INCR and downloaded byte-for-byte',flush=True)
    files=[('report 中文.txt',b'file contents\0'+secrets.token_bytes(200000)),('folder',None),('folder/empty',b''),('folder/nested.bin',secrets.token_bytes(1200000))]
    manifest=dict(kind='files',entries=[dict(path=n,size=len(b) if b is not None else 0,directory=b is None) for n,b in files]);raw=b''.join(b for _,b in files if b is not None)
    tid=upload(manifest,raw);answer=paste(tid,'text/uri-list');expected={n:hashlib.sha256(b).hexdigest() for n,b in files if b is not None};assert answer['files']==expected,answer
    print('PASS: multiple files, Unicode/spaces, directory tree and empty file paste as native local file URIs with matching hashes',flush=True)
    sources=root/'sources';sources.mkdir();(sources/'real file.txt').write_bytes(b'reverse file\0contents');(sources/'folder').mkdir();(sources/'folder/nested').write_bytes(secrets.token_bytes(100000))
    uris='\r\n'.join(p.as_uri() for p in [sources/'real file.txt',sources/'folder'])+'\r\n';(root/'uris').write_text(uris)
    (root/'command.json').write_text(json.dumps(dict(mime='text/uri-list',source='uris',request='text/uri-list')))
    peer.send(type='clipboard.copy.bundle',window_id=wid,request_id='files-copy');copied=peer.until('clipboard.bundle');data=download(copied['transfer_id']);meta,body=data.split(b'\n',1);manifest=json.loads(meta);offset=0
    for e in manifest['entries']:
        if e['directory']:continue
        part=body[offset:offset+e['size']];offset+=e['size'];assert part==(sources/e['path']).read_bytes()
    assert offset==len(body)
    peer.send(type='clipboard.copy.bundle',window_id=wid,request_id='files-cut',cut=True);assert peer.until('error')['code']=='clipboard_cut_failed';assert not(root/'cut-called').exists()
    print('PASS: copied local file selections downloaded with original contents; cross-host file cut rejected before touching originals',flush=True)
    bad=dict(kind='files',entries=[dict(path='../outside',size=1,directory=False)])
    rejected=subprocess.run([binary,'clipboard-upload'],input=json.dumps(bad).encode()+b'\nx',env=env,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL);assert rejected.returncode!=0
    truncated=subprocess.run([binary,'clipboard-upload'],input=json.dumps(dict(kind='files',entries=[dict(path='short',size=10,directory=False)])).encode()+b'\nx',env=env,stdout=subprocess.PIPE,stderr=subprocess.DEVNULL);assert truncated.returncode!=0
    peer.close();print('PASS: traversal and truncated streams rejected; all fixtures isolated from the real desktop',flush=True)
finally:
 for child in reversed(children):
    if child.poll() is None:
        os.killpg(child.pid,signal.SIGTERM)
        try:child.wait(timeout=5)
        except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=5)
