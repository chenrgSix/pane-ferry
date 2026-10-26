#!/usr/bin/env python3
"""No network or user apps: quit a real AppKit process from its main queue."""
from pathlib import Path
import subprocess
import tempfile
root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-termination-') as folder:
    scratch = Path(folder)
    types = scratch / 'Types.swift'
    types.write_text((source / 'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch / 'TerminationSmoke'
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'),
        *map(str, sorted(p for p in source.glob('*.swift') if p.name != 'main.swift')),
        str(types), str(root / 'tests/swift/TerminationSmoke.swift'), '-o', str(binary)],
        check=True, timeout=120)
    for mode in ('session', 'quit'):
        result = subprocess.run([str(binary), mode], capture_output=True, text=True, timeout=6)
        assert result.returncode == 0 and 'PASS:' in result.stdout, (mode, result.stdout, result.stderr)
        print(mode, result.stdout.strip())
