#!/usr/bin/env python3
"""Exercise real AppKit multi-session isolation with private RWP publishers."""
from pathlib import Path
import json
import os
import subprocess
import tempfile
import uuid

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='pf-group-', dir='/tmp') as directory:
    scratch = Path(directory)
    group = uuid.uuid4().hex + uuid.uuid4().hex
    broker_dir = Path(f'/tmp/pf-app-{os.geteuid()}')
    broker_dir.mkdir(mode=0o700, exist_ok=True)
    (scratch/'bridge').write_bytes((root/'tests/fixtures/group_publisher.py').read_bytes())
    (scratch/'bridge').chmod(0o700)
    (scratch/'video.json').write_bytes((root/'tests/fixtures/native-video.json').read_bytes())
    types = scratch/'Types.swift'
    types.write_text((source/'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch/'GroupSmoke'
    subprocess.run(['swiftc', '-module-cache-path', '/tmp/paneferry-appgroup-test-modules',
        *map(str, sorted(p for p in source.glob('*.swift') if p.name != 'main.swift')),
        str(types), str(root/'tests/swift/ApplicationGroupSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    result = subprocess.run([str(binary), str(scratch), group], capture_output=True, text=True, timeout=45)
    print(result.stdout, end='')
    assert result.returncode == 0, (result.returncode, result.stderr[-6000:])
    requests = [json.loads(line) for line in (scratch/'requests').read_text().splitlines()]
    for session in ('one', 'two', 'three'):
        assert any(r.get('fixture_session') == session and r.get('text') == f'text-{session}' for r in requests)
        assert not any(r.get('fixture_session') != session and r.get('text') == f'text-{session}' for r in requests)
    assert any(r.get('fixture_session') == 'two' and r.get('text') == 'after-close' for r in requests)
    assert all(r.get('type') not in ('app.launch', 'app.stop', 'window.close') for r in requests)
    print('PASS: bridge logs confirm isolated input routes and detach-only close semantics')
    (broker_dir/f'{group}.sock').unlink(missing_ok=True)
