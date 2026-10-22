#!/usr/bin/env python3
"""Opt-in clipboard bundle round trip to an already paired QUIC device.

Transfers generated fixture bytes only. Remote snapshots follow the publisher's
normal cache expiry; use an isolated publisher when running repeatedly.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile
root = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--peer', required=True)
parser.add_argument('--state', required=True, type=Path)
parser.add_argument('--network', required=True, type=Path)
args = parser.parse_args()
source = root/'apps/mac-windowhost/Sources/RemoteAppWindowHost'
with tempfile.TemporaryDirectory(prefix='lra-quic-bulk-') as directory:
    scratch = Path(directory)
    files = ['SurfaceView.swift', 'ClipboardBundle.swift', 'ClipboardBulkTransfer.swift', 'RenderGeometry.swift']
    binary = scratch/'bulk-test'
    subprocess.run(['swiftc','-module-cache-path',str(scratch/'modules'),
        *[str(source/f) for f in files],str(root/'tests/swift/ClipboardBulkQUICSmoke.swift'),'-o',str(binary)],check=True)
    subprocess.run([str(binary),args.peer,str(args.state.resolve()),str(args.network.resolve())],check=True,timeout=300)
