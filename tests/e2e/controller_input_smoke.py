#!/usr/bin/env python3
"""Verify the actual AppKit text client and clipboard reply gate without changing the user's clipboard."""
import pathlib
import subprocess
import tempfile

root = pathlib.Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/RemoteAppWindowHost'
with tempfile.TemporaryDirectory(prefix='remoteapp-input-', dir='/tmp') as directory:
    scratch = pathlib.Path(directory)
    binary = scratch / 'controller-input-smoke'
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'),
                    str(source / 'SurfaceView.swift'), str(source / 'ClipboardCopyGate.swift'), str(source / 'RenderGeometry.swift'),
                    str(source / 'HostedWindow.swift'),
                    str(source / 'ClipboardBundle.swift'), str(source / 'ClipboardBulkTransfer.swift'), str(source / 'SSHIdentity.swift'),
                    str(root / 'tests/swift/ClipboardBundleSmoke.swift'),
                    str(root / 'tests/swift/ControllerInputSmoke.swift'), '-o', str(binary)],
                   check=True, timeout=120)
    subprocess.run([str(binary)], check=True, timeout=15)
