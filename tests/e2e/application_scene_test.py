"""Actual GNOME composition and H.264 pixels, not just synthetic metadata."""
import base64
import json
import time
import gi
gi.require_version('Gst', '1.0')
from gi.repository import Gst


def verify(peer, sock, env, state, app, wait, Peer, Bridge, monitor_layout, scale=1):
    Gst.init(None)
    pipeline = Gst.parse_launch('appsrc name=input format=time is-live=true caps=video/x-h264,stream-format=byte-stream,alignment=au ! h264parse ! avdec_h264 ! videoconvert ! video/x-raw,format=RGB ! appsink name=output sync=false max-buffers=1 drop=true')
    source, sink = pipeline.get_by_name('input'), pipeline.get_by_name('output')
    pipeline.set_state(Gst.State.PLAYING)
    bridge = Bridge(env)
    baseline = monitor_layout()
    command = state.with_suffix('.command')
    serial = 0
    def action(name):
        nonlocal serial
        serial += 1
        temporary = command.with_suffix('.new')
        temporary.write_text(json.dumps({'serial': serial, 'action': name})); temporary.replace(command)
    def inventory():
        return [w for w in bridge.call(op='list')['result'] if w['pid'] == app.pid]
    root = next(w for w in inventory() if w['parent_id'] is None)
    target = next(w for w in peer.request('window.list')['windows'] if w['window_id'] == root['window_id'])
    root_id = root['window_id']; canvas = {'width': 1600 * scale, 'height': 900 * scale}
    metadata = {}
    def collect(predicate):
        deadline = time.monotonic()+20
        while time.monotonic() < deadline:
            raw = peer.file.readline(16*1024*1024+1)
            assert raw.endswith(b'\n')
            event = json.loads(raw); kind = event['type']
            assert kind not in ('error', 'session.exited'), event
            if kind in ('window.created', 'window.metadata'):
                metadata[event['window_id']] = event
                assert set(metadata) == {root_id}, metadata
                assert event['surface_type'] == 'session' and event['parent_id'] is None
            if kind != 'frame': continue
            assert event['window_id'] == root_id and (event['width'], event['height']) == (canvas['width'], canvas['height']), event.keys()
            data = base64.b64decode(event['data'])
            buffer = Gst.Buffer.new_allocate(None, len(data), None); buffer.fill(0, data)
            assert source.emit('push-buffer', buffer) == Gst.FlowReturn.OK
            sample = sink.emit('try-pull-sample', Gst.SECOND)
            if sample is None: continue
            buffer = sample.get_buffer(); ok, mapping = buffer.map(Gst.MapFlags.READ)
            assert ok
            try:
                pixels = bytes(mapping.data)
                colors = {'green': 0, 'blue': 0, 'red': 0, 'light': 0}
                # Sparse samples are sufficient for large, solid fixture areas.
                for i in range(0, len(pixels)-2, 3*17):
                    r, g, b = pixels[i:i+3]
                    if g > 150 and r < 80 and b < 100: colors['green'] += 1
                    if b > 150 and r < 80 and g < 100: colors['blue'] += 1
                    if r > 200 and g < 50 and b < 50: colors['red'] += 1
                    if min(r, g, b) > 180: colors['light'] += 1
            finally: buffer.unmap(mapping)
            assert colors['red'] == 0, ('unrelated window leaked', colors)
            if predicate(colors): return colors
        raise AssertionError(('scene did not change', metadata))
    try:
        started = peer.request('app.launch', 'session.started', app_id=target['id'], request_id='application-scene',
                               backend='native', scale_factor=scale, scene=canvas)
        assert collect(lambda c: c['light'] > 100)['light'] > 100
        point = wait(lambda: bridge.call(op='test_scene_point', x=800*scale, y=450*scale).get('result'))
        assert 'x' in point and 'y' in point, point
        action('open')
        colors = collect(lambda c: c['green'] > 100 and c['blue'] > 30)
        assert len(inventory()) == 4, inventory()
        action('stranger')
        wait(lambda: len(inventory()) == 5)
        assert 'result' in bridge.call(op='test_scene_intruder')
        stranger = next(w for w in inventory() if w['title'] == 'Unrelated red window')
        owned = next(w for w in inventory() if w['window_id'] == root_id)
        # Ensure this really challenges the cover, not a window on another output.
        assert max(stranger['x'], owned['x']) < min(stranger['x']+stranger['width'], owned['x']+owned['width'])
        time.sleep(.1)
        stranger = next(w for w in inventory() if w['title'] == 'Unrelated red window')
        blocked = bridge.call(op='test_scene_point', x=(stranger['x']-owned['x']+stranger['width']//2)*scale,
                              y=(stranger['y']-owned['y']+stranger['height']//2)*scale)
        assert blocked.get('error') == 'window_unavailable', (blocked, stranger, owned)
        peer.send(type='video.keyframe', window_id=root_id)
        collect(lambda c: c['green'] > 100 and c['blue'] > 30)
        action('close')
        collect(lambda c: c['green'] == 0 and c['blue'] == 0 and c['light'] > 100)
        for _ in range(8): action('pulse'); time.sleep(.07)
        action('close'); peer.send(type='video.keyframe', window_id=root_id)
        collect(lambda c: c['green'] == 0 and c['blue'] == 0)
        assert len(metadata) == 1 and len(monitor_layout()) == len(baseline)+1
        peer.close()
        wait(lambda: monitor_layout() == baseline)
        resumed = Peer(sock)
        try:
            snapshot = resumed.request('session.resume', 'session.snapshot', session_id=started['session_id'])
            window = snapshot['windows'][0] if snapshot['windows'] else resumed.until('window.created')
            assert window['surface_type'] == 'session' and window['window_id'] == root_id
            resumed.until('frame')
        finally: resumed.close()
        wait(lambda: monitor_layout() == baseline)
        # Switching a detached external session must release only its lease,
        # preserving the application process and obtaining a fresh presentation.
        switched = Peer(sock)
        try:
            switched.request('app.launch', 'session.started', app_id=target['id'], request_id='scene-to-windows', backend='native', scale_factor=scale)
            window = switched.until('window.created')
            assert window.get('surface_type') != 'session' and window['window_id'] == root_id
            switched.until('frame')
        finally: switched.close()
        wait(lambda: monitor_layout() == baseline)
        assert app.poll() is None
        print('PASS: one native scene/stream for root, dialog and nested menus; unrelated same-PID window excluded; popup churn and reconnect; output cleanup preserves app', flush=True)
    finally:
        action('close'); bridge.close(); pipeline.set_state(Gst.State.NULL)
