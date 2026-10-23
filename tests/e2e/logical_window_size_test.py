"""Contract check used inside wayland_smoke's private compositor only."""
import base64
import json
import struct
import time


def verify(peer, sock, env, state, app, fixture, scratch, log, start, wait, Peer, Bridge, monitor_layout, with_xwayland):
    bridge = Bridge(env)
    original_layout = monitor_layout()
    original_text = json.loads(state.read_text())['text']
    try:
        targets = [('Wayland', app, state)]
        if with_xwayland:
            xstate = scratch/'logical-x11.json'
            xapp = start(['/usr/bin/python3', str(fixture), str(xstate), '--x11'], dict(env, GDK_BACKEND='x11'), log)
            wait(xstate.exists)
            targets.append(('Xwayland', xapp, xstate))
        for kind, process, record in targets:
            remote = Peer(sock)
            try:
                info = wait(lambda: next((w for w in bridge.call(op='list')['result'] if w['pid'] == process.pid), None))
                identity = info['window_id'] if info['native'] else info['xwindow']
                target = next(w for w in remote.request('window.list')['windows'] if w['window_id'] == identity)
                started = remote.request('app.launch', 'session.started', app_id=target['id'],
                                         request_id='logical-'+kind, backend='native', attach=False, scale_factor=1)
                snap = remote.request('session.resume', 'session.snapshot', session_id=started['session_id'])
                window = next(w for w in snap['windows'] if w['window_id'] == identity)
                scale = window['scale_factor']
                assert scale in (1, 2), window
                remote.until('frame')
                source_scale = json.loads(record.read_text())['scale']
                for width, height in [(1470, 802), (900, 600), (1470, 900), (700, 500)]:
                    # This is the Mac receiver's wire contract: logical size
                    # times existing SOURCE scale, without a viewport/DPI op.
                    pixels = (width * scale, height * scale)
                    remote.send(type='window.configure', window_id=identity, width=pixels[0], height=pixels[1])
                    until = time.monotonic() + 15
                    while time.monotonic() < until:
                        frame = remote.until('frame')
                        if (frame['width'], frame['height']) == pixels:
                            break
                    assert (frame['width'], frame['height']) == pixels, (kind, pixels, frame.keys())
                    assert struct.unpack('>II', base64.b64decode(frame['png'])[16:24]) == pixels
                    actual = next(w for w in bridge.call(op='list')['result'] if w['pid'] == process.pid)
                    if actual['native']:
                        assert (actual['width'], actual['height']) == (width, height), actual
                    else:
                        wait(lambda: (json.loads(record.read_text())['width'], json.loads(record.read_text())['height']) == (width, height))
                    assert json.loads(record.read_text())['scale'] == source_scale
                    assert json.loads(record.read_text())['text'] == original_text and process.poll() is None
                    print(json.dumps(dict(backend=kind, logical=[width, height], received=list(pixels),
                                          source_scale=source_scale, pid=process.pid, preserved=True)), flush=True)
                remote.close()
                remote = Peer(sock)
                resumed = remote.request('app.launch', 'session.started', app_id=target['id'],
                                         request_id='logical-reconnect-'+kind, backend='native', attach=False, scale_factor=1)
                assert resumed['session_id'] == started['session_id']
                remote.request('session.resume', 'session.snapshot', session_id=started['session_id'])
                remote.until('frame')
                assert process.poll() is None and json.loads(record.read_text())['text'] == original_text
                print('PASS:', kind, 'logical resize, actual PNG dimensions, unchanged source scale, same process/content/session on reconnect', flush=True)
            finally:
                remote.close()
        wait(lambda: monitor_layout() == original_layout)
        print('PASS: private compositor output cleanup; user desktop, applications and profiles untouched', flush=True)
        print('NOT TESTED: real user applications or Portal-authorized pointer injection; AppKit pointer mapping is checked separately', flush=True)
    finally:
        bridge.close()
