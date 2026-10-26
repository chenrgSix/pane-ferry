#!/usr/bin/env python3
"""Actual AppDelegate/RWPTransport with a private subprocess fixture, no network or user state.

The fixture terminates one transport then accepts a new one. Check automatic
resume, unchanged NSWindow, stream sequence reset and dropped offline input.
"""
from pathlib import Path
import subprocess
import tempfile
import plistlib
import shutil

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-auto-resume-') as directory:
    scratch = Path(directory)
    types = scratch/'WindowHostTypes.swift'
    text = (source/'main.swift').read_text()
    marker = '\n// WINDOWHOST_ENTRYPOINT\n'
    assert text.count(marker) == 1
    types.write_text(text.split(marker)[0])
    fixture = scratch/'fake-bridge'
    fixture.write_text('''#!/usr/bin/env python3
import json, os, pathlib, select, sys
root=pathlib.Path(__file__).parent
count=root/'connections'
n=int(count.read_text())+1 if count.exists() else 1
count.write_text(str(n))
def emit(**value):
    print(json.dumps(dict(value, version=1)), flush=True)
with (root/'requests').open('a', buffering=1) as log:
    pending=b''; detach=False
    while not detach and not (n==1 and (root/'cut').exists()):
        if select.select([0],[],[],.02)[0]:
            chunk=os.read(0,65536)
            if not chunk: break
            pending+=chunk
        while b'\\n' in pending:
            line,pending=pending.split(b'\\n',1)
            if not line: continue
            item=json.loads(line); log.write(json.dumps(item)+'\\n')
            if item['type']=='hello': emit(type='hello', capabilities=['input.text.commit','native.h264','video.qos'], video_max_fps=60)
            if item['type']=='video.enable': emit(type='video.ready', codec='h264')
            if item['type']=='session.resume':
                emit(type='session.snapshot', session_id='retained', seq=100 if n==1 else 1,
                     windows=[dict(window_id=42, title='Automatic reconnect fixture', width=640, height=480,
                                   geometry_revision=n, scale_factor=1, visible=True, role='toplevel')])
                emit(type='input.authorization', session_id='retained', seq=101 if n==1 else 2, state='pending')
                emit(type='input.authorization', session_id='retained', seq=102 if n==1 else 3, state='ready')
            if item['type']=='session.detach': detach=True; break
sys.exit(255 if n==1 else 0)
''')
    fixture.chmod(0o700)
    binary = scratch/'smoke'
    files = [str(path) for path in source.glob('*.swift') if path.name != 'main.swift']
    subprocess.run(['swiftc', '-module-cache-path', str(scratch/'modules'), *files, str(types),
                    str(root/'tests/swift/AutomaticReconnectSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    # LaunchServices grants this AppKit fixture normal foreground activation.
    # A bare subprocess may remain inactive on macOS even after activate(),
    # making the key-window/menu assertions depend on the test runner's focus.
    bundle = scratch/'ReconnectFixture.app'
    executable = bundle/'Contents/MacOS/smoke'
    executable.parent.mkdir(parents=True)
    shutil.copy2(binary, executable)
    (bundle/'Contents/Info.plist').write_bytes(plistlib.dumps({
        'CFBundleExecutable': 'smoke', 'CFBundleIdentifier': 'io.paneferry.tests.reconnect',
        'CFBundlePackageType': 'APPL', 'CFBundleName': 'PaneFerry reconnect test'}))
    stdout, stderr = scratch/'stdout', scratch/'stderr'
    result = subprocess.run(['open', '-W', '-n', '--stdout', str(stdout), '--stderr', str(stderr),
                             str(bundle), '--args', str(scratch)], timeout=25)
    output = stdout.read_text() if stdout.exists() else ''
    errors = stderr.read_text() if stderr.exists() else ''
    passed = 'PASS: shared Ubuntu authorization state' in output
    if result.returncode or not passed:
        print(output, errors[-4000:], flush=True)
        print((scratch/'requests').read_text()[-4000:] if (scratch/'requests').exists() else 'no requests', flush=True)
    assert result.returncode == 0 and passed, (result.returncode, 'fixture did not complete')
    print(output, end='')
