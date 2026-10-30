#!/usr/bin/env python3
"""Decode a synthetic Windows full-chroma packet on macOS; no user applications."""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-chroma-') as directory:
    scratch = Path(directory)
    binary = scratch / 'chroma-smoke'
    subprocess.run(['swiftc', '-O', '-module-cache-path', str(scratch / 'modules'),
                    *[str(source / name) for name in ('VideoFrame.swift', 'ChromaAtlas.swift', 'H264Decoder.swift')],
                    str(root / 'tests/swift/ChromaAtlasSmoke.swift'), '-o', str(binary)], check=True, timeout=180)
    subprocess.run([str(binary), str(root / 'tests/fixtures/windows-444-atlas.json')], check=True, timeout=30)
