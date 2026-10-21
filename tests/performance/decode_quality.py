"""Decode one AU to packed RGB, for native/legacy encoder pixel comparison."""
import sys
import gi
gi.require_version('Gst', '1.0')
gi.require_version('GstApp', '1.0')
from gi.repository import Gst
Gst.init(None)
width, height, frames = map(int, sys.argv[1:])
pipeline = Gst.parse_launch('appsrc name=source is-live=true format=time '
    'caps=video/x-h264,stream-format=byte-stream '
    '! h264parse ! openh264dec ! videoconvert ! video/x-raw,format=RGB '
    '! appsink name=sink sync=false max-buffers=1')
pipeline.set_state(Gst.State.PLAYING)
try:
    data = sys.stdin.buffer.read()
    buffer = Gst.Buffer.new_allocate(None, len(data), None)
    buffer.fill(0, data)
    buffer.pts = 0
    assert pipeline.get_by_name('source').emit('push-buffer', buffer) == Gst.FlowReturn.OK
    pipeline.get_by_name('source').emit('end-of-stream')
    for _ in range(frames):
        sample = pipeline.get_by_name('sink').emit('try-pull-sample', 2 * Gst.SECOND)
        assert sample is not None
        caps = sample.get_caps().get_structure(0)
        ew, eh = caps.get_value('width'), caps.get_value('height')
        assert ew == width + width % 2 and eh == height + height % 2
        stride = (ew * 3 + 3) & ~3
        pixels = sample.get_buffer().extract_dup(0, sample.get_buffer().get_size())
        sys.stdout.buffer.write(b''.join(pixels[y*stride:y*stride+width*3] for y in range(height)))
finally:
    pipeline.set_state(Gst.State.NULL)
