#!/usr/bin/env python3
"""Verify the actual probe's visual detection without controlling a remote app."""
from pathlib import Path
import os
import subprocess
import tempfile
root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-scene-probe-unit-') as directory:
    scratch = Path(directory)
    main = scratch/'Types.swift'
    main.write_text((source/'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch/'smoke'
    subprocess.run(['swiftc','-module-cache-path',str(scratch/'modules'),
        *[str(p) for p in source.glob('*.swift') if p.name != 'main.swift'],str(main),
        str(root/'tests/performance/PopupProbe.swift'),str(root/'tests/swift/SceneProbeSmoke.swift'),
        '-o',str(binary)],check=True,timeout=120)
    subprocess.run([str(binary)],check=True,timeout=15,
        env=dict(os.environ,PROBE_OUTPUT=str(scratch/'result.json'),PROBE_COMMAND=str(scratch/'command.json')))
