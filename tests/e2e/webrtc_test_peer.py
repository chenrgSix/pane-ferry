"""Test-only Agent client with native WebRTC receiver; never imports a user's catalog."""
import base64
import ctypes as C
import json
import queue
import socket
import struct
import threading


class RTCConnection:
    def __init__(self, path, library):
        self.events = queue.Queue()
        self.accept_fallback = False
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(20)
        self.socket.connect(str(path))
        self.wire = self.socket.makefile('rb', buffering=65536)
        self.api = C.CDLL(str(library.resolve()))
        self.callback_type = C.CFUNCTYPE(None, C.c_void_p, C.c_int, C.c_char_p, C.c_void_p, C.c_size_t)
        self.api.ra_rtc_create.argtypes = [C.c_int, C.c_char_p, self.callback_type, C.c_void_p]
        self.api.ra_rtc_create.restype = C.c_void_p
        for name in ['ra_rtc_description', 'ra_rtc_candidate']:
            getattr(self.api, name).argtypes = [C.c_void_p, C.c_char_p, C.c_char_p]
        self.api.ra_rtc_close.argtypes = [C.c_void_p]

        @self.callback_type
        def callback(_context, kind, label, data, size):
            if size > 12 * 1024 * 1024 + 4100:
                self.events.put(dict(type='_rtc.error', code='oversized_callback'))
                return
            payload = C.string_at(data, size) if size else b''
            if kind == 3:
                length = struct.unpack('>I', payload[:4])[0]
                metadata = json.loads(payload[4:4 + length])
                metadata['data'] = base64.b64encode(payload[4 + length:]).decode()
                metadata['_actual_webrtc'] = True
                self.events.put(metadata)
            elif kind in (1, 2):
                self.events.put(dict(type='_rtc.signal', kind=kind, label=label.decode(), payload=payload.decode()))
            elif kind == 6 or (kind == 5 and label in (b'4', b'5')):
                self.events.put(dict(type='_rtc.error', code='media_connection_failed'))

        self.callback = callback
        self.handle = self.api.ra_rtc_create(0, b'127.0.0.1', self.callback, None)
        assert self.handle

        def reader():
            try:
                while True:
                    line = self.wire.readline(16 * 1024 * 1024 + 1)
                    if not line:
                        break
                    self.events.put(json.loads(line))
            except OSError:
                pass
            finally:
                self.events.put(dict(type='_rtc.error', code='Agent_closed'))
        self.reader = threading.Thread(target=reader, daemon=True)
        self.reader.start()
        self.send(type='hello')
        hello = self.read()
        assert hello['type'] == 'hello' and 'native.webrtc.h264' in hello['capabilities']
        self.send(type='webrtc.enable')
        ready = self.read()
        assert ready['type'] == 'video.ready' and ready['transport'] == 'webrtc'

    def send(self, **value):
        self.socket.sendall((json.dumps(dict(value, version=1)) + '\n').encode())

    def read(self):
        while True:
            value = self.events.get(timeout=20)
            kind = value['type']
            if kind == 'webrtc.offer':
                assert self.api.ra_rtc_description(self.handle, value['sdp'].encode(), b'offer') == 0
            elif kind == 'webrtc.candidate':
                assert self.api.ra_rtc_candidate(self.handle, value['candidate'].encode(), value['mid'].encode()) == 0
            elif kind in ('webrtc.state', 'webrtc.pulse'):
                continue
            elif kind == '_rtc.signal':
                if value['kind'] == 1:
                    assert value['label'] == 'answer'
                    self.send(type='webrtc.answer', sdp=value['payload'])
                else:
                    self.send(type='webrtc.candidate', candidate=value['payload'], mid=value['label'])
            elif kind == '_rtc.error':
                if not self.accept_fallback or value['code'] == 'Agent_closed':
                    raise AssertionError(value)
            else:
                if kind == 'frame' and not self.accept_fallback:
                    assert value.get('_actual_webrtc') and value.get('transport') == 'webrtc', 'silent paired-control media fallback'
                return value

    def close(self):
        self.socket.shutdown(socket.SHUT_RDWR)
        self.reader.join(timeout=2)
        self.wire.close(); self.socket.close()
        self.api.ra_rtc_close(self.handle)
        self.handle = None
