#!/usr/bin/env python3
"""Credential tests in a temporary Keychain; never use the user's saved items."""
from pathlib import Path
import subprocess
import tempfile
root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='remoteapp-password-') as directory:
    scratch=Path(directory)
    binary=scratch/'smoke'
    subprocess.run(['swiftc','-module-cache-path',str(scratch/'modules'),
                    str(root/'apps/mac-windowhost/Sources/RemoteAppWindowHost/SSHAuthentication.swift'),
                    str(root/'tests/swift/SSHPasswordSmoke.swift'),'-o',str(binary)],check=True,timeout=120)
    subprocess.run([str(binary),str(scratch)],check=True,timeout=20)
