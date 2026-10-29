"""Continuous publisher measurement inside wayland_smoke's private GNOME only."""
import base64
import json
import math
import os
from pathlib import Path
import subprocess
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
            if (frame['width'], frame['height']) == (width, height) and frame['key_frame']:
                break
            assert time.monotonic()<deadline, 'resize timeout'
        warmup=float(os.environ.get('PANEFERRY_SCROLL_WARMUP_SECONDS','2'))
        assert math.isfinite(warmup) and 1<=warmup<=30
        start = time.monotonic_ns()+int(warmup*1e9)
        end = start+int(seconds*1e9)
        frames = []
        stream = [frame['data']]
        stream_rows = [dict(ns=time.monotonic_ns(), sequence=frame['seq'])]
        stream_id = frame['stream_id']
        streams=[dict(stream_id=stream_id,sequence=frame['seq'])]
        while True:
            raw = peer.file.readline(16*1024*1024+1)
            arrived = time.monotonic_ns()
            assert raw.endswith(b'\n')
            event = json.loads(raw)
            assert event['type'] != 'error', event
            if arrived >= end:
                break
            if event['type'] != 'frame':
                continue
            assert event['window_id'] == xid and event['codec'] == 'h264'
            assert (event['width'], event['height']) == (width, height)
            if event['stream_id']!=stream_id:
                assert os.environ.get('PANEFERRY_SCROLL_ALLOW_CODEC_SWITCH')=='1', 'unexpected encoder stream restart'
                assert event['key_frame'], 'a codec switch must start an IDR'
                stream_id=event['stream_id']
                streams.append(dict(stream_id=stream_id,sequence=event['seq']))
            stream.append(event['data'])
            stream_rows.append(dict(ns=arrived, sequence=event['seq']))
            if arrived < start:
                continue
            frames.append(dict(ns=arrived, sequence=event['seq'], json_bytes=len(raw), key_frame=event['key_frame']))
        all_draws = [json.loads(line) for line in draws_path.read_text().splitlines()]
        draws = [row for row in all_draws if start <= row['ns'] < end]
        assert len(frames)>=2 and len(draws)>=2
        assert all((row['width'], row['height']) == (width, height) for row in draws)
        gaps = [(b['ns']-a['ns'])/1e6 for a,b in zip(frames, frames[1:])]
        # Decode only after the measured interval. Retain encoded base64 during
        # receive so CPU decoding cannot slow capture/publication samples.
        encoded=b''.join(base64.b64decode(packet) for packet in stream)
        if len(streams)>1:
            pixels=subprocess.run(['ffmpeg','-v','error','-f','h264','-i','pipe:0',
                '-vf','crop=256:32:0:0,scale=8:1:flags=neighbor,format=gray',
                '-fps_mode','passthrough','-f','rawvideo','-pix_fmt','gray','pipe:1'],
                input=encoded,capture_output=True,check=True).stdout
            assert len(pixels)==len(stream_rows)*8
            markers=bytes(sum((value>128)<<bit for bit,value in enumerate(pixels[n:n+8])) for n in range(0,len(pixels),8))
        else:
            markers=subprocess.run(['/usr/bin/python3', str(Path(__file__).with_name('dmabuf_decode_markers.py')),
                                str(width),str(height),str(len(stream))],
                               input=encoded,
                               stdout=subprocess.PIPE,stderr=subprocess.PIPE,check=True).stdout
        assert len(markers)==len(stream_rows)
        visual=[]
        invalid=[]
        for marker,row in zip(markers,stream_rows):
            candidates=[draw for draw in all_draws if draw['ns']<=row['ns'] and draw['sequence']%256==marker
                        and (draw['width'],draw['height'])==(width,height)]
            if not candidates:
                invalid.append(dict(sequence=row['sequence'],marker=marker,ns=row['ns'],measured=row['ns']>=start))
                continue
            source=candidates[-1]
            visual.append(dict(sequence=row['sequence'],source_sequence=source['sequence'],
                               source_age_ms=(row['ns']-source['ns'])/1e6))
        measured_sequences={f['sequence'] for f in frames}
        measured_visual=[row for row in visual if row['sequence'] in measured_sequences]
        check=dict(decoded_frames=len(markers), measured_frames=len(measured_visual),
                   streams=streams,decoder='ffmpeg' if len(streams)>1 else 'openh264',
                   ordered=all(b['source_sequence']>=a['source_sequence'] for a,b in zip(measured_visual,measured_visual[1:])),
                   invalid=invalid, frames=measured_visual,
                   source_age_ms=distribution([row['source_age_ms'] for row in measured_visual]) if measured_visual else None)
        # The initial resized surface may be published before GTK paints its
        # first counter. Keep those warmup observations; validate every timed
        # frame strictly and save evidence before reporting a failure.
        output.with_suffix('.visual.json').write_text(json.dumps(check,indent=2)+'\n')
        assert len(measured_visual)==len(frames), 'measured marker does not name a source frame'
        assert check['ordered'], 'decoded frames moved backwards'
        result = dict(schema=1, capture='private GNOME XWayland + XComposite; local Unix socket',
                      width=width, height=height, cap_fps=60, seconds=seconds,warmup_seconds=warmup,
                      received_fps=len(frames)/seconds, source_draw_fps=len(draws)/seconds,
                      frame_gap_ms=distribution(gaps), frames=frames, source_draws=draws,
                      visual_counter=check,
                      path_decisions=[line for line in (folder/'fixture.log').read_text(errors='replace').splitlines()
                                      if line.startswith(('video path comparison:','video path: GPU'))],
                      limitations=['Private compositor and test-owned window; not a user application',
                                   'No QUIC or remote display; local decode runs after timing'])
        output.write_text(json.dumps(result, indent=2)+'\n')
        print(json.dumps({k:v for k,v in result.items() if k not in ('frames','source_draws','visual_counter')}), flush=True)
    finally:
        if not output.exists():
            output.with_suffix('.failed.json').write_text(json.dumps(dict(source_draws=[json.loads(line) for line in draws_path.read_text().splitlines()]), indent=2)+'\n')
        bridge.close()
