#!/usr/bin/env python3
"""Opt-in: private D-Bus, headless GNOME, Xvfb keymap and native GTK fixture.
Never replaces the logged-in compositor, opens Firefox, or changes desktop settings.
"""
import argparse
from contextlib import ExitStack
import base64
import json
import os
from pathlib import Path
import secrets
import re
import shutil
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import uuid
from concurrent.futures import ThreadPoolExecutor

p = argparse.ArgumentParser(description=__doc__)
p.add_argument('--agent', required=True, type=Path)
p.add_argument('--private-bus', action='store_true')
p.add_argument('--with-xwayland', action='store_true')
p.add_argument('--logical-size-only', action='store_true', help='test retained-DPI logical window sizing only')
p.add_argument('--logical-wayland-scale', type=int, choices=(1, 2), default=1,
               help='source scale to request for the logical-size fixture')
p.add_argument('--window-lifecycle-only', action='store_true', help='test close/acquire/release in the private compositor only')
p.add_argument('--extension-dir', type=Path, help='copy a compatible installed bridge into the isolated compositor')
a = p.parse_args()
if not a.private_bus and os.environ.get('XDG_SESSION_TYPE') == 'tty':
    p.error('Run from the graphical login or via systemd-run --user --collect --wait --pipe; a headless GNOME launched in an SSH login may not render.')
if not a.private_bus:
    with tempfile.TemporaryDirectory(prefix='paneferry-wayland-bus-') as folder:
        conf=Path(folder)/'session.conf'
        conf.write_text('''<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>
<policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy></busconfig>''')
        # No activation of the user's calendar, keyring, document portal or IM.
        raise SystemExit(subprocess.call(['dbus-run-session', '--config-file='+str(conf), '--', sys.executable, __file__, '--agent', str(a.agent.resolve()), '--private-bus',
            *(['--with-xwayland'] if a.with_xwayland else []), *(['--logical-size-only'] if a.logical_size_only else []),
            '--logical-wayland-scale', str(a.logical_wayland_scale),
            *(['--window-lifecycle-only'] if a.window_lifecycle_only else []),
            *(['--extension-dir', str(a.extension_dir.resolve())] if a.extension_dir else [])]))
root = Path(__file__).resolve().parents[2]
children = []

def stop_children():
    for child in reversed(children):
        if child.poll() is None:
            os.killpg(child.pid,signal.SIGTERM)
            try:child.wait(timeout=5)
            except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait(timeout=5)

def start(cmd, env, log):
    child = subprocess.Popen(cmd, env=env, stdout=log, stderr=log, stdin=subprocess.DEVNULL, start_new_session=True)
    children.append(child)
    return child

def wait(fn, seconds=20):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        result = fn()
        if result: return result
        time.sleep(.1)
    raise AssertionError('fixture startup timed out')

def monitor_layout():
    import gi
    gi.require_version('Gio', '2.0')
    from gi.repository import Gio
    state = Gio.bus_get_sync(Gio.BusType.SESSION, None).call_sync('org.gnome.Mutter.DisplayConfig',
        '/org/gnome/Mutter/DisplayConfig', 'org.gnome.Mutter.DisplayConfig', 'GetCurrentState',
        None, None, Gio.DBusCallFlags.NO_AUTO_START, 3000, None).unpack()
    return {spec[0]: tuple(logical[:5]) for logical in state[2] for spec in logical[5]}

class Peer:
    def __init__(self, path):
        self.sock = socket.socket(socket.AF_UNIX); self.sock.settimeout(20); self.sock.connect(str(path)); self.file = self.sock.makefile('rwb')
        self.send(type='hello'); self.until('hello')
    def send(self, **kw):
        self.file.write(json.dumps(dict(kw, version=1)).encode()+b'\n');self.file.flush()
    def until(self, kind):
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            raw=self.file.readline(16*1024*1024+1)
            assert raw.endswith(b'\n')
            m=json.loads(raw)
            assert m['type']!='error' or kind=='error',m
            if m['type']==kind:return m
        raise AssertionError('missing '+kind)
    def request(self, kind, result=None, **kw):
        self.send(type=kind, **kw);return self.until(result or kind)
    def close(self):self.file.close();self.sock.close()

class Bridge:
    def __init__(self, env):
        self.child=subprocess.Popen(['/usr/bin/python3',str(root/'apps/linux-agent/src/wayland_helper.py')],
            env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True)
    def call(self, **request):
        self.child.stdin.write(json.dumps(request,ensure_ascii=False)+'\n');self.child.stdin.flush()
        return json.loads(self.child.stdout.readline())
    def close(self):
        self.child.stdin.close();self.child.wait(timeout=8);self.child.stdout.close()

try:
    with tempfile.TemporaryDirectory(prefix='paneferry-wayland-') as tmp, ExitStack() as lifetime:
        # Stop fixture writers before TemporaryDirectory removes their files.
        lifetime.callback(stop_children)
        t=Path(tmp);home=t/'home';home.mkdir();data=t/'data';config=t/'config';cache=t/'cache'
        for d in [data,config,cache]:d.mkdir()
        runtime=t/'run';runtime.mkdir(mode=0o700)
        # Keep compositor sockets/startup sentinels private; only its own
        # screen-cast nodes use the logged-in user's PipeWire service.
        (runtime/'pipewire-0').symlink_to(Path(os.environ['XDG_RUNTIME_DIR'])/'pipewire-0')
        ext=data/'gnome-shell/extensions/paneferry@local';ext.parent.mkdir(parents=True)
        shutil.copytree(a.extension_dir or root/'apps/linux-agent/gnome-extension',ext)
        (data/'gnome-shell/update-check-50').touch()
        applications=data/'applications';applications.mkdir()
        state=t/'state.json';fixture=root/'tests/e2e/wayland_fixture.py'
        (applications/'io.paneferry.WaylandFixture.desktop').write_text(f'[Desktop Entry]\nType=Application\nName=Wayland fixture\nExec=/usr/bin/python3 {fixture} {state}\nStartupWMClass=io.paneferry.WaylandFixture\n')
        authority=t/'Xauthority';authority.touch(mode=0o600)
        display=next(n for n in range(3100,3200) if not Path(f'/tmp/.X11-unix/X{n}').exists())
        subprocess.run(['xauth','-f',str(authority),'add',f':{display}','.',secrets.token_hex(16)],check=True)
        wayland='paneferry-test-'+uuid.uuid4().hex
        env=dict(os.environ,HOME=str(home),XDG_DATA_HOME=str(data),XDG_CONFIG_HOME=str(config),XDG_CACHE_HOME=str(cache),
            XDG_RUNTIME_DIR=str(runtime),
            LC_ALL='C',GSETTINGS_BACKEND='keyfile',XDG_CURRENT_DESKTOP='GNOME',XDG_SESSION_TYPE='wayland',GNOME_SHELL_SESSION_MODE='user',
            DISPLAY=f':{display}',XAUTHORITY=str(authority),WAYLAND_DISPLAY=wayland,
            GTK_USE_PORTAL='0', GIO_USE_VFS='local', NO_AT_BRIDGE='1',
            GTK_IM_MODULE='simple', QT_IM_MODULE='compose', XMODIFIERS='@im=none')
        subprocess.run(['dbus-update-activation-environment', *[f'{k}={v}' for k,v in env.items() if k in
            ('HOME','XDG_DATA_HOME','XDG_CONFIG_HOME','XDG_CACHE_HOME','XDG_RUNTIME_DIR','GSETTINGS_BACKEND','XDG_CURRENT_DESKTOP',
             'XDG_SESSION_TYPE','DISPLAY','XAUTHORITY','WAYLAND_DISPLAY','GTK_USE_PORTAL','GIO_USE_VFS','NO_AT_BRIDGE')]], env=env, check=True)
        subprocess.run(['gsettings','set','org.gnome.shell','enabled-extensions',"['paneferry@local']"],env=env,check=True)
        subprocess.run(['gsettings','set','org.gnome.shell','disable-user-extensions','false'],env=env,check=True)
        with (t/'fixture.log').open('w') as log:
            start(['Xvfb',f':{display}','-screen','0','1600x900x24','-nolisten','tcp','-auth',str(authority)],env,log)
            wait(lambda:Path(f'/tmp/.X11-unix/X{display}').exists())
            shell=start(['gnome-shell','--headless','--wayland',*(['--no-x11'] if not a.with_xwayland else []),'--wayland-display',wayland,'--virtual-monitor','1600x900'],env,log)
            wait(lambda:(runtime/wayland).exists())
            app=start(['/usr/bin/python3',str(fixture),str(state),'--delayed-paste'],dict(env,GDK_BACKEND='wayland'),log)
            try:
                wait(state.exists, seconds=60)
            except Exception:
                print((t/'fixture.log').read_text(errors='replace')[-10000:], file=sys.stderr)
                raise
            if a.with_xwayland:
                display_info=wait(lambda:re.search(r'Using public X11 display (:\d+)',(t/'fixture.log').read_text()))
                private_authority=wait(lambda:next(iter(runtime.glob('.mutter-Xwaylandauth.*')),None))
                env.update(DISPLAY=display_info[1],XAUTHORITY=str(private_authority))
            publisher=start([str(a.agent.resolve()),'serve'],env,log)
            sock=home/'.local/state/lan-remoteapp/agent.sock';wait(sock.exists)
            try:
                peer=Peer(sock)
            except Exception:
                print((t/'fixture.log').read_text(errors='replace')[-10000:],file=sys.stderr)
                raise
            try:
                if a.window_lifecycle_only:
                    from window_lifecycle_test import verify
                    verify(env, app, state, fixture, t, log, start, wait, Bridge, monitor_layout, shell)
                    raise SystemExit(0)
                if a.logical_size_only:
                    from logical_window_size_test import verify
                    verify(peer, sock, env, state, app, fixture, t, log, start, wait, Peer, Bridge,
                           monitor_layout, a.with_xwayland, a.logical_wayland_scale)
                    raise SystemExit(0)
                detail=subprocess.run(['gdbus','call','--timeout','3','--session','--dest','org.gnome.Shell','--object-path','/org/gnome/Shell',
                    '--method','org.gnome.Shell.Extensions.GetExtensionInfo','paneferry@local'],env=env,capture_output=True,text=True)
                print('Extension:',detail.stdout,detail.stderr,flush=True)
                detail=subprocess.run(['/usr/bin/python3',str(root/'apps/linux-agent/src/wayland_helper.py'),'list'],env=env,capture_output=True,text=True)
                print('Compositor:',detail.stdout,detail.stderr,flush=True)
                reply=peer.request('app.list');assert not reply.get('discovery_error'),reply
                catalog=reply['apps']
                app_id=next(app['id'] for app in catalog if app['name']=='Wayland fixture')
                windows=wait(lambda:peer.request('window.list')['windows'])
                target=next(w for w in windows if w['name']=='Wayland fixture')
                assert target['id'].startswith('wayland-') and target['app_id']==app_id,target
                one,two=Bridge(env),Bridge(env)
                try:
                    info=next(w for w in one.call(op='list')['result'] if w['pid']==app.pid)
                    baseline_monitors=monitor_layout()
                    original_geometry=tuple(info[k] for k in ('x','y','width','height','scale'))
                    assert one.call(op='acquire',id=info['id'],pid=app.pid+1)['error']=='window_unavailable'
                    assert 'result' in one.call(op='acquire',id=info['id'],pid=app.pid)
                    assert two.call(op='acquire',id=info['id'],pid=app.pid)['error']=='session_controlled'
                    assert one.call(op='resize',window_id=info['window_id']+1000,width=800,height=500)['error']=='window_unavailable'
                    assert one.call(op='set_text',window_id=info['window_id'],text='界'*3000)['error']=='invalid_text'
                    assert 'result' in one.call(op='set_text',window_id=info['window_id'],text='\n'*8192)
                finally:
                    one.close();two.close()
                wait(lambda: monitor_layout()==baseline_monitors)
                wait(lambda:json.loads(state.read_text())['scale']==1)
                released=Bridge(env)
                try:
                    restored=next(w for w in released.call(op='list')['result'] if w['pid']==app.pid)
                    assert tuple(restored[k] for k in ('x','y','width','height','scale'))==original_geometry,restored
                finally:released.close()
                other_state=t/'other-state.json'
                other_app=start(['/usr/bin/python3',str(fixture),str(other_state),'--other'],dict(env,GDK_BACKEND='wayland'),log)
                wait(other_state.exists)
                one,two=Bridge(env),Bridge(env)
                try:
                    inventory=one.call(op='list')['result']
                    originals=[next(w for w in inventory if w['pid']==pid) for pid in (app.pid,other_app.pid)]
                    with ThreadPoolExecutor(max_workers=2) as pool:
                        replies=list(pool.map(lambda pair: pair[0].call(op='acquire',id=pair[1]['id'],pid=pair[1]['pid']),zip((one,two),originals)))
                    assert all('result' in reply for reply in replies),replies
                    layout=monitor_layout()
                    assert all(layout[key]==value for key,value in baseline_monitors.items()),layout
                    assert len(layout)==len(baseline_monitors)+2,layout
                    assert one.call(op='resize',window_id=originals[1]['window_id'],width=800,height=500)['error']=='window_unavailable'
                    assert two.call(op='resize',window_id=originals[0]['window_id'],width=800,height=500)['error']=='window_unavailable'
                finally:one.close();two.close()
                wait(lambda:monitor_layout()==baseline_monitors)
                wait(lambda:json.loads(state.read_text())['scale']==json.loads(other_state.read_text())['scale']==1)
                print('PASS: concurrent output ownership, cross-window denial, physical layout preservation and release cleanup')
                from wayland_text_test import verify
                verify(env,app.pid,state,Bridge,wait,None,delayed_paste=True)
                wait(lambda:monitor_layout()==baseline_monitors)
                im_state=t/'im-state.json'
                im_app=start(['/usr/bin/python3',str(fixture),str(im_state),'--im'],dict(env,GDK_BACKEND='wayland',GTK_IM_MODULE='wayland'),log)
                wait(im_state.exists)
                verify(env,im_app.pid,im_state,Bridge,wait,'text-input')
                wait(lambda:monitor_layout()==baseline_monitors)
                before=state.read_text()
                started=peer.request('app.launch','session.started',app_id=app_id,request_id='take-original',backend='native',attach=False)
                assert app.poll() is None and json.loads(state.read_text())['text']=='preserved fixture contents'
                snap=peer.request('session.resume','session.snapshot',session_id=started['session_id'])
                window=snap['windows'][0]['window_id'];frame=peer.until('frame')
                assert frame['window_id']==window
                print('Initial capture:',frame['width'],frame['height'],flush=True)
                assert struct.unpack('>II',base64.b64decode(frame['png'])[16:24])==(frame['width'],frame['height'])
                sizes=[]
                for width,height in [(1200,700),(2940,1604),(2940,1846),(800,500)]:
                    peer.send(type='window.configure',window_id=window,width=width,height=height)
                    print('Requested:',width,height,flush=True)
                    until=time.monotonic()+15
                    while time.monotonic()<until:
                        frame=peer.until('frame')
                        print('Received:',frame['width'],frame['height'],flush=True)
                        if (frame['width'],frame['height'])==(width,height):break
                    assert (frame['width'],frame['height'])==(width,height),frame.keys()
                    assert struct.unpack('>II',base64.b64decode(frame['png'])[16:24])==(width,height)
                    assert json.loads(state.read_text())['scale']==2, 'client must redraw at output density, not upscale its screenshot'
                    assert json.loads(other_state.read_text())['scale']==1, 'another native application stays on its original output'
                    assert all(monitor_layout()[key]==value for key,value in baseline_monitors.items())
                    sizes.append((frame['width'],frame['height']))
                error=peer.request('window.configure','error',window_id=window,width=1200,height=3000)
                assert error['code']=='resolution_limited',error
                cut=peer.request('clipboard.cut','error',window_id=window,request_id='safe-cut')
                assert cut['code']=='wayland_cut_unavailable',cut
                assert json.loads(state.read_text())['text']=='preserved fixture contents'
                denied=peer.request('input.key','error',window_id=window,keysym=97,pressed=True)
                assert denied['code'] in ('desktop_authorization_pending','desktop_input_unavailable','desktop_authorization_denied'),denied
                for kind in ('input.text.commit','clipboard.paste'):
                    denied=peer.request(kind,'error',window_id=window,text='must not bypass portal')
                    assert denied['code'] in ('desktop_authorization_pending','desktop_input_unavailable','desktop_authorization_denied'),denied
                assert json.loads(state.read_text())['text']=='preserved fixture contents'
                peer.close()
                wait(lambda:monitor_layout()==baseline_monitors)
                wait(lambda:json.loads(state.read_text())['scale']==1)
                peer=Peer(sock)
                video=peer.request('video.enable','video.ready',codec='h264');assert video['codec']=='h264'
                reused=peer.request('app.launch','session.started',app_id=app_id,request_id='reuse-original',backend='native',attach=False)
                assert reused['session_id']==started['session_id']
                snap=peer.request('session.resume','session.snapshot',session_id=started['session_id']);peer.until('frame')
                assert app.poll() is None and json.loads(state.read_text())['text']=='preserved fixture contents'
                peer.send(type='window.configure',window_id=window,width=960,height=540)
                deadline=time.monotonic()+15
                while time.monotonic()<deadline:
                    encoded=peer.until('frame')
                    if (encoded['width'],encoded['height'])==(960,540):break
                assert encoded.get('codec')=='h264' and encoded['key_frame'],encoded.keys()
                assert base64.b64decode(encoded['data']).startswith((b'\0\0\0\1',b'\0\0\1'))
                print('PASS: cropped native PipeWire pixels enter the H.264 encoder at 960x540')
                print('PASS: native Wayland inventory, original application scale=2, Retina PipeWire pixels, resize, oversized request rejection, reconnect and preserved process/content',sizes)
                if a.with_xwayland:
                    # Discover only this private compositor's Xwayland. Never
                    # guess :0 or read the real graphical login's authority.
                    match=wait(lambda:re.search(r'Using public X11 display (:\d+)',(t/'fixture.log').read_text()))
                    xauth=wait(lambda:next(iter(runtime.glob('.mutter-Xwaylandauth.*')),None))
                    xstate=t/'x11-state.json'
                    xapp=start(['/usr/bin/python3',str(fixture),str(xstate),'--x11'],dict(env,DISPLAY=match[1],XAUTHORITY=str(xauth),GDK_BACKEND='x11'),log)
                    wait(xstate.exists)
                    wait(lambda:json.loads(xstate.read_text())['scale']==1)
                    geometry=Bridge(env)
                    try:
                        target=wait(lambda:next((w for w in geometry.call(op='list')['result'] if w['pid']==xapp.pid),None))
                        assert not target['native'] and target['xwindow']>0,target
                        assert 'result' in geometry.call(op='acquire',id=target['id'],pid=xapp.pid)
                        reply=geometry.call(op='resize_x11',xwindow=target['xwindow'],width=2940,height=1660)
                        assert 'result' in reply,reply
                        wait(lambda:(json.loads(xstate.read_text())['width'],json.loads(xstate.read_text())['height'])==(2940,1660))
                        point=geometry.call(op='locate_x11',xwindow=target['xwindow'],x=2890,y=1600,width=2940,height=1660)
                        assert 'result' in point,point
                        xy=point['result']
                        assert 0<=xy['x']<1600 and 0<=xy['y']<900,point
                        actual=next(w for w in geometry.call(op='scope',window_id=target['window_id'])['result'] if w['pid']==xapp.pid)
                        assert abs(actual['client_x']+2890*actual['client_width']/2940-xy['x'])<=1,(actual,xy)
                        assert abs(actual['client_y']+1600*actual['client_height']/1660-xy['y'])<=1,(actual,xy)
                        assert json.loads(xstate.read_text())['text']=='preserved fixture contents'
                        print('PASS: Xwayland 2940x1660 client resize beyond 1600x900 monitor; scoped pointer pans to reachable exact content coordinates',xy)
                    finally:geometry.close()
                    other=Peer(sock)
                    try:
                        xwindow=next(w for w in other.request('window.list')['windows'] if w['window_id']==target['xwindow'])
                        xsession=other.request('app.launch','session.started',app_id=xwindow['id'],request_id='x11-original',backend='native',attach=False)
                        other.request('session.resume','session.snapshot',session_id=xsession['session_id'])
                        pixels=other.until('frame')
                        assert (pixels['width'],pixels['height'])==(2940,1660),pixels.keys()
                        assert struct.unpack('>II',base64.b64decode(pixels['png'])[16:24])==(2940,1660)
                        other.send(type='window.configure',window_id=target['xwindow'],width=2800,height=1500)
                        deadline=time.monotonic()+15
                        while time.monotonic()<deadline:
                            pixels=other.until('frame')
                            if (pixels['width'],pixels['height'])==(2800,1500):break
                        assert (pixels['width'],pixels['height'])==(2800,1500),pixels.keys()
                        assert struct.unpack('>II',base64.b64decode(pixels['png'])[16:24])==(2800,1500)
                        print('PASS: publisher Xwayland takeover and compositor resize produce actual 2940x1660 and 2800x1500 XComposite PNG frames')
                    finally:other.close()
                print('NOT TESTED: real Firefox, real desktop input authorization, binary clipboard or multi-monitor scale changes')
            except Exception:
                print((t/'fixture.log').read_text(errors='replace')[-10000:],file=sys.stderr)
                raise
            finally:peer.close()
except Exception:
    raise
finally:
    stop_children()
