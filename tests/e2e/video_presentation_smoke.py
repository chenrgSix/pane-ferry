#!/usr/bin/env python3
"""Private AppKit/CoreVideo surface, no network, user apps or clipboard access."""
from pathlib import Path
import plistlib
import subprocess
import tempfile
root = Path(__file__).resolve().parents[2]
source = root / 'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='paneferry-video-display-') as temporary:
    scratch = Path(temporary).resolve()
    bundle = scratch / 'VideoPresentation.app'
    binary = bundle / 'Contents/MacOS/presentation'
    binary.parent.mkdir(parents=True)
    names = ['VideoFrame.swift', 'SurfaceView.swift', 'ClipboardBundle.swift', 'ClipboardBulkTransfer.swift', 'RenderGeometry.swift']
    subprocess.run(['swiftc', '-g', '-module-cache-path', str(scratch/'modules'),
        *[str(source/name) for name in names], str(root/'tests/swift/VideoPresentationSmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    (bundle/'Contents/Info.plist').write_bytes(plistlib.dumps({'CFBundleExecutable': 'presentation',
        'CFBundleIdentifier': 'io.paneferry.tests.presentation', 'CFBundlePackageType': 'APPL'}))
    subprocess.run(['open', '-W', '-n', '--stdout', str(scratch/'stdout'), '--stderr', str(scratch/'stderr'), str(bundle)], check=True, timeout=20)
    output = (scratch/'stdout').read_text()
    errors = (scratch/'stderr').read_text()
    if 'PASS: retained CoreVideo surface actually displayed' not in output:
        import shutil
        shutil.copytree(scratch, Path('/tmp/paneferry-video-display-failure'), dirs_exist_ok=True)
        raise AssertionError(output + errors[-2000:] + ' (artifacts: /tmp/paneferry-video-display-failure)')
    print(output, end='')
