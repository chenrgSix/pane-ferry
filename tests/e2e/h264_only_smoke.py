#!/usr/bin/env python3
"""Mac host negotiates H.264 only and rejects obsolete quality selection."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-h264-only-') as directory:
    scratch = Path(directory)
    types = scratch / 'WindowHostTypes.swift'
    types.write_text((source / 'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    files = [str(path) for path in source.glob('*.swift') if path.name != 'main.swift']
    binary = scratch / 'smoke'
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'), *files, str(types),
                    str(root / 'tests/swift/H264OnlySmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    subprocess.run([str(binary)], check=True, timeout=20)
