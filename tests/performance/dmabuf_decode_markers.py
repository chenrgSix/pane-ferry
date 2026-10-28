"""Decode owned fixture H264 after measurement; emit one counter byte per frame."""
import sys
import gi

gi.require_version('Gst', '1.0')
gi.require_version('GstApp', '1.0')
from gi.repository import Gst

Gst.init(None)
width, height, frames = map(int, sys.argv[1:])
pipeline = Gst.parse_launch(
    'appsrc name=source is-live=true format=time caps=video/x-h264,stream-format=byte-stream '
    '! h264parse ! openh264dec '
    f'! videocrop right={width - 256} bottom={height - 32} '
    '! videoconvert ! video/x-raw,format=RGB '
    '! appsink name=sink sync=false max-buffers=1')
pipeline.set_state(Gst.State.PLAYING)
try:
    data = sys.stdin.buffer.read()
    buffer = Gst.Buffer.new_allocate(None, len(data), None)
    buffer.fill(0, data)
    assert pipeline.get_by_name('source').emit('push-buffer', buffer) == Gst.FlowReturn.OK
    pipeline.get_by_name('source').emit('end-of-stream')
    for _ in range(frames):
        sample = pipeline.get_by_name('sink').emit('try-pull-sample', 2 * Gst.SECOND)
        assert sample is not None, 'missing decoded frame'
        caps = sample.get_caps().get_structure(0)
        assert (caps.get_value('width'), caps.get_value('height')) == (256, 32)
        pixels = sample.get_buffer().extract_dup(0, sample.get_buffer().get_size())
        counter = sum((pixels[(16 * 256 + bit * 32 + 16) * 3] > 128) << bit for bit in range(8))
        sys.stdout.buffer.write(bytes([counter]))
    assert pipeline.get_by_name('sink').emit('try-pull-sample', Gst.SECOND) is None, 'extra decoded frame'
finally:
    pipeline.set_state(Gst.State.NULL)
