"""Run only inside wayland_smoke's private D-Bus and GNOME compositor."""
import json
import base64
import re
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
            native = tuple(n * 2 for n in expected)
            wait(lambda: (json.loads(record.read_text())['width'], json.loads(record.read_text())['height']) == native)
            assert hints() == original_hints, 'application constraints were modified'
            assert app.poll() is None and json.loads(record.read_text())['text'] == original_text
            print(json.dumps({'requested': [width, height], 'received': list(expected), 'native': native,
                              'constraints_unchanged': True, 'pid_unchanged': True}), flush=True)

        configure(1470, 805, (1470, 805))
        configure(900, 700, (900, 700))
        configure(900, 200, (900, 300))  # 600 native pixels at 2x remain enforced.
        bridge.call(op='test_resize_maximize', pid=app.pid)
        wait(lambda: json.loads(record.read_text())['width'] > 2000)
        configure(1470, 805, (1470, 805))
        configure(1000, 800, (1000, 800))
        print('PASS: live 125% scaling, unchanged X11 hints, one-request restore from maximized, logical H.264 dimensions', flush=True)
        remote.close()
        verify_controls(sock, env, fixture, scratch, log, start, wait, Peer, bridge, display_call)
    finally:
        remote.close()
        bridge.close()


def verify_controls(sock, env, fixture, scratch, log, start, wait, Peer, bridge, display_call):
    record = scratch/'density-controls.json'
    app = start(['/usr/bin/python3', str(fixture.with_name('xwayland_density_fixture.py')), str(record)],
                dict(env, GDK_BACKEND='x11', GDK_SCALE='2'), log)
    wait(record.exists)
    remote = Peer(sock)
    try:
        info = wait(lambda: next((w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid), None))
        xid = info['xwindow']
        target = next(w for w in remote.request('window.list')['windows'] if w['window_id'] == xid)
        started = remote.request('app.launch', 'session.started', app_id=target['id'],
                                 request_id='density-controls', backend='native', scale_factor=1, attach=False)
        remote.request('session.resume', 'session.snapshot', session_id=started['session_id'])
        remote.send(type='window.configure', window_id=xid, width=900, height=701)
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            frame = remote.until('frame')
            if (frame['width'], frame['height']) == (900, 701): break
        assert (frame['width'], frame['height']) == (900, 701)
        wait(lambda: (json.loads(record.read_text())['width'], json.loads(record.read_text())['height']) == (900, 701))
        measured = json.loads(record.read_text())
        assert measured['scale'] == 2
        assert frame['key_frame']
        decoded = subprocess.run(['ffmpeg', '-v', 'error', '-f', 'h264', '-i', 'pipe:0', '-frames:v', '1',
                                  '-f', 'rawvideo', '-pix_fmt', 'rgb24', 'pipe:1'],
                                 input=base64.b64decode(frame['data']), capture_output=True, check=True).stdout
        assert len(decoded) == frame['encoded_width'] * frame['encoded_height'] * 3
        red = []
        for y in range(130):
            for x in range(220):
                offset = (y * frame['encoded_width'] + x) * 3
                r, g, b = decoded[offset:offset+3]
                if r > 170 and g < 80 and b < 80: red.append((x, y))
        assert red
        bounds = [min(x for x,y in red), min(y for x,y in red), max(x for x,y in red)+1, max(y for x,y in red)+1]
        assert all(abs(a-b) <= 2 for a,b in zip(bounds, [40, 40, 120, 70])), bounds
        assert measured['button_width'] == 100 and measured['button_height'] == 36
        print(json.dumps({'received': [900, 701], 'native': [1800, 1402], 'source_layout': [measured['width'], measured['height']],
                          'source_density': 2, 'decoded_marker_bounds': bounds,
                          'normal_control_size_verified': True}), flush=True)
        print('PASS: 2x source retains full logical layout and control sizes while H.264 remains 1x; odd-height padding is preserved', flush=True)
        command = record.with_suffix('.command')
        command.write_text(json.dumps({'serial': 1, 'action': 'open'}))
        metadata, frames = {}, set()
        deadline = time.monotonic() + 15
        while time.monotonic() < deadline:
            event = json.loads(remote.file.readline())
            assert event['type'] not in ('error', 'session.exited'), event
            wid = event.get('window_id')
            if event['type'] in ('window.created', 'window.metadata'):
                metadata[wid] = event
            if event['type'] == 'frame' and wid != xid:
                frames.add(wid)
            if len(frames) == 3: break
        assert len(frames) == 3, frames
        current = {w['xwindow']: w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid}
        assert sorted(metadata[wid]['role'] for wid in frames) == ['dialog', 'popup', 'popup']
        for wid in frames:
            event, source = metadata[wid], current[wid]
            assert (event['width'], event['height']) == (source['client_width'], source['client_height']), (event, source)
            assert (event['presentation']['x'], event['presentation']['y']) == (source['client_x'], source['client_y'])
        assert any(metadata[wid]['parent_id'] not in (None, xid) for wid in frames)
        print('PASS: 2x Xwayland dialogs and nested popup streams keep logical size, placement and parent relationships', flush=True)
        command.write_text(json.dumps({'serial': 2, 'action': 'close'}))
        wait(lambda: len([w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid]) == 1)
        from gi.repository import GLib
        for scale, resource_scale in [(1.0, 1), (1.25, 2)]:
            state = display_call('GetCurrentState')
            monitor = state[1][0]
            mode = next(m for m in monitor[1] if m[-1].get('is-current'))
            display_call('ApplyMonitorsConfig', GLib.Variant('(uua(iiduba(ssa{sv}))a{sv})', (
                state[0], 1, [(0, 0, scale, 0, True, [(monitor[0][0], mode[0], {})])],
                {'layout-mode': GLib.Variant('u', 1)})))
            source = wait(lambda: next((w for w in bridge.call(op='list')['result']
                          if w['pid'] == app.pid and w['scale'] == resource_scale), None))
            # Receive the post-scale geometry before configuring, as the host
            # does. No reconnect or application restart is allowed here.
            def native_size():
                info = subprocess.check_output(['xwininfo', '-id', str(xid)], env=env, text=True)
                return tuple(int(re.search(r'\b'+k+r':\s*(\d+)', info)[1]) for k in ('Width', 'Height'))
            raw = native_size()
            density = next(n for n in range(1, 5) if abs(raw[0] - source['client_width'] * n) <= 1
                           and abs(raw[1] - source['client_height'] * n) <= 1)
            expected = tuple((value + density - 1) // density for value in raw)
            print(json.dumps({'display_scale': scale, 'post_change_layout': [source['client_width'], source['client_height']],
                              'native': raw, 'density': density, 'post_change_wire': expected}), flush=True)
            remote.request('session.list')  # Drain pre-change queued frames.
            remote.send(type='video.keyframe', window_id=xid)
            while True:
                frame = remote.until('frame')
                if frame['window_id'] == xid and frame['key_frame'] and (frame['width'], frame['height']) == expected: break
            # Mutter can retain the former client rectangle until the first
            # resize. Exercise the same initial request plus two bounded
            # reconciliation attempts used by the Mac receiver.
            for attempt in range(3):
                previous_stream = frame['stream_id']
                remote.send(type='window.configure', window_id=xid, width=900, height=700)
                while True:
                    frame = remote.until('frame')
                    if frame['window_id'] == xid and frame['stream_id'] != previous_stream: break
                print(json.dumps({'display_scale': scale, 'attempt': attempt + 1,
                                  'configured_frame': [frame['width'], frame['height']]}), flush=True)
                if (frame['width'], frame['height']) == (900, 700): break
                time.sleep(.35)
            assert (frame['width'], frame['height']) == (900, 700)
            source = next(w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid)
            assert (source['client_width'], source['client_height']) == (900, 700)
        print('PASS: attached Xwayland session adapts 125% -> 100% -> 125% with bounded host reconciliation, no application restart', flush=True)
    finally:
        remote.close()
