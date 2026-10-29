#!/usr/bin/env python3
"""Verify the actual AppKit text client and clipboard reply gate without changing the user's clipboard."""
import pathlib
import subprocess
import tempfile

root = pathlib.Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-input-', dir='/tmp') as directory:
    scratch = pathlib.Path(directory)
    binary = scratch / 'controller-input-smoke'
    types = scratch / 'WindowHostTypes.swift'
    types.write_text((source / 'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    files = [str(path) for path in sorted(source.glob('*.swift')) if path.name != 'main.swift']
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'),
                    *files, str(types),
                    str(root / 'tests/swift/ClipboardBundleSmoke.swift'),
                    str(root / 'tests/swift/ControllerInputSmoke.swift'), '-o', str(binary)],
                   check=True, timeout=120)
    subprocess.run([str(binary)], check=True, timeout=15)
