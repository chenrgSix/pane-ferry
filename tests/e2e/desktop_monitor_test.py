"""Real private GNOME desktop video; never reads or controls the login desktop."""
import base64
import json
import time

import gi
gi.require_version('Gst', '1.0')
from gi.repository import Gst


def verify(peer, sock, env, state, app, wait, Peer, Bridge, monitor_layout, physical):
    Gst.init(None)
    baseline = monitor_layout()
    bridge = Bridge(env)
    command = state.with_suffix('.command')
    serial = 0
    pipeline = None

    def action(name):
        nonlocal serial
        serial += 1
        temporary = command.with_suffix('.new')
        temporary.write_text(json.dumps({'serial': serial, 'action': name}))
        temporary.replace(command)

    def decoder():
        nonlocal pipeline
        if pipeline:
            pipeline.set_state(Gst.State.NULL)
        pipeline = Gst.parse_launch('appsrc name=input format=time is-live=true caps=video/x-h264,stream-format=byte-stream,alignment=au ! h264parse ! avdec_h264 ! videoconvert ! video/x-raw,format=RGB ! appsink name=output sync=false max-buffers=1 drop=true')
        pipeline.set_state(Gst.State.PLAYING)
        return pipeline.get_by_name('input'), pipeline.get_by_name('output')

    source, sink = decoder()
    stream = None
    metadata = None
    canvas = None

    def collect(predicate):
        nonlocal source, sink, stream, metadata, canvas
        until = time.monotonic() + 20
        while time.monotonic() < until:
            event = json.loads(peer.file.readline(16 * 1024 * 1024 + 1))
            assert event['type'] not in ('error', 'session.exited'), event
            if event['type'] in ('window.created', 'window.metadata'):
                assert event['surface_type'] == 'desktop' and event['parent_id'] is None, event
                metadata = event
            if event['type'] != 'frame':
                continue
            assert event['window_id'] == 1
            assert event['width'] <= 4096 and event['height'] <= 4096
            assert event['width'] * event['height'] <= 8_000_000
            assert (event['width'], event['height']) == (metadata['width'], metadata['height'])
            if event['stream_id'] != stream:
                assert event['key_frame'], 'new stream did not start with IDR'
                stream = event['stream_id']
                source, sink = decoder()
            data = base64.b64decode(event['data'])
            buffer = Gst.Buffer.new_allocate(None, len(data), None)
            buffer.fill(0, data)
            assert source.emit('push-buffer', buffer) == Gst.FlowReturn.OK
            sample = sink.emit('try-pull-sample', Gst.SECOND)
            if sample is None:
                continue
            structure = sample.get_caps().get_structure(0)
            canvas = (structure.get_value('width'), structure.get_value('height'))
            assert canvas == (event['encoded_width'], event['encoded_height'])
            buffer = sample.get_buffer()
            ok, mapping = buffer.map(Gst.MapFlags.READ)
            assert ok
            try:
                pixels = bytes(mapping.data)
            finally:
                buffer.unmap(mapping)
            if predicate(pixels, canvas):
                return pixels, canvas
        raise AssertionError('desktop video did not reach expected content')

    def patch(pixels, canvas, point):
        width, height = canvas
        x = round((point[0] - logical['x']) * width / logical['width'])
        y = round((point[1] - logical['y']) * height / logical['height'])
        stride = (width * 3 + 3) & ~3
        radius = 35
        assert 0 < x < width - radius and 0 < y < height - radius
        return b''.join(pixels[row * stride + x * 3:row * stride + (x + radius) * 3]
                        for row in range(y, y + radius))

    def difference(a, b):
        return sum(abs(x - y) > 30 for x, y in zip(a, b))

    def colors(pixels, _canvas):
        green = blue = red = 0
        for i in range(0, len(pixels) - 2, 3 * 23):
            r, g, b = pixels[i:i+3]
            green += g > 150 and r < 80 and b < 100
            blue += b > 150 and r < 80 and g < 100
            red += r > 200 and g < 50 and b < 50
        return green, blue, red

    try:
        setup = bridge.call(op='test_desktop_setup', pid=app.pid)
        assert 'result' in setup, setup
        logical = setup['result']
        monitors = peer.request('desktop.list')['monitors']
        monitor = next((m for m in monitors if (m['width'], m['height']) == physical), None)
        assert monitor, ('physical desktop missing', physical, monitors)
        started = peer.request('desktop.start', 'session.started', monitor_id=monitor['id'],
                               request_id='complete-desktop')
        snapshot = peer.request('session.resume', 'session.snapshot', session_id=started['session_id'])
        assert snapshot['windows'][0]['surface_type'] == 'desktop', snapshot
        metadata = snapshot['windows'][0]
        before, canvas = collect(lambda pixels, size: True)
        assert canvas[0] <= physical[0] and canvas[1] <= physical[1]
        assert abs(canvas[0] / physical[0] - canvas[1] / physical[1]) < .002
        assert monitor_layout() == baseline
        point_a = (logical['x'] + logical['width'] - 450, logical['y'] + logical['height'] - 180)
        point_b = (logical['x'] + logical['width'] - 250, logical['y'] + logical['height'] - 180)
        reference_a = patch(before, canvas, point_a)
        reference_b = patch(before, canvas, point_b)
        assert 'result' in bridge.call(op='test_desktop_pointer', x=point_a[0], y=point_a[1])
        moved_a, _ = collect(lambda pixels, size: difference(reference_a, patch(pixels, size, point_a)) > 50)
        assert difference(reference_b, patch(moved_a, canvas, point_b)) < 30
        assert 'result' in bridge.call(op='test_desktop_pointer', x=point_b[0], y=point_b[1])
        collect(lambda pixels, size: difference(reference_b, patch(pixels, size, point_b)) > 50
                and difference(reference_a, patch(pixels, size, point_a)) < 30)
        cursor_stream = stream
        action('open')
        collect(lambda pixels, size: colors(pixels, size)[0] > 50 and colors(pixels, size)[1] > 10)
        assert stream == cursor_stream, 'desktop popup unnecessarily restarted the stream'
        action('stranger')
        collect(lambda pixels, size: colors(pixels, size)[2] > 100)
        assert stream == cursor_stream, 'unrelated desktop window restarted the stream'
        # Whole desktop deliberately includes unrelated windows. Application
        # scene tests separately verify their exclusion from scoped sessions.
        peer.send(type='window.configure', window_id=1, width=640, height=480)
        assert peer.until('error')['code'] == 'desktop_geometry_fixed'
        peer.send(type='window.close.request', window_id=1)
        assert peer.until('error')['code'] == 'desktop_close_unavailable'
        assert monitor_layout() == baseline and app.poll() is None
        peer.close()
        peer = Peer(sock)
        snapshot = peer.request('session.resume', 'session.snapshot', session_id=started['session_id'])
        metadata = snapshot['windows'][0]
        stream = None
        collect(lambda pixels, size: colors(pixels, size)[2] > 100)
        assert monitor_layout() == baseline and app.poll() is None
        print(f'PASS: desktop {physical} -> {canvas}, decoded moving cursor, dialog/nested popup, '
              'unrelated window on continuous stream, fixed desktop geometry, detach/reconnect preserves source')
    finally:
        action('close')
        peer.close()
        bridge.close()
        if pipeline:
            pipeline.set_state(Gst.State.NULL)
