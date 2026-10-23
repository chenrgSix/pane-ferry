"""Exercise an installed app in the private GNOME, with no user profile."""
import base64
from io import BytesIO
import json
import time

from PIL import Image


def verify(_peer, sock, env, scratch, log, start, wait, Peer, Bridge, monitor_layout,
           app_name, scale, artifact_dir):
    assert app_name == 'text-editor'
    title = 'PaneFerry isolated editor probe'
    document = scratch/(title+'.txt')
    document.write_text('PaneFerry 独立会话清晰度测试\n\n'
                        '中文、English 1234567890：应用应按逻辑窗口宽度重新排版。\n',
                        encoding='utf-8')
    before = monitor_layout()
    process = start(['/usr/bin/gnome-text-editor', '--standalone', '--new-window', str(document)],
                    dict(env, GDK_BACKEND='wayland'), log)
    bridge = Bridge(env)
    remote = None
    try:
        remote = Peer(sock)
        def compositor_window():
            reply = bridge.call(op='list')
            assert 'result' in reply, reply
            if process.poll() is not None:
                raise AssertionError(f'isolated editor exited: {process.returncode}')
            return next((w for w in reply['result'] if title in w.get('title', '') and w['native']), None)
        info = wait(compositor_window, seconds=20)
        target = wait(lambda: next((w for w in remote.request('window.list')['windows']
                                    if title in w.get('title', '')), None), seconds=10)
        identity = target['window_id']
        assert info['window_id'] == identity
        started = remote.request('app.launch', 'session.started', app_id=target['id'],
                                 request_id=f'isolated-editor-{scale}', backend='native',
                                 attach=False, scale_factor=scale)
        snapshot = remote.request('session.resume', 'session.snapshot', session_id=started['session_id'])
        assert any(w['window_id'] == identity for w in snapshot['windows']), snapshot
        remote.until('frame')
        for logical in ((900, 700), (850, 650)):
            pixels = tuple(side*scale for side in logical)
            remote.send(type='window.configure', window_id=identity,
                        width=pixels[0], height=pixels[1])
            deadline = time.monotonic()+20
            while time.monotonic() < deadline:
                try:
                    frame = remote.until('frame')
                except TimeoutError as error:
                    raise AssertionError(f'no fully redrawn frame at {pixels}') from error
                if (frame['width'], frame['height']) != pixels:
                    continue
                png = base64.b64decode(frame['png'])
                with Image.open(BytesIO(png)) as image:
                    assert image.size == pixels
                    corner = image.convert('RGB').getpixel((pixels[0]*9//10,
                                                           pixels[1]*9//10))
                if min(corner) > 200:
                    break
            else:
                raise AssertionError(f'{pixels} reports new size but retains black padding')
            actual = next(w for w in bridge.call(op='list')['result'] if w['window_id'] == identity)
            assert (actual['width'], actual['height'], actual['scale']) == (*logical, scale), actual
            if artifact_dir and logical == (900, 700):
                assert artifact_dir.is_dir(), artifact_dir
                (artifact_dir/f'text-editor-{scale}x.png').write_bytes(png)
            print(json.dumps(dict(app=app_name, logical=list(logical), received=list(pixels),
                                  source_scale=scale, window_pid=actual['pid'], corner=list(corner))),
                  flush=True)
        assert process.poll() is None
        assert len(monitor_layout()) == len(before)+1
    finally:
        if remote:
            remote.close()
        bridge.close()
    wait(lambda: monitor_layout() == before)
    print('PASS: isolated editor reflowed at native density; private output released', flush=True)
