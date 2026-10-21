#!/usr/bin/env python3
"""Exercise actual AppDelegate with an isolated pasteboard and injected wire; no SSH or system clipboard."""
from pathlib import Path
import subprocess
import tempfile
root=Path(__file__).resolve().parents[2];source=root/'apps/mac-windowhost/Sources/RemoteAppWindowHost'
with tempfile.TemporaryDirectory(prefix='remoteapp-clipboard-order-') as directory:
    scratch=Path(directory);types=scratch/'WindowHostTypes.swift'
    text=(source/'main.swift').read_text();marker='\nlet application = NSApplication.shared\n';assert text.count(marker)==1
    types.write_text(text.split(marker)[0])
    files=[str(file) for file in source.glob('*.swift') if file.name!='main.swift']
    binary=scratch/'smoke'
    subprocess.run(['swiftc','-module-cache-path',str(scratch/'modules'),*files,str(types),str(root/'tests/swift/ClipboardOrderingSmoke.swift'),'-o',str(binary)],check=True,timeout=120)
    subprocess.run([str(binary)],check=True,timeout=15)
