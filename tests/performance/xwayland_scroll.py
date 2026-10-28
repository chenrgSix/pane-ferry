"""Continuous publisher measurement inside wayland_smoke's private GNOME only."""
import json
from pathlib import Path
import time

from local_scroll import distribution


def verify(peer, env, folder, log, start_child, wait, Bridge, output):
    width, height, seconds = 2940, 1618, 8
    fixture = Path(__file__).with_name('scroll_fixture.py').resolve()
    draws_path = folder/'x11-scroll-draws.jsonl'
    app = start_child(['/usr/bin/python3', str(fixture), str(draws_path), str(width), str(height),
                       '--undecorated', '--small-start'], dict(env, GDK_BACKEND='x11', GDK_SCALE='1'), log)
    wait(draws_path.exists)
    bridge = Bridge(env)
    try:
        def target():
            return next((w for w in bridge.call(op='list').get('result', []) if w['pid'] == app.pid), None)
        xid = wait(target)['xwindow']
        assert 'result' in bridge.call(op='test_reveal_scroll', pid=app.pid)
        row = wait(lambda: next((row for row in peer.request('window.list')['windows'] if row['window_id'] == xid), None))
        peer.request('app.launch', 'session.started', app_id=row['id'], request_id='x11-scroll', backend='native')
        peer.send(type='window.configure', window_id=xid, width=width, height=height)
        deadline = time.monotonic()+20
        while True:
            frame = peer.until('frame')
            if (frame['width'], frame['height']) == (width, height):
                break
            assert time.monotonic()<deadline, 'resize timeout'
        start = time.monotonic_ns()+2_000_000_000
        end = start+int(seconds*1e9)
        frames = []
        while True:
            raw = peer.file.readline(16*1024*1024+1)
            arrived = time.monotonic_ns()
            assert raw.endswith(b'\n')
            event = json.loads(raw)
            assert event['type'] != 'error', event
            if arrived >= end:
                break
            if event['type'] != 'frame' or arrived < start:
                continue
            assert event['window_id'] == xid and event['codec'] == 'h264'
            assert (event['width'], event['height']) == (width, height)
            frames.append(dict(ns=arrived, sequence=event['seq'], json_bytes=len(raw), key_frame=event['key_frame']))
        draws = [json.loads(line) for line in draws_path.read_text().splitlines()]
        draws = [row for row in draws if start <= row['ns'] < end]
        assert len(frames)>=2 and len(draws)>=2
        assert all((row['width'], row['height']) == (width, height) for row in draws)
        gaps = [(b['ns']-a['ns'])/1e6 for a,b in zip(frames, frames[1:])]
        result = dict(schema=1, capture='private GNOME XWayland + XComposite; local Unix socket',
                      width=width, height=height, cap_fps=60, seconds=seconds,
                      received_fps=len(frames)/seconds, source_draw_fps=len(draws)/seconds,
                      frame_gap_ms=distribution(gaps), frames=frames, source_draws=draws,
                      limitations=['Private compositor and test-owned window; not a user application',
                                   'No QUIC, client decode or display; no decoded visual counter check'])
        output.write_text(json.dumps(result, indent=2)+'\n')
        print(json.dumps({k:v for k,v in result.items() if k not in ('frames','source_draws')}), flush=True)
    finally:
        if not output.exists():
            output.with_suffix('.failed.json').write_text(json.dumps(dict(source_draws=[json.loads(line) for line in draws_path.read_text().splitlines()]), indent=2)+'\n')
        bridge.close()
