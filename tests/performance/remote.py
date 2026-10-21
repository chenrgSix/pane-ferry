#!/usr/bin/env python3
"""Run an isolated Agent until stdin closes, returning only test-owned timing logs."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', type=Path, required=True)
parser.add_argument('--home', type=Path, required=True)
parser.add_argument('--width', type=int, required=True)
parser.add_argument('--height', type=int, required=True)
parser.add_argument('--animated', action='store_true')
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
    try:
        deadline = time.monotonic() + 15
        while not (home / '.local/state/lan-remoteapp/agent.sock').exists():
            assert agent.poll() is None, 'test Agent exited'
            assert time.monotonic() < deadline, 'test Agent startup timeout'
            time.sleep(.02)
        print(json.dumps(dict(ready=True, pid=agent.pid)), flush=True)
        input()
    except EOFError:
        pass
    finally:
        agent.terminate()
        agent.wait(timeout=10)
    log.seek(0)
    lines = log.read().splitlines()
    probes = [json.loads(line.removeprefix('LATENCY ')) for line in lines if line.startswith('LATENCY ')]
    fixture = [json.loads(line) for line in fixture_log.read_text().splitlines()] if fixture_log.exists() else []
    print(json.dumps(dict(agent=probes, fixture=fixture,
                          errors=[line for line in lines if not line.startswith('LATENCY ')][-10:])), flush=True)
