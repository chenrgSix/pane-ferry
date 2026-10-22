#!/usr/bin/env python3
"""Linux-only measurement of queued input and visible response on an isolated 2560x1664 session."""
import argparse
import base64
import io
import json
import os
from pathlib import Path
import queue
import socket
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from PIL import Image

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
parser.add_argument('--max-input-ms', type=float, default=0)
parser.add_argument('--samples', type=int, default=5)
parser.add_argument('--motions', type=int, default=32)
parser.add_argument('--stall-reader', action='store_true')
parser.add_argument('--video', action='store_true')
args = parser.parse_args()
class VideoDecoder:
    def __init__(self):
        import gi
        gi.require_version('Gst', '1.0')
        gi.require_version('GstApp', '1.0')
        from gi.repository import Gst
        self.Gst = Gst
        Gst.init(None)
        self.pipeline = Gst.parse_launch('appsrc name=source is-live=true format=time caps=video/x-h264,stream-format=byte-stream,alignment=au '
            '! h264parse ! openh264dec ! videoconvert ! video/x-raw,format=RGB ! appsink name=sink sync=false max-buffers=1')
        self.source, self.sink = self.pipeline.get_by_name('source'), self.pipeline.get_by_name('sink')
        self.pipeline.set_state(Gst.State.PLAYING)
        self.sequence = 0
    def red(self, frame):
        assert frame.get('codec') == 'h264', frame.get('video_error')
        packet = base64.b64decode(frame['data'])
        buf = self.Gst.Buffer.new_allocate(None, len(packet), None)
        buf.fill(0, packet)
        buf.pts = self.sequence * self.Gst.SECOND // 30
        self.sequence += 1
        assert self.source.emit('push-buffer', buf) == self.Gst.FlowReturn.OK
        sample = self.sink.emit('try-pull-sample', self.Gst.SECOND)
        assert sample is not None, 'video decode timeout'
        width = sample.get_caps().get_structure(0).get_value('width')
        stride = (width * 3 + 3) & ~3
        return sample.get_buffer().extract_dup(20 * stride + 20 * 3, 1)[0]
    def close(self): self.pipeline.set_state(self.Gst.State.NULL)
with tempfile.TemporaryDirectory(prefix='paneferry-latency-') as temporary:
    home = Path(temporary)
    config = home / '.config/lan-remoteapp'
    config.mkdir(parents=True)
    state = home / 'key.json'
    (config / 'apps.json').write_text(json.dumps([dict(id='latency', name='Latency',
        executable=sys.executable, argv=[str(Path(__file__).with_name('latency_fixture.py').resolve()), str(state)])]))
    env = dict(os.environ, HOME=str(home), XMODIFIERS='@im=none', GTK_IM_MODULE='simple')
    for name in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER'):
        env.pop(name, None)
    agent_log = (home / 'agent.log').open('w+')
    agent = subprocess.Popen([str(args.agent.resolve()), 'serve'], env=env,
                             stdout=subprocess.DEVNULL, stderr=agent_log)
    try:
        path = home / '.local/state/lan-remoteapp/agent.sock'
        deadline = time.monotonic() + 10
        while not path.exists():
            assert time.monotonic() < deadline
            time.sleep(.02)
        with socket.socket(socket.AF_UNIX) as peer:
            peer.settimeout(30)
            peer.connect(str(path))
            # SocketIO.readline without buffering performs a syscall per byte,
            # accidentally measuring a deliberately slow consumer instead of capture.
            wire = peer.makefile('rb', buffering=65536)
            events = queue.Queue()
            running = True
            read_gate, paused = threading.Event(), threading.Event()
            read_gate.set()
            def reader():
                decoder = VideoDecoder() if args.video else None
                try:
                    while running:
                        if not read_gate.is_set(): paused.set()
                        read_gate.wait()
                        line = wire.readline(16 * 1024 * 1024 + 1)
                        if not line: break
                        arrived = time.monotonic()
                        event = json.loads(line)
                        if decoder and event['type'] == 'frame': event['response_red'] = decoder.red(event)
                        events.put((arrived, event, len(line)))
                except (OSError, ValueError): pass
                except Exception as error:
                    events.put((time.monotonic(), dict(type='error', code=repr(error)), 0))
                finally:
                    if decoder: decoder.close()
            reader_thread = threading.Thread(target=reader, daemon=True)
            reader_thread.start()
            def send(**data): peer.sendall((json.dumps(dict(data, version=1)) + '\n').encode())
            def until(kind, predicate=lambda event: True):
                deadline = time.monotonic() + 30
                while time.monotonic() < deadline:
                    arrived, event, size = events.get(timeout=max(.01, deadline-time.monotonic()))
                    assert event['type'] != 'error', event
                    if event['type'] == kind and predicate(event): return arrived, event, size
                raise AssertionError(f'missing {kind}')
            send(type='hello'); hello = until('hello')[1]
            if args.video:
                assert 'native.h264' in hello['capabilities']
                send(type='video.enable', codec='h264'); until('video.ready')
            send(type='app.launch', app_id='latency', request_id='measure', backend='native')
            until('session.started')
            _, window, _ = until('window.created', lambda event: event['title'] == 'PaneFerryLatencyFixture')
            wid = window['window_id']
            assert (window['width'], window['height']) == (2560, 1664), window
            until('frame', lambda event: event['window_id'] == wid)
            send(type='window.focus', window_id=wid)
            inputs, visible, sizes = [], [], []
            for sample in range(1, args.samples + 1):
                started = time.monotonic()
                # About half a second of mouse motion, followed by an actual key.
                for x in range(args.motions):
                    send(type='input.pointer', window_id=wid, x=100+x, y=100, button=0, pressed=False)
                send(type='input.key', window_id=wid, keysym=ord('a') + sample, pressed=True)
                send(type='input.key', window_id=wid, keysym=ord('a') + sample, pressed=False)
                deadline = started + 30
                while True:
                    try: result = json.loads(state.read_text())
                    except (FileNotFoundError, json.JSONDecodeError): result = {}
                    if result.get('count') == sample: break
                    if time.monotonic() >= deadline:
                        agent_log.seek(0)
                        summary = [(e[1]['type'], e[1].get('code')) for e in list(events.queue)]
                        raise AssertionError(('input was not delivered', result, summary[-20:], agent_log.read()[-2000:]))
                    time.sleep(.005)
                inputs.append((result['received'] - started) * 1000)
                while True:
                    arrived, frame, size = until('frame', lambda event: event['window_id'] == wid)
                    if args.video:
                        red = frame['response_red']
                    else:
                        with Image.open(io.BytesIO(base64.b64decode(frame['png']))) as image:
                            red = image.convert('RGB').getpixel((20, 20))[0]
                    if (red > 128) == bool(sample % 2): break
                visible.append((arrived - started) * 1000)
                sizes.append(size)
            result = dict(codec='h264' if args.video else 'png', input_ms=[round(v, 1) for v in inputs], visible_ms=[round(v, 1) for v in visible],
                          median_input_ms=round(statistics.median(inputs), 1), median_visible_ms=round(statistics.median(visible), 1),
                          mean_frame_bytes=round(statistics.mean(sizes)))
            if args.stall_reader:
                read_gate.clear()
                assert paused.wait(2), 'reader did not pause'
                time.sleep(.25)  # Let a >1 MiB frame fill the unread socket buffer.
                started = time.monotonic()
                send(type='input.key', window_id=wid, keysym=ord('z'), pressed=True)
                send(type='input.key', window_id=wid, keysym=ord('z'), pressed=False)
                while True:
                    try: received = json.loads(state.read_text())
                    except (FileNotFoundError, json.JSONDecodeError): received = {}
                    if received.get('count') == args.samples + 1: break
                    assert time.monotonic() - started < .5, 'slow reader blocked input'
                    time.sleep(.005)
                result['stalled_reader_input_ms'] = round((received['received'] - started) * 1000, 1)
                read_gate.set()
            print(json.dumps(result), flush=True)
            if args.max_input_ms:
                assert max(inputs) < args.max_input_ms, result
            running = False
            read_gate.set()
            peer.shutdown(socket.SHUT_RDWR)
            reader_thread.join(timeout=2)
            wire.close()
    finally:
        agent.terminate()
        agent.wait(timeout=10)
