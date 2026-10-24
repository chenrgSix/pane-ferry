#!/usr/bin/env python3
"""Opt-in real AppKit/QUIC ChatGPT test. Pass normal WindowHost --session/--native-* options.

Requires an explicitly selected, prepared ChatGPT window on a paired test Agent.
Opening the host uses normal automatic resizing; the test toggles page density,
reconnects, then detaches. It never closes the remote app or sends chat messages.
"""
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/PaneFerryWindowHost'
if '--session' not in sys.argv or '--app' in sys.argv:
    raise SystemExit('Pass --session ID --native-peer UUID --native-state DIR --native-helper PATH')
with tempfile.TemporaryDirectory(prefix='paneferry-page-scale-') as directory:
    scratch = Path(directory)
    types = scratch/'WindowHostTypes.swift'
    types.write_text((source/'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch/'smoke'
    files = [str(p) for p in source.glob('*.swift') if p.name != 'main.swift']
    subprocess.run(['swiftc', '-module-cache-path', str(scratch/'modules'), *files, str(types),
                    str(root/'tests/swift/LivePageScaleSmoke.swift'), '-o', str(binary)], check=True, timeout=180)
    subprocess.run([str(binary), *sys.argv[1:]], check=True, timeout=180)
