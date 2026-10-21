#!/usr/bin/env python3
"""Linux-only isolated Agent regression: dead existing PID and expired retry IDs."""
import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='remoteapp-stale-') as temporary:
    home = Path(temporary)
    config = home / '.config/lan-remoteapp'
    config.mkdir(parents=True)
    (config / 'apps.json').write_text(json.dumps([
        dict(id='stale', name='Stale', executable='/usr/bin/true',
             existing=dict(display=0, xauthority=str(home / 'unused'), process_id=2147483647)),
        dict(id='exits', name='Exits', executable='/usr/bin/true'),
    ]))
    process = subprocess.Popen([str(args.agent.resolve()), 'serve'], env=dict(os.environ, HOME=str(home)),
                               stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        path = home / '.local/state/lan-remoteapp/agent.sock'
        deadline = time.monotonic() + 10
        while not path.exists():
            assert time.monotonic() < deadline
            time.sleep(.02)
        with socket.socket(socket.AF_UNIX) as connection:
            connection.settimeout(10)
            connection.connect(str(path))
            with connection.makefile('rwb', buffering=0) as wire:
                def request(**data):
                    wire.write((json.dumps(dict(data, version=1)) + '\n').encode())
                    return json.loads(wire.readline())
                assert request(type='hello')['type'] == 'hello'
                apps = request(type='app.list')['apps']
                assert apps[0]['unavailable_reason'] == 'existing_process_unavailable'
                assert apps[1]['unavailable_reason'] is None
                result = request(type='app.launch', app_id='stale', request_id='stale', backend='native', attach=False)
                assert result['code'] == 'existing_process_unavailable', result
                first = request(type='app.launch', app_id='exits', request_id='first', backend='native', attach=False)
                assert first['type'] == 'session.started', first
                deadline = time.monotonic() + 10
                while request(type='session.list')['sessions']:
                    assert time.monotonic() < deadline
                    time.sleep(.02)
                expired = request(type='app.launch', app_id='exits', request_id='first', backend='native', attach=False)
                assert expired['code'] == 'session_exited', expired
                second = request(type='app.launch', app_id='exits', request_id='second', backend='native', attach=False)
                assert second['type'] == 'session.started' and second['session_id'] != first['session_id'], second
                assert request(type='app.launch', app_id='exits', request_id='first', backend='native', attach=False)['code'] == 'session_expired'
        print('PASS: unavailable catalog target, explicit launch error, expired ID rejection, fresh launch ID')
    finally:
        process.terminate()
        process.wait(timeout=10)
