#!/usr/bin/env python3
"""Validate the real latency probe's markers and endpoint without remote input."""
from pathlib import Path
import os
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-live-probe-unit-') as directory:
    scratch = Path(directory)
    types = scratch / 'Types.swift'
    types.write_text((source / 'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch / 'smoke'
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'),
        *[str(p) for p in source.glob('*.swift') if p.name != 'main.swift'], str(types),
        str(root / 'tests/performance/LiveProbe.swift'), str(root / 'tests/swift/LiveProbeSmoke.swift'),
        '-o', str(binary)], check=True, timeout=120)
    subprocess.run([str(binary)], check=True, timeout=15,
        env=dict(os.environ, PROBE_OUTPUT=str(scratch / 'result.json'), PROBE_WIDTH='1470', PROBE_HEIGHT='809'))
