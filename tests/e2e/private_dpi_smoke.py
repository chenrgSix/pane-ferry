#!/usr/bin/env python3
"""Opt-in Linux: two owned displays, standard DPI, original process/content on resume.
No real desktop, application profiles, or app-specific scale arguments are used.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--agent', type=Path, required=True)
p.add_argument('--electron', type=Path, help='optional test-owned Electron binary; never the installed user app')
a = p.parse_args()
root = Path(__file__).resolve().parents[2]

def wait(fn):
    until = time.monotonic()+20
    while time.monotonic()<until:
        result = fn()
        if result: return result
        time.sleep(.05)
    raise AssertionError('DPI fixture timeout')

class Peer:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX)
        self.sock.settimeout(20)
        self.sock.connect(str(path))
        self.file = self.sock.makefile('rwb')
        self.send(type='hello')
        self.until('hello')
    def send(self, **event):
        self.file.write(json.dumps(dict(event,version=1)).encode()+b'\n')
        self.file.flush()
    def until(self, kind):
        until=time.monotonic()+20
        while time.monotonic()<until:
            event=json.loads(self.file.readline(16*1024*1024+1))
            assert event['type']!='error' or kind=='error', event
            if event['type']==kind:return event
        raise AssertionError('Missing '+kind)
    def close(self):self.file.close();self.sock.close()

def density_changes(peer, state, window, original):
    # The source application, not just frame metadata, must react to a display
    # change. Keep its process, contents and logical control size throughout.
    for scale in (1,2):
        peer.send(type='window.viewport',window_id=window,logical_width=1470,logical_height=802,scale_factor=scale)
        while True:
            frame=peer.until('frame')
            if (frame['width'],frame['height'])==(1470*scale,802*scale):break
        wait(lambda: (lambda d:(d['scale'],d['width'],d['height'])==(scale,1470,802))(json.loads(state.read_text())))
        actual=json.loads(state.read_text())
        for field in ('pid','text','button_height'):
            assert actual[field]==original[field], (field,original,actual)
        if 'text_pixels' in original:
            assert actual['text_pixels']==original['text_pixels'], (original,actual)
        print('PASS: live display density',scale,'actual pixels',frame['width'],frame['height'],'client',actual,flush=True)

with tempfile.TemporaryDirectory(prefix='paneferry-dpi-') as temporary:
    folder=Path(temporary); home=folder/'home'
    conf=home/'.config/lan-remoteapp';conf.mkdir(parents=True)
    apps=[dict(id=f'dpi-{scale}',name=f'DPI {scale}',executable='/usr/bin/python3',
               argv=[str(root/'tests/e2e/dpi_fixture.py'),str(folder/f'{scale}.json')]) for scale in (1,2)]
    if a.electron:
        apps += [dict(id=f'electron-{scale}',name=f'Chromium fixture {scale}',executable=str(a.electron.resolve()),
            # Ubuntu restricts unregistered user namespaces. This test-only,
            # local data: page has no network or user profile; production flags
            # and application sandboxes are never changed.
            argv=[str(root/'tests/e2e/dpi_electron_fixture.cjs'),str(folder/f'electron-{scale}.json'),'--no-sandbox']) for scale in (1,2)]
    (conf/'apps.json').write_text(json.dumps(apps))
    env=dict(os.environ,HOME=str(home),XDG_CONFIG_HOME=str(home/'.config'),
             XDG_DATA_HOME=str(home/'.local/share'),XDG_CACHE_HOME=str(home/'.cache'))
    for key in ('DISPLAY','XAUTHORITY','WAYLAND_DISPLAY','DBUS_SESSION_BUS_ADDRESS',
                'GDK_SCALE','GDK_DPI_SCALE','QT_SCALE_FACTOR','QT_FONT_DPI'):
        env.pop(key,None)
    peers=[]
    with (folder/'agent.log').open('w') as log:
        agent=subprocess.Popen([str(a.agent.resolve()),'serve'],env=env,stdout=log,stderr=log)
        try:
            sock=home/'.local/state/lan-remoteapp/agent.sock';wait(sock.exists)
            results=[]
            for scale in (1,2):
                peer=Peer(sock);peers.append(peer)
                peer.send(type='app.launch',app_id=f'dpi-{scale}',request_id=f'launch-{scale}',
                          backend='native',scale_factor=scale)
                started=peer.until('session.started')
                frame=peer.until('frame')
                state=folder/f'{scale}.json';wait(state.exists)
                data=json.loads(state.read_text())
                assert data['scale']==scale, data
                assert (frame['width'],frame['height'])==(data['width']*scale,data['height']*scale), (frame,data)
                assert struct.unpack('>II',base64.b64decode(frame['png'])[16:24])==(frame['width'],frame['height'])
                peer.send(type='window.configure',window_id=frame['window_id'],width=1470*scale,height=802*scale)
                while True:
                    frame=peer.until('frame')
                    if (frame['width'],frame['height'])==(1470*scale,802*scale):break
                wait(lambda: json.loads(state.read_text())['width']==1470)
                data=json.loads(state.read_text())
                assert data['height']==802 and data['text']=='preserved original contents',data
                results.append(data)
                peer.close();peers.remove(peer)
                resumed=Peer(sock);peers.append(resumed)
                resumed.send(type='session.resume',session_id=started['session_id'])
                resumed.until('session.snapshot');resumed.until('frame')
                assert json.loads(state.read_text())['pid']==data['pid']
                print('PASS: DPI',scale,'actual pixels',frame['width'],frame['height'],'client',data,flush=True)
                if scale==2:density_changes(resumed,state,frame['window_id'],data)
            assert results[0]['button_height']==results[1]['button_height'], results
            assert results[0]['text_pixels']==results[1]['text_pixels'], results
            assert json.loads((folder/'1.json').read_text())['scale']==1
            print('PASS: equal logical text/control sizes, 2x source pixels, separate display densities, retained PID/content')
            if a.electron:
                for scale in (1,2):
                    peer=Peer(sock);peers.append(peer)
                    peer.send(type='app.launch',app_id=f'electron-{scale}',request_id=f'electron-{scale}',backend='native',scale_factor=scale)
                    peer.until('session.started');frame=peer.until('frame')
                    state=folder/f'electron-{scale}.json';wait(state.exists)
                    data=json.loads(state.read_text())
                    assert data['scale']==scale, data
                    peer.send(type='window.viewport',window_id=frame['window_id'],logical_width=1470,logical_height=802,scale_factor=scale)
                    while True:
                        frame=peer.until('frame')
                        if (frame['width'],frame['height'])==(1470*scale,802*scale):break
                    wait(lambda:json.loads(state.read_text())['width']==1470)
                    data=json.loads(state.read_text())
                    assert data['height']==802 and data['button_height']==40 and data['text']=='preserved original contents',data
                    for pressed in (True,False):
                        peer.send(type='input.pointer',window_id=frame['window_id'],x=int(data['button_x']*scale),y=int(data['button_y']*scale),button=1,pressed=pressed)
                    wait(lambda:json.loads(state.read_text())['clicks']==1)
                    print('PASS: Electron viewport/DPR/control size/source pixels/pointer',scale,data,flush=True)
                    if scale==2:density_changes(peer,state,frame['window_id'],data)
        except Exception:
            print((folder/'agent.log').read_text(errors='replace')[-6000:])
            raise
        finally:
            for peer in peers:peer.close()
            agent.terminate();agent.wait(timeout=15)
