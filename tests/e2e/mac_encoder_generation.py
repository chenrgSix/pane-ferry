#!/usr/bin/env python3
from pathlib import Path
import subprocess,tempfile
root=Path(__file__).resolve().parents[2];source=root/'apps/mac-node/Sources/PaneFerryNode'
with tempfile.TemporaryDirectory(prefix='paneferry-encoder-generation-') as tmp:
    binary=str(Path(tmp)/'probe')
    subprocess.run(['swiftc','-module-cache-path',tmp+'/modules',*[str(source/name) for name in ('Wire.swift','Capture.swift','VideoRateController.swift')],str(root/'tests/swift/MacEncoderGenerationSmoke.swift'),'-o',binary],check=True,timeout=120)
    subprocess.run([binary],check=True,timeout=15)
