"""Run only inside wayland_smoke's private D-Bus and GNOME compositor."""
import json
import subprocess
import time


def verify(sock, env, fixture, scratch, log, start, wait, Peer, Bridge):
    from gi.repository import Gio, GLib
    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)

    def display_call(method, args=None):
        return bus.call_sync('org.gnome.Mutter.DisplayConfig', '/org/gnome/Mutter/DisplayConfig',
                             'org.gnome.Mutter.DisplayConfig', method, args, None, 0, 5000, None).unpack()

    record = scratch/'resize-x11.json'
    app = start(['/usr/bin/python3', str(fixture), str(record), '--x11', '--minimum-size'],
                dict(env, GDK_BACKEND='x11', GDK_SCALE='1'), log)
    wait(record.exists)
    bridge, remote = Bridge(env), Peer(sock)
    try:
        info = wait(lambda: next((w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid), None))
        xid = info['xwindow']
        hints = lambda: subprocess.check_output(['xprop', '-id', str(xid), 'WM_NORMAL_HINTS'], env=env)
        original_hints = hints()
        assert b'minimum size: 480 by 600' in original_hints
        original_text = json.loads(record.read_text())['text']

        # Change only the private compositor after this X11 window already
        # exists. Mutter 50 otherwise caches the former stage-space minimum.
        state = display_call('GetCurrentState')
        monitor = state[1][0]
        mode = next(m for m in monitor[1] if m[-1].get('is-current'))
        assert 1.25 in mode[5]
        display_call('ApplyMonitorsConfig', GLib.Variant('(uua(iiduba(ssa{sv}))a{sv})', (
            state[0], 1, [(0, 0, 1.25, 0, True, [(monitor[0][0], mode[0], {})])],
            {'layout-mode': GLib.Variant('u', 1)})))
        wait(lambda: next(w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid)['scale'] == 2)
        target = next(w for w in remote.request('window.list')['windows'] if w['window_id'] == xid)
        started = remote.request('app.launch', 'session.started', app_id=target['id'],
                                 request_id='scaled-x11', backend='native', scale_factor=1, attach=False)
        remote.request('session.resume', 'session.snapshot', session_id=started['session_id'])
        remote.until('frame')

        def configure(width, height, expected):
            remote.send(type='window.configure', window_id=xid, width=width, height=height)
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                frame = remote.until('frame')
                if (frame['width'], frame['height']) == expected:
                    break
            assert (frame['width'], frame['height']) == expected, (width, height, frame['width'], frame['height'])
            assert frame['codec'] == 'h264'
            wait(lambda: (json.loads(record.read_text())['width'], json.loads(record.read_text())['height']) == expected)
            assert hints() == original_hints, 'application constraints were modified'
            assert app.poll() is None and json.loads(record.read_text())['text'] == original_text
            print(json.dumps({'requested': [width, height], 'received': list(expected),
                              'constraints_unchanged': True, 'pid_unchanged': True}), flush=True)

        configure(1470, 805, (1470, 806))
        configure(900, 700, (900, 700))
        configure(900, 500, (900, 600))  # Genuine minimums remain enforced.
        bridge.call(op='test_resize_maximize', pid=app.pid)
        wait(lambda: json.loads(record.read_text())['width'] > 2000)
        configure(1470, 805, (1470, 806))
        configure(1000, 800, (1000, 800))
        print('PASS: live 125% scaling, unchanged X11 hints, one-request restore from maximized, real H.264 dimensions', flush=True)
    finally:
        remote.close()
        bridge.close()
