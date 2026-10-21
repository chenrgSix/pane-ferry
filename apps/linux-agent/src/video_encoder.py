"""Fixed private worker, embedded in the Agent. BGRx in; length-delimited H.264 AU out."""
import struct
import sys
import gi
gi.require_version('Gst', '1.0')
gi.require_version('GstApp', '1.0')
from gi.repository import Gst, GstApp

Gst.init(None)
required = ('appsrc', 'videoconvert', 'videobox', 'openh264enc', 'h264parse', 'appsink')
if not all(Gst.ElementFactory.find(name) for name in required):
    raise SystemExit('H.264 components unavailable')
if sys.argv[1:] == ['--check']:
    raise SystemExit(0)

width, height = map(int, sys.argv[1:])
assert 0 < width <= 4096 and 0 < height <= 4096 and width * height <= 8_000_000
bitrate = max(2_000_000, min(24_000_000, width * height * 3))
pipeline = Gst.parse_launch(
    f'appsrc name=source is-live=true format=time block=true max-buffers=1 '
    f'caps=video/x-raw,format=BGRx,width={width},height={height},framerate=30/1 '
    f'! videobox right={-(width % 2)} bottom={-(height % 2)} '
    f'! videoconvert ! video/x-raw,format=I420 '
    f'! openh264enc usage-type=screen complexity=low rate-control=bitrate '
    f'bitrate={bitrate} max-bitrate={bitrate * 2} gop-size=60 multi-thread=4 '
    f'enable-frame-skip=false qp-max=28 '
    f'! h264parse config-interval=-1 '
    f'! video/x-h264,stream-format=byte-stream,alignment=au '
    f'! appsink name=sink sync=false max-buffers=1 drop=false')
source, sink = pipeline.get_by_name('source'), pipeline.get_by_name('sink')
if pipeline.set_state(Gst.State.PLAYING) == Gst.StateChangeReturn.FAILURE:
    raise SystemExit('H.264 pipeline unavailable')

def read_exact(size):
    data = bytearray()
    while len(data) < size:
        chunk = sys.stdin.buffer.read(size - len(data))
        if not chunk:
            if data: raise EOFError('incomplete raw frame')
            return None
        data.extend(chunk)
    # PyGObject marshals bytearray byte by byte; bytes uses its buffer fast path.
    return bytes(data)

try:
    sequence = 0
    while True:
        header = read_exact(4)
        if header is None: break
        length, = struct.unpack('>I', header)
        assert length == width * height * 4
        data = read_exact(length)
        if data is None: raise EOFError('missing raw frame')
        buffer = Gst.Buffer.new_allocate(None, length, None)
        buffer.fill(0, data)
        buffer.pts = sequence * Gst.SECOND // 30
        buffer.duration = Gst.SECOND // 30
        sequence += 1
        if source.emit('push-buffer', buffer) != Gst.FlowReturn.OK:
            raise RuntimeError('H.264 input rejected')
        sample = sink.emit('try-pull-sample', 2 * Gst.SECOND)
        if sample is None: raise RuntimeError('H.264 output timeout')
        encoded = sample.get_buffer()
        packet = encoded.extract_dup(0, encoded.get_size())
        assert 0 < len(packet) <= 12 * 1024 * 1024
        keyframe = not encoded.has_flags(Gst.BufferFlags.DELTA_UNIT)
        sys.stdout.buffer.write(struct.pack('>IB', len(packet), keyframe) + packet)
        sys.stdout.buffer.flush()
finally:
    pipeline.set_state(Gst.State.NULL)
