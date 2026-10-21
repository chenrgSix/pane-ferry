#!/usr/bin/env python3
"""Actual AppDelegate/SSHTransport with a private subprocess fixture, no user SSH.

The fixture terminates one transport then accepts a new one. Check automatic
resume, unchanged NSWindow, stream sequence reset and dropped offline input.
"""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/RemoteAppWindowHost'
with tempfile.TemporaryDirectory(prefix='remoteapp-auto-resume-') as directory:
    scratch = Path(directory)
    types = scratch/'WindowHostTypes.swift'
    text = (source/'main.swift').read_text()
    marker = '\n// WINDOWHOST_ENTRYPOINT\n'
    assert text.count(marker) == 1
    types.write_text(text.split(marker)[0])
    fixture = scratch/'fake-ssh'
    fixture.write_text('''#!/usr/bin/env python3
import json, pathlib, select, sys
root=pathlib.Path(__file__).parent
count=root/'connections'
n=int(count.read_text())+1 if count.exists() else 1
count.write_text(str(n))
def emit(**value):
    print(json.dumps(dict(value, version=1)), flush=True)
with (root/'requests').open('a', buffering=1) as log:
    while not (n==1 and (root/'cut').exists()):
        if not select.select([sys.stdin],[],[],.02)[0]: continue
        line=sys.stdin.readline()
        if not line: break
        item=json.loads(line); log.write(json.dumps(item)+'\\n')
        if item['type']=='hello': emit(type='hello', capabilities=['input.text.commit'])
        if item['type']=='session.resume':
            emit(type='session.snapshot', session_id='retained', seq=100 if n==1 else 1,
                 windows=[dict(window_id=42, title='Automatic reconnect fixture', width=640, height=480,
                               geometry_revision=n, scale_factor=1, visible=True, role='toplevel')])
            emit(type='input.authorization', session_id='retained', seq=101 if n==1 else 2, state='pending')
            emit(type='input.authorization', session_id='retained', seq=102 if n==1 else 3, state='ready')
        if item['type']=='session.detach': break
sys.exit(255 if n==1 else 0)
''')
    fixture.chmod(0o700)
    binary = scratch/'smoke'
    files = [str(path) for path in source.glob('*.swift') if path.name != 'main.swift']
    subprocess.run(['swiftc', '-module-cache-path', str(scratch/'modules'), *files, str(types),
                    str(root/'tests/swift/AutomaticReconnectSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    subprocess.run([str(binary), str(scratch)], check=True, timeout=25)
