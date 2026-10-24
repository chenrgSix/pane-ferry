#!/usr/bin/env python3
"""Inspect a built Debian package without installing it on the current host."""
from pathlib import Path
import json
import subprocess
import sys
import tarfile
import tempfile

root = Path(__file__).resolve().parents[2]
deb = Path(sys.argv[1]).resolve()
fields = subprocess.check_output(['dpkg-deb', '--field', str(deb)], text=True)
assert 'Package: paneferry\n' in fields
for dependency in ('libc6 (>= ', 'libei1', 'python3-websocket', 'gstreamer1.0-plugins-bad', 'xdg-desktop-portal'):
    assert dependency in fields, dependency
with tempfile.TemporaryDirectory(prefix='paneferry-deb-check-') as temporary:
    extracted = Path(temporary)/'package'
    subprocess.run(['dpkg-deb', '--raw-extract', str(deb), str(extracted)], check=True)
    assert not any((extracted/'DEBIAN'/name).exists() for name in ('preinst', 'postinst', 'prerm', 'postrm'))
    assert not (extracted/'home').exists()
    assert not (extracted/'root').exists()
    assert (extracted/'usr/share/doc/paneferry/LICENSE').read_bytes() == (root/'LICENSE').read_bytes()
    entry = extracted/'usr/share/applications/io.lanremoteapp.launcher.desktop'
    subprocess.run(['desktop-file-validate', str(entry)], check=True)
    assert 'Exec=/usr/bin/paneferry\n' in entry.read_text()
    assert '/usr/lib/paneferry/main.py' in (extracted/'usr/bin/paneferry').read_text()
    unit = (extracted/'usr/lib/systemd/user/remoteapp-agent.service').read_text()
    assert 'ExecStart=/usr/lib/paneferry/paneferry-agent serve' in unit
    for name in ('paneferry-agent', 'paneferry-network'):
        link = extracted/'usr/bin'/name
        assert link.resolve() == extracted/'usr/lib/paneferry'/name
        assert link.stat().st_mode & 0o111
    assert json.loads((extracted/'usr/share/gnome-shell/extensions/paneferry@local/metadata.json').read_text())['uuid']=='paneferry@local'
    with subprocess.Popen(['dpkg-deb', '--fsys-tarfile', str(deb)], stdout=subprocess.PIPE) as archive:
        with tarfile.open(fileobj=archive.stdout, mode='r|') as content:
            for member in content:
                assert member.uid == member.gid == 0, member.name
                if not member.issym():
                    assert not member.mode & 0o022, member.name
        assert archive.wait() == 0
print('PASS: Debian dependencies, desktop entry, helpers, unit, extension, owner/modes and license; no session-changing maintainer scripts')
