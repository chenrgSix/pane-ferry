#!/usr/bin/env python3
"""macOS half of video_stream_smoke.py: decode its exported real Linux H.264 packets."""
import argparse
from pathlib import Path
import subprocess
import tempfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('packets', type=Path)
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='paneferry-video-swift-', dir='/tmp') as temporary:
    scratch = Path(temporary)
    binary = scratch / 'video-decoder-smoke'
    subprocess.run(['swiftc', '-O', '-module-cache-path', str(scratch / 'modules'),
        str(root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost/VideoFrame.swift'), str(root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost/H264Decoder.swift'),
        str(root / 'tests/swift/H264DecoderSmoke.swift'), '-o', str(binary)], check=True, timeout=180)
    subprocess.run([str(binary), str(args.packets.resolve())], check=True, timeout=60)
