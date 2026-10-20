#!/usr/bin/env python3
"""Create deterministic source ZIP, excluding build outputs, secrets, and symlinks."""
from pathlib import Path
from zipfile import ZipFile, ZipInfo, ZIP_DEFLATED
import hashlib
root = Path(__file__).resolve().parents[1]
out = root.parent / 'lan-remoteapp-source.zip'
skip = {'target', 'node_modules', 'dist', '.build', '.git', '__pycache__', '.swiftpm'}
secret_suffixes = {'.pem', '.key', '.p12', '.pfx', '.log'}
with ZipFile(out, 'w', compression=ZIP_DEFLATED) as z:
    for p in sorted(root.rglob('*')):
        rel = p.relative_to(root)
        if p.is_symlink() or not p.is_file() or set(rel.parts) & skip:
            continue
        if p.name.startswith('.env') or p.suffix in secret_suffixes or p.name == '.DS_Store':
            continue
        info = ZipInfo('lan-remoteapp/' + rel.as_posix(), date_time=(2026, 10, 20, 0, 0, 0))
        info.compress_type = ZIP_DEFLATED
        info.external_attr = (p.stat().st_mode & 0xffff) << 16
        z.writestr(info, p.read_bytes())
print(out)
print('SHA256', hashlib.sha256(out.read_bytes()).hexdigest())
