#!/usr/bin/env python3
"""Exercise the real login helper's recovery flow with an isolated SSH fixture."""
from pathlib import Path
import json
import os
import shlex
import subprocess
import tempfile
import time

source = Path(__file__).resolve().parents[2] / 'scripts/connect-ssh.sh'
with tempfile.TemporaryDirectory(prefix='remoteapp ssh recovery ') as directory:
    root = Path(directory)
    fixture = root / 'ssh-fixture'
    fixture.write_text('''#!/usr/bin/env python3
import json, os, pathlib, sys, time
args=sys.argv[1:]; path=pathlib.Path(os.environ['REMOTEAPP_TEST_SSH_STATE'])
state=json.loads(path.read_text())
with open(os.environ['REMOTEAPP_TEST_SSH_LOG'],'a') as log:log.write(json.dumps(args)+'\\n')
if '-O' in args:
 action=args[args.index('-O')+1]
 if action=='check':sys.exit(0 if state['master'] else 255)
 if action=='exit':
  state['master']=False;path.write_text(json.dumps(state));sys.exit(0)
if args[-1]=='/usr/bin/true':
 if state.get('stalled'):time.sleep(60)
 sys.exit(0 if state['healthy'] else 255)
if '-N' in args:
 assert '-oControlMaster=auto' in args
 assert '-oServerAliveInterval=10' in args and '-oServerAliveCountMax=2' in args
 assert '-oStrictHostKeyChecking=yes' in args and '-oForwardAgent=no' in args
 state.update(master=True,healthy=True);path.write_text(json.dumps(state));sys.exit(0)
sys.exit(2)
''')
    fixture.chmod(0o700)
    helper = root / 'connect.sh'
    text = source.read_text().replace('/usr/bin/ssh', shlex.quote(str(fixture)))
    assert text.count('sleep 10') == 1
    helper.write_text(text.replace('sleep 10', 'sleep 0.2'))
    test_home = root / 'home'
    test_home.mkdir()
    state_file, log_file = root / 'state.json', root / 'calls.jsonl'
    environment = {**os.environ, 'HOME': str(test_home),
                   'REMOTEAPP_TEST_SSH_STATE': str(state_file),
                   'REMOTEAPP_TEST_SSH_LOG': str(log_file)}

    def run(state, action=None):
        state_file.write_text(json.dumps(state))
        log_file.write_text('')
        args = ['bash', str(helper)] + ([action] if action else []) + ['ubuntu.test', 'alice', '22']
        result = subprocess.run(args, env=environment, capture_output=True, text=True, timeout=4)
        calls = [json.loads(line) for line in log_file.read_text().splitlines()]
        return result, calls

    def operations(calls):
        return [args[args.index('-O') + 1] if '-O' in args else 'probe' if args[-1] == '/usr/bin/true' else 'login' for args in calls]

    result, calls = run(dict(master=True, healthy=True))
    assert result.returncode == 0 and operations(calls) == ['check', 'probe'], (result, calls)
    result, calls = run(dict(master=False, healthy=False))
    assert result.returncode == 0 and operations(calls) == ['check', 'login'], (result, calls)
    result, calls = run(dict(master=True, healthy=False))
    assert result.returncode == 0 and operations(calls) == ['check', 'probe', 'exit', 'login'], (result, calls)
    result, calls = run(dict(master=True, healthy=False), '--check')
    assert result.returncode == 1 and operations(calls) == ['check', 'probe'], (result, calls)
    result, calls = run(dict(master=False, healthy=False), '--check')
    assert result.returncode == 1 and operations(calls) == ['check'], (result, calls)
    result, calls = run(dict(master=True, healthy=True), '--disconnect')
    assert result.returncode == 0 and operations(calls) == ['exit'], (result, calls)
    started = time.monotonic()
    result, calls = run(dict(master=True, healthy=False, stalled=True))
    assert result.returncode == 0 and operations(calls) == ['check', 'probe', 'exit', 'login'], (result, calls)
    assert time.monotonic() - started < 3
    print('PASS: healthy login reused; missing/stale transport recovered; stalled probe bounded; read-only check never resets a login')
