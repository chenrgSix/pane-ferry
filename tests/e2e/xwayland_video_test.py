"""Own fixture only, invoked inside wayland_smoke's private compositor."""
import base64
import json
import subprocess
import time


def verify(sock,env,fixture,scratch,log,start,wait,Peer,Bridge,root):
    state=scratch/'video-x11.json'
    app=start(['/usr/bin/python3',str(fixture),str(state),'--x11'],dict(env,GDK_BACKEND='x11',GDK_SCALE='1'),log)
    wait(state.exists)
    bridge,remote=Bridge(env),Peer(sock)
    try:
        target=wait(lambda:next((w for w in bridge.call(op='list')['result'] if w['pid']==app.pid),None))
        xid=target['xwindow']
        row=next(w for w in remote.request('window.list')['windows'] if w['window_id']==xid)
        started=remote.request('app.launch','session.started',app_id=row['id'],request_id='gpu-video',backend='native',attach=False)
        remote.request('session.resume','session.snapshot',session_id=started['session_id'])
        remote.until('frame')
        for width,height in [(2800,1500),(1470,809),(641,483),(2940,1618),(900,701)]:
            remote.send(type='window.configure',window_id=xid,width=width,height=height)
            deadline=time.monotonic()+15
            while True:
                frame=remote.until('frame')
                if (frame['width'],frame['height'])==(width,height):break
                assert time.monotonic()<deadline
            assert frame['codec']=='h264' and frame['key_frame']
            assert (frame['encoded_width'],frame['encoded_height'])==(width+width%2,height+height%2)
            # A single IDR must be independently decodable. OpenH264's Gst
            # wrapper may retain it until a second picture even after EOS, so
            # use FFmpeg here; the continuous test also checks OpenH264.
            decoded=subprocess.run(['ffmpeg','-v','error','-f','h264','-i','pipe:0','-frames:v','1',
                                    '-f','rawvideo','-pix_fmt','rgb24','pipe:1'],
                                   input=base64.b64decode(frame['data']),capture_output=True)
            if decoded.returncode:
                raise AssertionError((width,height,decoded.stderr.decode(errors='replace')))
            assert len(decoded.stdout)==(width+width%2)*(height+height%2)*3
            print('PASS: decoded Xwayland H264 resize',width,height,flush=True)
        remote.close()
        remote=Peer(sock)
        snapshot=remote.request('session.resume','session.snapshot',session_id=started['session_id'])
        assert snapshot['windows']
        frame=remote.until('frame');assert frame['key_frame']
        assert app.poll() is None and json.loads(state.read_text())['text']=='preserved fixture contents'
        print('PASS: Xwayland reconnect retains original app and starts IDR',flush=True)
        remote.close()
        # More windows than the private target pool: plugins retaining one
        # input per idle encoder must not starve subsequent windows forever.
        six=scratch/'six-video-windows.json'
        code='''import gi,json,sys
gi.require_version('Gtk','3.0');gi.require_version('GdkX11','3.0')
from gi.repository import Gtk,Gdk,GdkX11
windows=[]
for i in range(6):
 w=Gtk.Window(title='GPU lease fixture '+str(i));w.set_default_size(480,320)
 if i:w.set_transient_for(windows[0]);w.set_type_hint(Gdk.WindowTypeHint.DIALOG)
 w.add(Gtk.Label(label='Static private GPU window '+str(i)));w.show_all();windows.append(w)
open(sys.argv[1],'w').write(json.dumps([w.get_window().get_xid() for w in windows]))
Gtk.main()
'''
        family=start(['/usr/bin/python3','-c',code,str(six)],dict(env,GDK_BACKEND='x11',GDK_SCALE='1'),log)
        wait(six.exists);ids=json.loads(six.read_text())
        remote=Peer(sock)
        row=wait(lambda:next((w for w in remote.request('window.list')['windows'] if w['window_id']==ids[0]),None))
        started=remote.request('app.launch','session.started',app_id=row['id'],request_id='six-gpu-windows',backend='native',attach=False)
        snapshot=remote.request('session.resume','session.snapshot',session_id=started['session_id'])
        assert len(snapshot['windows'])==6
        seen=set();deadline=time.monotonic()+20
        while len(seen)<6:
            frame=remote.until('frame');seen.add(frame['window_id'])
            assert time.monotonic()<deadline,'GPU input leases starved the target pool'
        assert seen==set(ids) and family.poll() is None
        print('PASS: six static Xwayland windows publish through a four-target GPU lease pool',flush=True)
    finally:
        remote.close();bridge.close()
