"""Opt-in publisher benchmark hook for the private GNOME smoke-test harness."""
import json
from pathlib import Path
import time

from local_scroll import distribution


def verify(peer, env, folder, log, start_child, wait, Bridge, output):
    width, height, fps, seconds = 2940, 1618, 60, 8
    fixture = Path(__file__).with_name('scroll_fixture.py').resolve()
    draws_path = folder / 'scroll-draws.jsonl'
    app = start_child(['/usr/bin/python3', str(fixture), str(draws_path), str(width), str(height), '--undecorated'],
                      dict(env, GDK_BACKEND='wayland'), log)
    wait(draws_path.exists)
    bridge = Bridge(env)
    try:
        def target():
            return next((window for window in bridge.call(op='list').get('result', [])
                         if window['pid'] == app.pid), None)
        window = wait(target)
        row = next(row for row in peer.request('window.list')['windows']
                   if row['window_id'] == window['window_id'])
        peer.request('app.launch', 'session.started', app_id=row['id'],
                     request_id='scroll-benchmark', backend='native', scale_factor=1,
                     scene=dict(width=width, height=height))
        peer.until('frame')
        start = time.monotonic_ns() + 2_000_000_000
        end = start + int(seconds * 1e9)
        frames = []
        while True:
            raw = peer.file.readline(16 * 1024 * 1024 + 1)
            arrived = time.monotonic_ns()
            assert raw.endswith(b'\n'), 'invalid or truncated event'
            event = json.loads(raw)
            assert event['type'] != 'error', event
            if arrived >= end:
                break
            if event['type'] != 'frame' or arrived < start:
                continue
            assert event['window_id'] == window['window_id'] and event['codec'] == 'h264'
            assert (event['width'], event['height']) == (width, height)
            frames.append(dict(ns=arrived, sequence=event['seq'], json_bytes=len(raw), key_frame=event['key_frame']))
        assert len(frames) >= 2
        draws = [json.loads(line) for line in draws_path.read_text().splitlines()]
        draws = [row for row in draws if start <= row['ns'] < end]
        assert len(draws) >= 2 and all((row['width'], row['height']) == (width, height) for row in draws), {
            (row['width'], row['height']) for row in draws}
        gaps = [(b['ns'] - a['ns']) / 1e6 for a, b in zip(frames, frames[1:])]
        result = dict(schema=1, capture='private GNOME application scene + PipeWire; local Unix socket',
                      width=width, height=height, cap_fps=fps, seconds=seconds,
                      source_draw_fps=len(draws) / seconds, received_fps=len(frames) / seconds,
                      frame_gap_ms=distribution(gaps), frames=frames, source_draws=draws,
                      limitations=['Publisher delivery only; no QUIC, client decode or display',
                                   'Private compositor; not a measurement of user applications',
                                   'Frame count does not verify decoded visual counters'])
        output.write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps({key: value for key, value in result.items()
                          if key not in ('frames', 'source_draws')}), flush=True)
    finally:
        bridge.close()
