#!/usr/bin/env python3
"""Isolated HOME: restricted key installer, idempotence, revocation and dispatcher.

No real account authorization, SSH login or user keys are modified.
"""
import base64
import importlib.util
import json
import os
from pathlib import Path
import stat
import struct
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('automatic_identity', root/'scripts/enable-auto-reconnect.py')
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
dispatcher = (root/'scripts/remoteapp-ssh-command').read_bytes()
public = 'ssh-ed25519 '+base64.b64encode(struct.pack('>I', 11)+b'ssh-ed25519'+struct.pack('>I', 32)+bytes(range(32))).decode()
with tempfile.TemporaryDirectory(prefix='remoteapp-auto-identity-') as temporary:
    home = Path(temporary)
    env = dict(os.environ, HOME=str(home))
    ssh = home/'.ssh'; ssh.mkdir(mode=0o700)
    authorized = ssh/'authorized_keys'
    original = '# existing user entry\nssh-ed25519 unrelated-key keep-user-entry\n'
    authorized.write_text(original)
    payload = dict(action='enable', public_key=public, dispatcher=base64.b64encode(dispatcher).decode())
    def install():
        return subprocess.run([sys.executable, '-c', module.INSTALLER], env=env, input=json.dumps(payload).encode(), capture_output=True)
    result = install(); assert result.returncode == 0, result.stderr
    first = authorized.read_text()
    assert first.startswith(original) and first.count(public) == 1
    assert r'restrict,command="exec \"$HOME/.local/bin/remoteapp-ssh-command\""' in first
    assert stat.S_IMODE(authorized.stat().st_mode) == 0o600
    installed = home/'.local/bin/remoteapp-ssh-command'
    assert installed.read_bytes() == dispatcher and stat.S_IMODE(installed.stat().st_mode) == 0o700
    assert install().returncode == 0 and authorized.read_text() == first
    agent = home/'.local/bin/remoteapp-agent'
    agent.write_text('#!/bin/sh\nprintf "%s\\n" "$@"\ncat\n'); agent.chmod(0o700)
    for command, output in [('.local/bin/remoteapp-agent bridge', b'bridge\npayload'),
                            ('.local/bin/remoteapp-agent clipboard-upload', b'clipboard-upload\npayload'),
                            ('.local/bin/remoteapp-agent clipboard-download transfer_1-2', b'clipboard-download\ntransfer_1-2\npayload'),
                            ('/usr/bin/true', b'')]:
        result = subprocess.run(['/bin/sh', str(installed)], env=dict(env, SSH_ORIGINAL_COMMAND=command), input=b'payload', capture_output=True)
        assert result.returncode == 0 and result.stdout == output, (command, result)
    for command in ('', 'sh', 'id', '.local/bin/remoteapp-agent serve', '.local/bin/remoteapp-agent bridge; id',
                    '.local/bin/remoteapp-agent clipboard-download $(id)', '.local/bin/remoteapp-agent clipboard-download ../file',
                    '.local/bin/remoteapp-agent clipboard-download a\nb', '.local/bin/remoteapp-agent clipboard-download '+'a'*101):
        result = subprocess.run(['/bin/sh', str(installed)], env=dict(env, SSH_ORIGINAL_COMMAND=command), capture_output=True)
        assert result.returncode == 126, command
    payload['action'] = 'disable'
    assert install().returncode == 0 and authorized.read_text() == original
    authorized.unlink(); authorized.symlink_to(home/'unrelated')
    payload['action'] = 'enable'
    assert install().returncode != 0 and not (home/'unrelated').exists()
    for host, user, port in [('$(id)', 'alice', 22), ('host', 'x;id', 22), ('host..x', 'alice', 22), ('host', 'alice', 0)]:
        try:
            module.identity(home, host, user, port)
            raise AssertionError('unsafe endpoint accepted')
        except ValueError:
            pass
print('PASS: isolated restricted identity installation, dispatcher allowlist and revocation')
