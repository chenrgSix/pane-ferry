#!/usr/bin/env python3
"""Native bounded decode scheduling and cancellation without touching user apps."""
from pathlib import Path
import subprocess
import tempfile
root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-async-video-') as directory:
    scratch = Path(directory)
    binary = scratch / 'decode-smoke'
    subprocess.run(['swiftc', '-module-cache-path', str(scratch / 'modules'),
                    str(source / 'VideoFrame.swift'), str(source / 'VideoTiming.swift'), str(source / 'ChromaAtlas.swift'), str(source / 'H264Decoder.swift'), str(source / 'VideoDecodeQueue.swift'),
                    str(root / 'tests/swift/AsyncVideoDecodeSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    subprocess.run([str(binary)], check=True, timeout=15)
