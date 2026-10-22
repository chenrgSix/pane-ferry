#!/usr/bin/env python3
"""Opt-in real AppKit/QUIC test against a disposable paired GTK editor.

The publisher must have an explicitly configured test application running
clipboard_editor_fixture.py. Never use an existing personal document/application.
This runner does not deploy, pair, authenticate with SSH or alter the system clipboard.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--peer', required=True)
parser.add_argument('--state', type=Path, required=True)
parser.add_argument('--network', type=Path, required=True)
parser.add_argument('--test-app', required=True)
args = parser.parse_args()
source = root/'apps/mac-windowhost/Sources/RemoteAppWindowHost'
with tempfile.TemporaryDirectory(prefix='lra-live-quic-') as directory:
    scratch = Path(directory)
    types = scratch/'WindowHostTypes.swift'
    types.write_text((source/'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch/'receiver'
    subprocess.run(['swiftc', '-module-cache-path', str(scratch/'modules'),
        *[str(p) for p in source.glob('*.swift') if p.name != 'main.swift'], str(types),
        str(root/'tests/swift/LiveQUICSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    subprocess.run([str(binary), '--native-peer', args.peer, '--native-state', str(args.state.resolve()),
        '--native-helper', str(args.network.resolve()), '--app', args.test_app], check=True, timeout=120)
