#!/usr/bin/env python3
"""Two isolated localhost WebRTC peers: real ICE/DTLS/SRTP and H264 RTP, no GUI."""
import argparse
import ctypes as C
import json
from pathlib import Path
import queue
import struct
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--library', required=True, type=Path)
args = parser.parse_args()
api = C.CDLL(str(args.library.resolve()))
callback_type = C.CFUNCTYPE(None, C.c_void_p, C.c_int, C.c_char_p, C.c_void_p, C.c_size_t)
api.ra_rtc_create.argtypes = [C.c_int, C.c_char_p, callback_type, C.c_void_p]
api.ra_rtc_create.restype = C.c_void_p
api.ra_rtc_add_video.argtypes = [C.c_void_p, C.c_uint32]
api.ra_rtc_description.argtypes = [C.c_void_p, C.c_char_p, C.c_char_p]
api.ra_rtc_candidate.argtypes = [C.c_void_p, C.c_char_p, C.c_char_p]
api.ra_rtc_send.argtypes = [C.c_void_p, C.c_uint32, C.c_void_p, C.c_size_t, C.c_void_p, C.c_size_t]
api.ra_rtc_selected_pair.argtypes = [C.c_void_p, C.c_void_p, C.c_size_t]
api.ra_rtc_close.argtypes = [C.c_void_p]
events = queue.Queue()
peers = []
frames = []
states = {}


@callback_type
def callback(context, kind, label, data, size):
    if size > 12 * 1024 * 1024 + 4100:
        events.put((int(context), 6, b'oversized_callback', b''))
    else:
        events.put((int(context), kind, label, C.string_at(data, size) if size else b''))


def pump(predicate, timeout=12):
    until = time.monotonic() + timeout
    while not predicate():
        assert time.monotonic() < until, ('WebRTC test timeout', states, len(frames))
        try:
            index, kind, label, payload = events.get(timeout=.02)
        except queue.Empty:
            continue
        other = peers[2 - index]
        if kind == 1:
            assert api.ra_rtc_description(other, payload, label) == 0, ('description', label)
        elif kind == 2:
            assert api.ra_rtc_candidate(other, payload, label) == 0, 'candidate'
        elif kind == 3:
            assert index == 2
            size = struct.unpack('>I', payload[:4])[0]
            assert 0 < size <= 4096
            frames.append((label, json.loads(payload[4:4 + size]), payload[4 + size:]))
        elif kind == 5:
            states[index] = int(label)
        elif kind == 6:
            raise AssertionError(label)


try:
    peers = [api.ra_rtc_create(index == 1, b'127.0.0.1', callback, index) for index in (1, 2)]
    assert all(peers), 'cannot create WebRTC peers'
    assert api.ra_rtc_add_video(peers[0], 7) == 0
    pump(lambda: states.get(1) == states.get(2) == 2)
    pump(lambda: api.ra_rtc_add_video(peers[0], 7) == 1)
    pair = C.create_string_buffer(4096)
    assert api.ra_rtc_selected_pair(peers[0], pair, len(pair)) > 0
    assert b'udp' in pair.value.lower() and b'127.0.0.1' in pair.value
    for i, size in enumerate((32, 4096, 350000)):
        raw = b'\0\0\0\1\x65' + b'\x55' * size
        metadata = dict(window_id=7, frame_id=i + 1, width=2940, height=1612, geometry_revision=i + 1)
        header = json.dumps(metadata).encode()
        assert api.ra_rtc_send(peers[0], 7, raw, len(raw), header, len(header)) == 0
        pump(lambda: len(frames) == i + 1)
        assert frames[-1] == (b'7', metadata, raw), 'media bytes changed'
    assert api.ra_rtc_add_video(peers[0], 9) == 0
    pump(lambda: api.ra_rtc_add_video(peers[0], 9) == 1)
    raw = b'\0\0\0\1\x65\x42'
    assert api.ra_rtc_send(peers[0], 9, raw, len(raw), b'{}', 2) == 0
    pump(lambda: len(frames) == 4)
    assert frames[-1] == (b'9', {}, raw)
    assert api.ra_rtc_send(peers[0], 9, raw, len(raw), b'x' * 4097, 4097) < 0
    print('PASS: ICE/DTLS-SRTP over UDP, 350KB fragmented H264 AU exact round trip, in-media metadata, second-window renegotiation and size bounds')
finally:
    for peer in peers:
        if peer:
            api.ra_rtc_close(peer)
