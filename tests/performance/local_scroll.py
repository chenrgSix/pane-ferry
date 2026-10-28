#!/usr/bin/env python3
"""Measure publisher frame delivery on a private Xvfb, without a network or user apps."""
import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import socket
import statistics
import subprocess
import tempfile
import time


def distribution(values):
    values = sorted(values)
    return dict(p50=statistics.median(values),
                p95=values[math.ceil(len(values) * .95) - 1], max=max(values))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--agent', required=True, type=Path)
    parser.add_argument('--width', type=int, default=2940)
    parser.add_argument('--height', type=int, default=1618)
    parser.add_argument('--fps', type=int, choices=(30, 60, 90), default=60)
    parser.add_argument('--seconds', type=float, default=10)
    parser.add_argument('--output', type=Path)
    args = parser.parse_args()
    if not (1 <= args.width <= 4096 and 1 <= args.height <= 4096
            and args.width * args.height <= 8_000_000
            and math.isfinite(args.seconds) and args.seconds >= 2):
        parser.error('dimensions must fit the protocol; --seconds must be at least 2')
    agent_path = args.agent.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix='paneferry-scroll-') as folder:
        home = Path(folder)
        config = home / '.config/lan-remoteapp'
        config.mkdir(parents=True)
        draws_path = home / 'draws.jsonl'
        fixture = Path(__file__).with_name('scroll_fixture.py').resolve()
        (config / 'apps.json').write_text(json.dumps([dict(
            id='scroll-benchmark', name='Scroll benchmark', executable='/usr/bin/python3',
            argv=[str(fixture), str(draws_path), str(args.width), str(args.height)])]))
        env = dict(os.environ, HOME=str(home), GDK_BACKEND='x11', GTK_IM_MODULE='simple',
                   XMODIFIERS='@im=none', PANEFERRY_VIDEO_MAX_FPS=str(args.fps))
        for key in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER',
                    'XDG_CONFIG_HOME', 'XDG_STATE_HOME', 'XDG_DATA_HOME'):
            env.pop(key, None)
        with (home / 'agent.log').open('w+') as log:
            agent = subprocess.Popen([str(agent_path), 'serve'], env=env,
                                     stdin=subprocess.DEVNULL, stdout=log, stderr=log)
            try:
                path = home / '.local/state/lan-remoteapp/agent.sock'
                deadline = time.monotonic() + 15
                while not path.exists():
                    if agent.poll() is not None or time.monotonic() > deadline:
                        raise RuntimeError('private agent did not start')
                    time.sleep(.02)
                with socket.socket(socket.AF_UNIX) as peer:
                    peer.settimeout(20)
                    peer.connect(str(path))
                    with peer.makefile('rb', buffering=65536) as wire:
                        def send(**message):
                            peer.sendall((json.dumps(dict(message, version=1)) + '\n').encode())

                        def receive():
                            raw = wire.readline(16 * 1024 * 1024 + 1)
                            arrived = time.monotonic_ns()
                            if not raw.endswith(b'\n'):
                                raise RuntimeError('invalid or truncated event')
                            event = json.loads(raw)
                            if event['type'] == 'error':
                                raise RuntimeError(event)
                            return arrived, event, len(raw)

                        def until(kind):
                            until_ns = time.monotonic_ns() + 20_000_000_000
                            while time.monotonic_ns() < until_ns:
                                _, event, _ = receive()
                                if event['type'] == kind:
                                    return event
                            raise RuntimeError('missing ' + kind)

                        send(type='hello')
                        until('hello')
                        send(type='video.enable', codec='h264', max_fps=args.fps)
                        ready = until('video.ready')
                        assert ready['max_fps'] == args.fps, ready
                        send(type='app.launch', app_id='scroll-benchmark',
                             request_id='scroll', backend='native')
                        until('session.started')
                        window = until('window.created')
                        assert (window['width'], window['height']) == (args.width, args.height), window
                        window_id = window['window_id']
                        until('frame')
                        start = time.monotonic_ns() + 2_000_000_000
                        end = start + int(args.seconds * 1e9)
                        frames = []
                        cpu_start = None
                        def cpu_ticks():
                            fields = Path(f'/proc/{agent.pid}/stat').read_text().rsplit(')', 1)[1].split()
                            return int(fields[11]) + int(fields[12])
                        while True:
                            arrived, event, size = receive()
                            if arrived >= end:
                                cpu_end = cpu_ticks()
                                break
                            if event['type'] != 'frame' or arrived < start:
                                continue
                            if cpu_start is None:
                                cpu_start = (cpu_ticks(), arrived)
                            assert event['window_id'] == window_id and event['codec'] == 'h264', event['type']
                            assert (event['width'], event['height']) == (args.width, args.height)
                            frames.append(dict(ns=arrived, sequence=event['seq'], json_bytes=size,
                                               key_frame=event['key_frame']))
                assert len(frames) >= 2, 'no continuous video'
                draws = [json.loads(line) for line in draws_path.read_text().splitlines()]
                draws = [row for row in draws if start <= row['ns'] < end]
                assert len(draws) >= 2 and all((row['width'], row['height']) == (args.width, args.height) for row in draws)
                gaps = [(b['ns'] - a['ns']) / 1e6 for a, b in zip(frames, frames[1:])]
                result = dict(schema=1, capture='private Xvfb + MIT-SHM; local Unix socket',
                              agent_sha256=hashlib.sha256(agent_path.read_bytes()).hexdigest(),
                              width=args.width, height=args.height, cap_fps=args.fps,
                              seconds=args.seconds, source_draw_fps=len(draws) / args.seconds,
                              received_fps=len(frames) / args.seconds,
                              agent_cpu_one_core_percent=(cpu_end - cpu_start[0]) / os.sysconf('SC_CLK_TCK')
                                  / ((arrived - cpu_start[1]) / 1e9) * 100,
                              frame_gap_ms=distribution(gaps),
                              gaps_over_1_5_periods=sum(gap > 1500 / args.fps for gap in gaps),
                              frames=frames, source_draws=draws,
                              limitations=['Publisher delivery only; no QUIC, client decode or display',
                                           'Xvfb does not measure GNOME/PipeWire/GPU readback',
                                           'Frame count does not verify decoded visual counters'])
                if args.output:
                    args.output.write_text(json.dumps(result, indent=2) + '\n')
                print(json.dumps({key: value for key, value in result.items()
                                  if key not in ('frames', 'source_draws')}, ensure_ascii=False))
            except Exception:
                log.seek(0)
                print(log.read()[-4000:])
                raise
            finally:
                agent.terminate()
                agent.wait(timeout=10)


if __name__ == '__main__':
    main()
