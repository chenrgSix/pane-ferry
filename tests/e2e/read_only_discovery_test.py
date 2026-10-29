"""Metadata queries must not revive an idle, detached capture output.

Runs only within wayland_smoke's disposable GNOME/PipeWire desktop. This does
not issue any requests against the user's graphical session or applications.
"""
import time


def verify(peer, socket, env, target, app, wait, Peer, Bridge, monitor_layout):
    bridge = Bridge(env)
    observer = resumed = None
    try:
        def source():
            return next(w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid)

        def geometry():
            value = source()
            return tuple(value[k] for k in ('x', 'y', 'width', 'height', 'scale', 'minimized'))

        before_layout = monitor_layout()
        before_geometry = geometry()
        started = peer.request('app.launch', 'session.started', app_id=target['id'],
                               request_id='readonly-discovery', backend='native', scale_factor=1, attach=False)
        peer.request('session.resume', 'session.snapshot', session_id=started['session_id'])
        peer.until('frame')
        peer.send(type='session.detach')
        peer.close()
        wait(lambda: monitor_layout() == before_layout, seconds=15)
        wait(lambda: geometry() == before_geometry, seconds=10)
        # The lease's private output has expired; querying it used to call
        # scope -> prepareOutput and move this window onto a new monitor.
        observer = Peer(socket)
        for _ in range(8):
            sessions = observer.request('session.list')['sessions']
            session = next(s for s in sessions if s['session_id'] == started['session_id'])
            assert session['controlled'] is False and session['window_count'] == 1, session
            observer.request('window.list')
            observer.request('app.list')
            observer.request('desktop.list')
            assert monitor_layout() == before_layout, 'metadata query created a private capture output'
            assert geometry() == before_geometry, ('metadata query changed source geometry', before_geometry, geometry())
            assert app.poll() is None
            time.sleep(.12)
        resumed = Peer(socket)
        snapshot = resumed.request('session.resume', 'session.snapshot', session_id=started['session_id'])
        assert snapshot['session_id'] == started['session_id']
        resumed.until('frame')
        resumed.send(type='session.detach')
        print('PASS: 8 rounds of app/window/session/monitor queries preserved detached native geometry, scale and monitor layout; counts and explicit reconnect still work', flush=True)
    finally:
        if observer:
            observer.close()
        if resumed:
            resumed.close()
        bridge.close()
