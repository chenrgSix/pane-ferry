#!/usr/bin/env python3
"""Exercise the actual pipe reader without requiring the full Xcode/XCTest SDK."""
import pathlib
import subprocess
import tempfile

root = pathlib.Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix="paneferry-swift-", dir="/tmp") as temporary:
    scratch = pathlib.Path(temporary)
    binary = scratch / "pipe-reader-smoke"
    subprocess.run([
        "swiftc", "-module-cache-path", str(scratch / "modules"),
        str(root / "apps/mac-windowhost/Sources/PaneFerryWindowHost/PipeReader.swift"),
        str(root / "tests/swift/PipeReaderSmoke.swift"), "-o", str(binary),
    ], check=True, timeout=120)
    subprocess.run([str(binary)], check=True, timeout=10)
