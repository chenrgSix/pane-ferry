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

path=Path(temporary.name)/'publisher.sock'
pipeline=Gst.parse_launch('videotestsrc num-buffers=1 ! video/x-raw,format=I420,width=640,height=480,framerate=30/1 ! openh264enc ! h264parse config-interval=-1 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=out sync=false')
pipeline.set_state(Gst.State.PLAYING)
sample=pipeline.get_by_name('out').emit('try-pull-sample',5*Gst.SECOND)
assert sample,'video fixture encoder unavailable'
buffer=sample.get_buffer();raw=buffer.extract_dup(0,buffer.get_size());pipeline.set_state(Gst.State.NULL)
class PublisherFixture:
    def __init__(self, path):
        self.listener=socket.socket(socket.AF_UNIX);self.listener.bind(str(path));self.listener.listen()
        self.events=[];self.connections=[];self.members={42:dict(window_id=42,title='fixture',width=640,height=480,geometry_revision=1,scale_factor=1)}
        threading.Thread(target=self.serve,daemon=True).start()
    def serve(self):
        while not stop.is_set():
            try: sock,_=self.listener.accept()
            except OSError:return
            self.connections.append(sock)
            threading.Thread(target=self.client,args=(sock,len(self.connections)),daemon=True).start()
    def client(self,sock,stream):
        wire=sock.makefile('rb');seq=0
        def send(kind,**fields):
            nonlocal seq
            seq+=1;sock.sendall((json.dumps(dict(type=kind,version=1,session_id='fixture',seq=seq,**fields))+'\n').encode())
        def frame(wid):
            send('frame',window_id=wid,width=640,height=480,encoded_width=640,encoded_height=480,geometry_revision=1,stream_id=stream,key_frame=True,codec='h264',data=base64.b64encode(raw).decode())
        try:
            for line in wire:
                event=json.loads(line);kind=event['type'];self.events.append(event)
                if kind=='hello':send('hello',capabilities=['native.h264','input.key.logical'])
                elif kind=='video.enable':send('video.ready',codec='h264')
                elif kind=='session.resume':
                    send('session.snapshot',windows=list(self.members.values()))
                    for wid in self.members:frame(wid)
                elif kind=='clipboard.inline.copy' and event['request_id']=='copy-error-child':send('error',code='clipboard_copy_failed',window_id=event['window_id'])
                elif kind=='clipboard.inline.copy' and event['request_id']=='copy-error-legacy':send('error',code='clipboard_copy_failed')
                elif kind=='clipboard.inline.copy':send('clipboard.inline.data',window_id=event['window_id'],request_id=event['request_id'],bundle=dict(kind='text',text='中文剪贴板 fixture'))
                elif kind=='input.key.logical' and event['key']=='F2' and event['pressed'] and 43 not in self.members:
                    self.members[43]=dict(window_id=43,parent_id=42,title='preview',width=640,height=480,geometry_revision=1,scale_factor=1)
                    send('window.created',**self.members[43]);frame(43)
                elif kind=='window.close.request' and event['window_id']==43:
                    self.members.pop(43,None);send('window.destroyed',window_id=43)
                    frame(43)  # A delayed frame must never resurrect a closed child.
        except OSError:pass
        finally:wire.close();sock.close()
    def close(self):
        self.listener.close()
        for sock in self.connections:
            try:sock.shutdown(socket.SHUT_RDWR)
            except OSError:pass
stop=threading.Event();network_processes=[]
fixture=PublisherFixture(path);other_path=Path(temporary.name)/'other.sock';other=PublisherFixture(other_path)
events,connections=fixture.events,fixture.connections
binary=root/'target/debug/paneferry-network'
def manage(directory,**body):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(10);client.connect(str(directory/'control.sock'))
        client.sendall(json.dumps(body).encode()+b'\n')
        result=json.loads(client.makefile('rb').readline())
        assert 'error' not in result,result
        return result
directories=[STATE/'network',Path(temporary.name)/'publisher-network',Path(temporary.name)/'other-network']
for index,directory in enumerate(directories):
    network_processes.append(subprocess.Popen([str(binary),'serve','--state',str(directory),'--publisher',str(other_path if index==2 else path),'--listen','127.0.0.1:0'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL))
for directory in directories:
    deadline=time.monotonic()+10
    while not (directory/'control.sock').exists():
        assert time.monotonic()<deadline;time.sleep(.02)
remote=manage(directories[1],op='status')
manage(directories[0],op='pair',invitation=manage(directories[1],op='invite')['invitation'],address=remote['listen'])
peer='quic:'+remote['device_id']
remote_other=manage(directories[2],op='status')
manage(directories[0],op='pair',invitation=manage(directories[2],op='invite')['invitation'],address=remote_other['listen'])
other_peer='quic:'+remote_other['device_id']
deadline=time.monotonic()+5
while not (directories[0]/('r-'+remote['device_id']+'.sock')).exists():
    assert time.monotonic()<deadline;time.sleep(.02)
from main import Manager
from types import SimpleNamespace
opened=[]
manager=SimpleNamespace(activate=lambda *_:None,open_viewer=lambda *a,**kw:opened.append((a,kw)))
for invalid in [peer[5:], 'legacy:'+peer[5:], 'quic:invalid', peer.upper()]:
    command=SimpleNamespace(get_arguments=lambda value=invalid:['paneferry','--peer',value,'--session','fixture'])
    assert Manager.command_line(manager,None,command)==1
assert not opened
command=SimpleNamespace(get_arguments=lambda:['paneferry','--peer',peer,'--session','fixture'])
assert Manager.command_line(manager,None,command)==0 and opened[0][0][0]==peer
app=Gtk.Application(application_id='dev.lra.ReceiverFixture');app.register(None)
viewer=Viewer(app,peer,'fixture',session_id='fixture')
other_viewer=None
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
    for bad in [[{}], [dict(window_id=1,width=1,height=1,geometry_revision=1,parent_id=1)],
                [dict(window_id=1,width=1,height=1,geometry_revision=1)]*2,
                [dict(window_id=i+1,width=2000,height=2000,geometry_revision=1) for i in range(5)]]:
        try:Viewer.validate_windows(bad);raise AssertionError('invalid membership accepted')
        except ValueError:pass
    other_viewer=Viewer(app,other_peer,'other fixture',session_id='fixture')
    until(lambda:other_viewer.ready and other_viewer.pixels is not None)
    assert viewer.members is not other_viewer.members and viewer.window_id==other_viewer.window_id==42
    viewer.send(type='input.key.logical',key='F2',pressed=True)
    viewer.send(type='input.key.logical',key='F2',pressed=False)
    until(lambda:43 in viewer.members and viewer.members[43].pixels is not None)
    child=viewer.members[43]
    assert child.window.get_transient_for() is viewer.window and set(other_viewer.members)=={42}
    child.im.emit('commit','preview child input')
    until(lambda:any(e['type']=='input.text.commit' and e['window_id']==43 for e in events))
    assert not any(e['type']=='input.text.commit' for e in other.events)
    viewer.copy_request=('root-pending',clipboard.begin_copy())
    child.copy_request=('copy-error-child',clipboard.begin_copy())
    child.send(type='clipboard.inline.copy',request_id='copy-error-child',cut=False)
    until(lambda:child.copy_request is None)
    assert viewer.copy_request is not None, 'a child error canceled another window request'
    child.copy_request=('copy-error-legacy',clipboard.begin_copy())
    child.send(type='clipboard.inline.copy',request_id='copy-error-legacy',cut=False)
    until(lambda:child.copy_request is None and viewer.copy_request is None)
    root_native,child_native=viewer.window,child.window
    old=connections[-1];old.shutdown(socket.SHUT_RDWR)
    until(lambda:len(connections)>=2 and viewer.ready and child.ready and child.pixels is not None,timeout=12)
    assert viewer.window is root_native and viewer.members[43].window is child_native, 'reconnect replaced native windows'
    assert all(e['session_id']=='fixture' for e in events if e['type']=='session.resume')
    assert len([e for e in events if e['type']=='input.text.commit'])==2,'input replayed after reconnect'
    assert not any(e['type']=='app.launch' for e in events),'resume relaunched app'
    other_viewer.send(type='input.key.logical',key='F2',pressed=True)
    other_viewer.send(type='input.key.logical',key='F2',pressed=False)
    until(lambda:43 in other_viewer.members and other_viewer.members[43].pixels is not None)
    assert other_viewer.members[43] is not child and other_viewer.members[43].window.get_transient_for() is other_viewer.window
    child.close()
    until(lambda:43 not in viewer.members)
    assert viewer.ready and not viewer.closed.is_set() and other_viewer.members[43].ready
    child.im.emit('commit','stale input')
    assert child.closed.is_set()
    old_epoch=viewer.epoch
    child.image(viewer.pixels,old_epoch,child.revision,object())
    assert child.pending_image is None, 'closed window accepted a late decoded image'
    other_viewer.close();viewer.close();until(lambda:viewer.closed.is_set())
    assert [e['window_id'] for e in events if e['type']=='window.close.request']==[43]
    assert not any(e['type']=='window.close.request' for e in other.events), 'main close unexpectedly closed a remote window'
    print('Native QUIC: PASS: real GTK image, H.264/SPS bounds, IM/keys/pointer, text/image/file clipboard and reconnect without replay')
    print('Native QUIC: PASS: associated windows, per-window input and child close, native-window retention on reconnect, late-frame rejection, three paired devices with identical session/window IDs')
finally:
    viewer.close()
    if other_viewer:other_viewer.close()
    stop.set();fixture.close();other.close()
    for process in network_processes:
        process.terminate();process.wait(timeout=5)
    temporary.cleanup()
