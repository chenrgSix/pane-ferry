#!/usr/bin/env python3
"""Serialized capture -> packet comparison, with three retained private BOs.

Run trials sequentially in alternating orders. This never publishes to the Agent
socket and does not measure production queueing, QUIC, or remote presentation.
Decode the fixture counters only after timing; fail on missing/reordered frames.
"""
import argparse
import ctypes as C
import hashlib
import json
from pathlib import Path
import subprocess
import time

from dmabuf_validation import Canvas, Native, threads
from local_scroll import distribution


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--library', type=Path, required=True)
    parser.add_argument('--mode', choices=('software', 'staging-va', 'dmabuf-va'), required=True)
    parser.add_argument('--width', type=int, default=2940)
    parser.add_argument('--height', type=int, default=1618)
    parser.add_argument('--warmup', type=int, default=16)
    parser.add_argument('--frames', type=int, default=90)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--await-fence', action='store_true')
    args = parser.parse_args()
    if not (256 <= args.width <= 4096 and 34 <= args.height <= 4096
            and args.width * args.height <= 8_000_000
            and args.width % 2 == args.height % 2 == 0
            and 2 <= args.warmup <= 120 and 60 <= args.frames <= 300):
        parser.error('invalid bounded fixture geometry or frame count')
    native = Native(args.library)
    mode = {'software': 0, 'staging-va': 1, 'dmabuf-va': 3}[args.mode]
    canvases, inputs, encoder = [], [], None
    stream, samples, keyframes, input_markers = bytearray(), [], [], []
    try:
        for _ in range(3):
            canvas = Canvas(args.width, args.height, alignment=16, await_fence=args.await_fence)
            canvases.append(canvas)
            source = native.input_new(canvas.fd, args.width, args.height, canvas.stride, mode < 2)
            assert source
            inputs.append(source)
        encoder = native.encoder_new(args.width, args.height, 60, threads(args.width, args.height), mode, b'')
        assert encoder and not native.error(encoder)
        for sequence in range(args.warmup + args.frames):
            if sequence == args.warmup:
                cpu_start, wall_start = time.process_time_ns(), time.monotonic_ns()
            slot = sequence % len(canvases)
            canvas = canvases[slot]
            canvas.draw(sequence % 256)
            begin = canvas.capture(sequence * 11)
            packet, key, points = native.packet(encoder, inputs[slot])
            input_markers.append(native.input_counter(encoder))
            keyframes.append(sequence) if key else None
            stream.extend(packet)
            if sequence >= args.warmup:
                samples.append(dict(sequence=sequence, slot=slot, begin_ns=begin,
                                    input_begin_ns=points[0], input_ready_ns=points[1], packet_ready_ns=points[2],
                                    bytes=len(packet)))
        wall_ns, cpu_ns = time.monotonic_ns() - wall_start, time.process_time_ns() - cpu_start
        pointer = native.caps(encoder)
        caps = C.string_at(pointer).decode()
        native.free(pointer)
        # No further capture until pipeline NULL; BOs outlive all GstMemory refs.
        native.encoder_free(encoder)
        encoder = None
        markers = subprocess.run(['/usr/bin/python3', str(Path(__file__).with_name('dmabuf_decode_markers.py')),
                                  str(args.width), str(args.height), str(args.warmup + args.frames)],
                                 input=bytes(stream), stdout=subprocess.PIPE, stderr=subprocess.PIPE, check=True).stdout
        expected = bytes(n % 256 for n in range(args.warmup + args.frames))
        result = dict(schema=1, mode=args.mode, dimensions=[args.width, args.height],
                      buffers=len(canvases), stride=canvases[0].stride, warmup=args.warmup,
                      await_fence=args.await_fence,
                      frames=args.frames, frame_markers_pass=markers == expected,
                      decoded_frames=len(markers), decoded_markers=list(markers), keyframes=keyframes,
                      input_markers=input_markers,
                      keyframe_cadence_pass=keyframes == list(range(0, args.warmup + args.frames, 60)),
                      capture_to_packet_ms=distribution([(s['packet_ready_ns'] - s['begin_ns']) / 1e6 for s in samples]),
                      input_ready_ms=distribution([(s['input_ready_ns'] - s['input_begin_ns']) / 1e6 for s in samples]),
                      convert_encode_ms=distribution([(s['packet_ready_ns'] - s['input_ready_ns']) / 1e6 for s in samples]),
                      fps=args.frames / (wall_ns / 1e9), cpu_percent=100 * cpu_ns / wall_ns,
                      packet_bytes=sum(s['bytes'] for s in samples), wall_ns=wall_ns, cpu_ns=cpu_ns,
                      stream_sha256=hashlib.sha256(stream).hexdigest(),
                      caps=caps, samples=samples,
                      limitation='Serialized private pixmaps -> encoded packet; no Agent socket or remote endpoint')
        args.output.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({k: v for k, v in result.items()
                          if k not in ('samples', 'input_markers', 'decoded_markers')}), flush=True)
        if not result['frame_markers_pass']:
            args.output.with_suffix('.failed.h264').write_bytes(stream)
        assert result['frame_markers_pass'] and result['keyframe_cadence_pass'], 'frame reuse/GOP check failed'
    finally:
        if encoder:
            native.encoder_free(encoder)
        for source in inputs:
            native.input_free(source)
        for canvas in canvases:
            canvas.close()


if __name__ == '__main__':
    main()
