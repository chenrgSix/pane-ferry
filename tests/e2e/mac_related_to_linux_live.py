#!/usr/bin/env python3
"""Opt-in real QUIC/SCK/GTK check; controls only MacRelatedWindowFixture."""
import argparse
from pathlib import Path
import sys
import time
import uuid

root=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(root/'apps/linux-client'))
import wire
import clipboard
from windowhost import Gtk,Gdk,Viewer
parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--peer',required=True)
parser.add_argument('--state',type=Path,required=True)
args=parser.parse_args();wire.STATE=args.state.resolve()
targets=wire.request(args.peer,'window.list')['windows']
target=next(w for w in targets if w['title']=='LAN RemoteApp Related Alpha' and w['name']=='RemoteApp Related Test')
sid=wire.request(args.peer,'app.launch','session.started',app_id=target['id'],request_id=uuid.uuid4().hex,backend='native',attach=False)['session_id']
class FixedViewer(Viewer):
    def allocated(self,*args): pass
app=Gtk.Application(application_id='dev.lra.RelatedReceiverFixture');app.register(None)
viewer=FixedViewer(app,args.peer,'related Mac fixture',session_id=sid)
def until(predicate,timeout=20):
    end=time.monotonic()+timeout
    while not predicate():
        assert time.monotonic()<end,('related live check timed out',viewer.notice.get_text())
        while Gtk.events_pending():Gtk.main_iteration_do(False)
        time.sleep(.01)
def key(view,name,pressed):view.send(type='input.key.logical',key=name,pressed=pressed)
def select_all(view):
    key(view,'Control_L',True);key(view,'a',True);key(view,'a',False);key(view,'Control_L',False)
def copy(view,text):
    select_all(view)
    identity=uuid.uuid4().hex;view.copy_request=(identity,clipboard.begin_copy())
    view.send(type='clipboard.inline.copy',request_id=identity,cut=False)
    until(lambda:view.copy_request is None)
    assert Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD).wait_for_text()==text, 'child clipboard roundtrip'
try:
    until(lambda:viewer.ready and viewer.pixels is not None)
    assert set(viewer.members)=={target['window_id']}
    key(viewer,'F2',True);key(viewer,'F2',False)
    until(lambda:len(viewer.members)==2 and all(v.pixels is not None for v in viewer.members.values()))
    child=next(v for v in viewer.members.values() if v is not viewer)
    assert child.window.get_transient_for() is viewer.window
    child.im.emit('commit','双机 QUIC 子进程🙂');copy(child,'双机 QUIC 子进程🙂')
    child_native=child.window
    for _ in range(2):
        epoch,previous=viewer.epoch,child.pixels
        viewer.wire.socket.shutdown(2)
        until(lambda:viewer.epoch>epoch and viewer.ready and child.ready and child.pixels is not previous)
        assert viewer.members[child.window_id] is child and child.window is child_native and viewer.session_id==sid
    child.close();until(lambda:child.window_id not in viewer.members)
    assert viewer.ready
    select_all(viewer);viewer.im.emit('commit','双机主窗口继续工作🙂');copy(viewer,'双机主窗口继续工作🙂')
    viewer.close()
    until(lambda:not next(s for s in wire.request(args.peer,'session.list')['sessions'] if s['session_id']==sid)['controlled'])
    assert any(w['id']==target['id'] for w in wire.request(args.peer,'window.list')['windows'])
    print('PASS: real Mac accessory helper -> QUIC -> GTK/H264; automatic related window, child Unicode/copy, two reconnects retaining native windows, child close and parent survival, detach preserves Mac app',flush=True)
finally:viewer.close()
