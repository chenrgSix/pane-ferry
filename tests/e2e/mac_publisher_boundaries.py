#!/usr/bin/env python3
"""Compile production boundary logic without XCTest or touching desktop contents."""
from pathlib import Path
import subprocess
import tempfile
root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-node/Sources/PaneFerryNode'
with tempfile.TemporaryDirectory(prefix='paneferry-publisher-boundaries-') as temporary:
    output = str(Path(temporary) / 'boundaries')
    subprocess.run(['swiftc', '-module-cache-path', temporary + '/cache',
        *[str(source / name) for name in ('Wire.swift', 'ProcessIdentity.swift', 'Publisher.swift', 'Clipboard.swift', 'VideoRateController.swift')],
        str(root / 'tests/swift/MacPublisherBoundarySmoke.swift'), '-o', output], check=True, timeout=120)
    subprocess.run([output], check=True, timeout=30)
