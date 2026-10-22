#!/usr/bin/env python3
"""Opt-in dual-machine test. Controls only MacPublishingFixture's exact window.

Requires user-enabled Mac sharing, recording/accessibility permission and an
already authenticated return route. Never enables sharing or grants permission.
"""
import argparse
from pathlib import Path
import sys
import time
import uuid

root=Path(__file__).resolve().parents[2]
sys.path.insert(0,str(root/'apps/linux-client'))
from wire import request
import wire
import clipboard
from windowhost import Gtk,Gdk,Viewer

parser=argparse.ArgumentParser(description=__doc__)
parser.add_argument('--peer',required=True)
parser.add_argument('--state',type=Path,help='isolated receiver state directory (contains network/ for QUIC)')
parser.add_argument('--capture-only',action='store_true',help='verify real video/reconnect without sending input')
parser.add_argument('--hold-seconds',type=float,default=0,help='keep capture ready while an independent forward-link test runs')
args=parser.parse_args();uuid.UUID(args.peer.removeprefix('quic:'))
if not 0 <= args.hold_seconds <= 120 or (args.hold_seconds and not args.capture_only):
    parser.error('--hold-seconds requires --capture-only and must be between 0 and 120')
if args.state:
    wire.STATE=args.state.resolve()
deadline=time.monotonic()+30
while True:
    try:
        info=request(args.peer,'node.info');break
    except (OSError,EOFError):
        if time.monotonic()>=deadline:raise
        time.sleep(.25)
assert info['sharing_enabled'] and info['permissions']['screen_recording'], 'user sharing/recording permission required'
assert args.capture_only or info['permissions']['accessibility'], 'user accessibility permission required'
apps=request(args.peer,'app.list')['apps']
assert not any('WindowHosts' in a.get('id','') or a.get('name')=='窗渡 PaneFerry WindowHost' for a in apps)
windows=request(args.peer,'window.list')['windows']
targets=[w for w in windows if w['title']=='窗渡 PaneFerry Mac 发布测试' and w['name']=='PaneFerry Mac Test' and not w.get('minimized')]
assert len(targets)==1, 'expected exactly one visible dedicated Mac fixture'
target=targets[0]
response=request(args.peer,'app.launch','session.started',app_id=target['id'],request_id=uuid.uuid4().hex,backend='native',attach=False)
sid=response['session_id']
class FixedViewer(Viewer):
    def allocated(self,*args):
        pass  # Keep initial 2x source geometry until the explicit resize check.
    def send(self,**fields):
        if not args.capture_only:
            return super().send(**fields)
app=Gtk.Application(application_id='dev.lra.LiveReceiverFixture');app.register(None)
viewer=FixedViewer(app,args.peer,'Mac 发布实测',session_id=sid)
def until(predicate,timeout=12):
    deadline=time.monotonic()+timeout
    while not predicate():
        assert time.monotonic()<deadline,('live check timed out',viewer.notice.get_text(),viewer.ready,viewer.epoch)
        while Gtk.events_pending():Gtk.main_iteration_do(False)
        time.sleep(.01)
def send(**fields):
    assert viewer.ready
    viewer.send(**fields)
def key(name):
    send(type='input.key.logical',key=name,pressed=True);send(type='input.key.logical',key=name,pressed=False)
def click(x,y):
    send(type='input.pointer',x=x,y=y,button=1,pressed=True);send(type='input.pointer',x=x,y=y,button=1,pressed=False)
def reconnect_and_detach():
    old_epoch,old_pixels=viewer.epoch,viewer.pixels
    viewer.wire.socket.shutdown(2)
    until(lambda:viewer.epoch>old_epoch and viewer.ready and viewer.pixels is not old_pixels,timeout=20)
    assert viewer.session_id==sid,'reconnected to a different session'
    viewer.close()
    until(lambda:not next(s for s in request(args.peer,'session.list')['sessions'] if s['session_id']==sid)['controlled'])
    assert any(w['id']==target['id'] for w in request(args.peer,'window.list')['windows']), 'local close ended remote app'
try:
    started=time.monotonic()
    until(lambda:viewer.ready and viewer.pixels is not None)
    assert viewer.window_id==target['window_id']
    first=(viewer.width,viewer.height)
    print('Real H.264 first frame:',first,'startup_seconds:',round(time.monotonic()-started,3),flush=True)
    if args.capture_only:
        epoch=viewer.epoch
        deadline=time.monotonic()+args.hold_seconds
        while time.monotonic()<deadline:
            while Gtk.events_pending():Gtk.main_iteration_do(False)
            assert viewer.ready and viewer.epoch==epoch,'capture disconnected during parallel-link check'
            time.sleep(.01)
        reconnect_and_detach()
        print('PASS: actual Mac SCK/H264 -> Ubuntu GTK; new decoded frame after same-session reconnect; detach preserves Mac app; no input sent')
        sys.exit(0)
    # Source fixture content is 800x560 points, with the system titlebar above.
    # SCK publishes at 2x. These hit only the fixture's fixed button/text area.
    click(208,140)
    click(160,310)
    board=Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
    send(type='input.key.logical',key='Control_L',pressed=True);key('a');send(type='input.key.logical',key='Control_L',pressed=False)
    request_id=uuid.uuid4().hex;viewer.copy_request=(request_id,clipboard.begin_copy())
    send(type='clipboard.inline.copy',request_id=request_id,cut=False)
    until(lambda:viewer.copy_request is None)
    assert board.wait_for_text()=='点击计数 1','native button click did not increment exactly once'
    viewer.im.emit('commit','反向链路中文🙂 Mac → Ubuntu')
    send(type='input.key.logical',key='Control_L',pressed=True);key('a');send(type='input.key.logical',key='Control_L',pressed=False)
    request_id=uuid.uuid4().hex
    viewer.copy_request=(request_id,clipboard.begin_copy())
    send(type='clipboard.inline.copy',request_id=request_id,cut=False)
    until(lambda:viewer.copy_request is None)
    assert board.wait_for_text()=='反向链路中文🙂 Mac → Ubuntu','Mac text input/copy round trip failed'
    board.set_text('剪贴板从 Ubuntu 到 Mac',-1);viewer.paste()
    send(type='input.key.logical',key='Control_L',pressed=True);key('a');send(type='input.key.logical',key='Control_L',pressed=False)
    request_id=uuid.uuid4().hex;viewer.copy_request=(request_id,clipboard.begin_copy())
    send(type='clipboard.inline.copy',request_id=request_id,cut=False)
    until(lambda:viewer.copy_request is None)
    assert board.wait_for_text()=='剪贴板从 Ubuntu 到 Mac','paste round trip failed'
    request_id=uuid.uuid4().hex;viewer.copy_request=(request_id,clipboard.begin_copy())
    send(type='clipboard.inline.copy',request_id=request_id,cut=True)
    until(lambda:viewer.copy_request is None)
    assert board.wait_for_text()=='剪贴板从 Ubuntu 到 Mac','native cut round trip failed'
    # Clear any remaining selection. A failed cut would then retain old text,
    # so the following assertion also verifies actual deletion and key release.
    key('Left');viewer.im.emit('commit','剪切后输入正常🙂')
    send(type='input.key.logical',key='Control_L',pressed=True);key('a');send(type='input.key.logical',key='Control_L',pressed=False)
    request_id=uuid.uuid4().hex;viewer.copy_request=(request_id,clipboard.begin_copy())
    send(type='clipboard.inline.copy',request_id=request_id,cut=False)
    until(lambda:viewer.copy_request is None)
    assert board.wait_for_text()=='剪切后输入正常🙂','text after native cut was corrupted'
    send(type='window.configure',width=1200,height=840)
    until(lambda:viewer.width==1200 and viewer.height==840 and viewer.pixels.get_width()==1200 and viewer.pixels.get_height()==840)
    reconnect_and_detach()
    print('PASS: actual Mac SCK/H264 -> Ubuntu GTK; click, controller Unicode, native shortcuts, bidirectional text clipboard, cut/deletion/key release, resize, same-session reconnect, detach preserves Mac app; initial resolution',first)
finally:
    viewer.close()
