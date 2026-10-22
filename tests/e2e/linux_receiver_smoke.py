#!/usr/bin/env python3
"""Real GTK/GStreamer receiver against an isolated protocol fixture, under Xvfb."""
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

root=Path(__file__).resolve().parents[2]
temporary=tempfile.TemporaryDirectory(prefix='lra-rx-')
os.environ['HOME']=temporary.name
sys.path.insert(0,str(root/'apps/linux-client'))
from wire import STATE,private,save
import clipboard
from windowhost import Gtk,Gdk,GLib,Gst,GdkPixbuf,Viewer,check_sps

private(STATE/'peers')
peer=str(uuid.uuid4());generation=uuid.uuid4().hex
path=STATE/'peers'/('p'+generation+'.sock')
save(STATE/'peers'/(peer+'.json'),dict(version=1,device_id=peer,name='fixture',generation=generation,socket=str(path),transport='ssh-reverse'))
pipeline=Gst.parse_launch('videotestsrc num-buffers=1 ! video/x-raw,format=I420,width=640,height=480,framerate=30/1 ! openh264enc ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=out sync=false')
pipeline.set_state(Gst.State.PLAYING)
sample=pipeline.get_by_name('out').emit('try-pull-sample',5*Gst.SECOND)
assert sample,'video fixture encoder unavailable'
buffer=sample.get_buffer();raw=buffer.extract_dup(0,buffer.get_size());pipeline.set_state(Gst.State.NULL)
listener=socket.socket(socket.AF_UNIX);listener.bind(str(path));listener.listen()
events=[];connections=[];stop=threading.Event();network_processes=[]
def serve():
    while not stop.is_set():
        try: sock,_=listener.accept()
        except OSError:return
        connections.append(sock);wire=sock.makefile('rb')
        seq=0
        def send(kind,**fields):
            nonlocal seq
            seq+=1;sock.sendall((json.dumps(dict(type=kind,version=1,session_id='fixture',seq=seq,**fields))+'\n').encode())
        try:
            for line in wire:
                event=json.loads(line);kind=event['type'];events.append(event)
                if kind=='hello':send('hello',capabilities=['native.h264','input.key.logical'])
                elif kind=='video.enable':send('video.ready',codec='h264')
                elif kind=='session.resume':
                    send('session.snapshot',windows=[dict(window_id=42,title='fixture',width=640,height=480,geometry_revision=1,scale_factor=1)])
                    send('frame',window_id=42,width=640,height=480,encoded_width=640,encoded_height=480,geometry_revision=1,stream_id=len(connections),key_frame=True,codec='h264',data=base64.b64encode(raw).decode())
                elif kind=='clipboard.inline.copy':send('clipboard.inline.data',window_id=42,request_id=event['request_id'],bundle=dict(kind='text',text='中文剪贴板 fixture'))
        except OSError:pass
        finally:wire.close();sock.close()
threading.Thread(target=serve,daemon=True).start()
if '--native' in sys.argv:
    binary=root/'target/debug/remoteapp-network'
    def manage(directory,**body):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(10);client.connect(str(directory/'control.sock'))
            client.sendall(json.dumps(body).encode()+b'\n')
            result=json.loads(client.makefile('rb').readline())
            assert 'error' not in result,result
            return result
    directories=[STATE/'network',Path(temporary.name)/'publisher-network']
    for directory in directories:
        network_processes.append(subprocess.Popen([str(binary),'serve','--state',str(directory),'--publisher',str(path),'--listen','127.0.0.1:0'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL))
    for directory in directories:
        deadline=time.monotonic()+10
        while not (directory/'control.sock').exists():
            assert time.monotonic()<deadline;time.sleep(.02)
    remote=manage(directories[1],op='status')
    manage(directories[0],op='pair',invitation=manage(directories[1],op='invite')['invitation'],address=remote['listen'])
    peer='quic:'+remote['device_id']
    deadline=time.monotonic()+5
    while not (directories[0]/('r-'+remote['device_id']+'.sock')).exists():
        assert time.monotonic()<deadline;time.sleep(.02)
app=Gtk.Application(application_id='dev.lra.ReceiverFixture');app.register(None)
viewer=Viewer(app,peer,'fixture',session_id='fixture')
def until(predicate,timeout=8):
    deadline=time.monotonic()+timeout
    while not predicate():
        assert time.monotonic()<deadline,'condition timed out'
        while Gtk.events_pending():Gtk.main_iteration_do(False)
        time.sleep(.01)
try:
    until(lambda:viewer.ready and viewer.pixels is not None)
    assert (viewer.pixels.get_width(),viewer.pixels.get_height())==(640,480)
    viewer.im.emit('commit','中文🙂')
    until(lambda:any(e['type']=='input.text.commit' and e['text']=='中文🙂' for e in events))
    viewer.send(type='input.key.logical',key='Return',pressed=True)
    viewer.send(type='input.key.logical',key='Return',pressed=False)
    viewer.send(type='input.pointer',x=120,y=150,button=1,pressed=True)
    viewer.send(type='input.pointer',x=120,y=150,button=1,pressed=False)
    until(lambda:len([e for e in events if e['type']=='input.pointer'])==2)
    viewer.copy_request=('copy-test',clipboard.begin_copy())
    viewer.send(type='clipboard.inline.copy',request_id='copy-test',cut=False)
    until(lambda:viewer.copy_request is None)
    assert Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()=='中文剪贴板 fixture'
    viewer.paste();until(lambda:any(e['type']=='clipboard.inline.paste' for e in events))
    board=Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    image=GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB,True,8,32,24);image.fill(0x3a8050ff)
    board.set_image(image)
    image_bundle=clipboard.read(board);assert image_bundle['kind']=='image'
    clipboard.write(board,image_bundle)
    assert (board.wait_for_image().get_width(),board.wait_for_image().get_height())==(32,24)
    file_bundle=dict(kind='files',entries=[dict(path='folder',size=0,directory=True),dict(path='folder/中文.txt',size=3,directory=False,data=base64.b64encode(b'abc').decode())])
    clipboard.write(board,file_bundle)
    uris=board.wait_for_uris();assert len(uris)==1 and uris[0].endswith('/folder')
    copied=clipboard.read(board);assert copied['kind']=='files' and copied['entries'][1]['data']==base64.b64encode(b'abc').decode()
    viewer.paste();until(lambda:any(e['type']=='clipboard.inline.paste' and e['bundle']['kind']=='files' for e in events))
    try:check_sps(raw,4096,4096);raise AssertionError('false SPS dimensions accepted')
    except ValueError:pass
    for malicious in ['../file','/file','a//b','a\\b']:
        try:clipboard.validate(dict(kind='files',entries=[dict(path=malicious,size=0,directory=False,data='')]));raise AssertionError('unsafe file accepted')
        except ValueError:pass
    old=connections[-1];old.shutdown(socket.SHUT_RDWR)
    until(lambda:len(connections)>=2 and viewer.ready,timeout=12)
    assert all(e['session_id']=='fixture' for e in events if e['type']=='session.resume')
    assert len([e for e in events if e['type']=='input.text.commit'])==1,'input replayed after reconnect'
    assert not any(e['type']=='app.launch' for e in events),'resume relaunched app'
    viewer.close();until(lambda:viewer.closed.is_set())
    print(('Native QUIC: ' if '--native' in sys.argv else 'Compatibility socket: ')+'PASS: real GTK image, H.264 decode and SPS bounds, controller IM commit, keys/pointer, text/image/file clipboard, same-session automatic reconnect without replay')
finally:
    viewer.close();stop.set();listener.close()
    for sock in connections:
        try:sock.shutdown(socket.SHUT_RDWR)
        except OSError:pass
    for process in network_processes:
        process.terminate();process.wait(timeout=5)
    temporary.cleanup()
