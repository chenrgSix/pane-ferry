#!/usr/bin/env python3
"""Run an isolated Agent until stdin closes, returning only test-owned timing logs."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import socket
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', type=Path, required=True)
parser.add_argument('--home', type=Path, required=True)
parser.add_argument('--width', type=int, required=True)
parser.add_argument('--height', type=int, required=True)
parser.add_argument('--animated', action='store_true')
parser.add_argument('--network', type=Path, help='optional native QUIC binary for the isolated publisher')
args = parser.parse_args()
home = args.home.resolve()
config = home / '.config/lan-remoteapp'
config.mkdir(parents=True)
fixture_log = home / 'fixture.jsonl'
(config / 'apps.json').write_text(json.dumps([dict(
    id='latency', name='Latency measurement', executable='/usr/bin/python3',
    argv=[str(Path(__file__).with_name('fixture.py').resolve()), str(fixture_log),
          str(args.width), str(args.height), 'animated' if args.animated else 'idle'])]))
env = dict(os.environ, HOME=str(home), XMODIFIERS='@im=none', GTK_IM_MODULE='simple')
for name in ('WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER'):
    env.pop(name, None)
with (home / 'agent.log').open('w+') as log:
    agent = subprocess.Popen([str(args.agent.resolve()), 'serve'], env=env,
                             stdout=subprocess.DEVNULL, stderr=log)
    network = None
    network_stats = None
    try:
        deadline = time.monotonic() + 15
        while not (home / '.local/state/lan-remoteapp/agent.sock').exists():
            assert agent.poll() is None, 'test Agent exited'
            assert time.monotonic() < deadline, 'test Agent startup timeout'
            time.sleep(.02)
        ready = dict(ready=True, pid=agent.pid)
        if args.network:
            state = home / 'network'
            network = subprocess.Popen([str(args.network.resolve()), 'serve', '--state', str(state),
                '--publisher', str(home / '.local/state/lan-remoteapp/agent.sock'), '--listen', '0.0.0.0:0',
                '--name', 'Isolated timing publisher'], env=env, stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL, stderr=log)
            deadline = time.monotonic() + 10
            while not (state / 'control.sock').exists():
                assert network.poll() is None and time.monotonic() < deadline, 'test network startup failed'
                time.sleep(.02)
            for operation in ('status', 'invite'):
                with socket.socket(socket.AF_UNIX) as client:
                    client.settimeout(10); client.connect(str(state / 'control.sock'))
                    client.sendall(json.dumps(dict(op=operation)).encode() + b'\n')
                    value = json.loads(client.makefile('rb').readline(131073))
                    assert 'error' not in value, 'test network operation failed'
                    if operation == 'status':
                        ready['network_port'] = int(value['listen'].rsplit(':', 1)[1])
                    else:
                        ready['invitation'] = value['invitation']
        # The invitation crosses only the authenticated harness pipe. It is
        # consumed immediately and is never retained in the benchmark report.
        print(json.dumps(ready), flush=True)
        input()
        if network is not None:
            with socket.socket(socket.AF_UNIX) as client:
                client.settimeout(10); client.connect(str(state / 'control.sock'))
                client.sendall(b'{"op":"status"}\n')
                network_stats = json.loads(client.makefile('rb').readline(131073)).get('_probe_connections')
    except EOFError:
        pass
    finally:
        if network is not None:
            network.terminate(); network.wait(timeout=10)
        agent.terminate()
        agent.wait(timeout=10)
    log.seek(0)
    lines = log.read().splitlines()
    probes = [json.loads(line.removeprefix('LATENCY ')) for line in lines if line.startswith('LATENCY ')]
    fixture = [json.loads(line) for line in fixture_log.read_text().splitlines()] if fixture_log.exists() else []
    print(json.dumps(dict(agent=probes, fixture=fixture, network_stats=network_stats,
                          errors=[line for line in lines if not line.startswith('LATENCY ')][-10:])), flush=True)
