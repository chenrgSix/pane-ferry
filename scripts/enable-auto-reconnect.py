#!/usr/bin/env python3
"""One-time, explicit setup/revocation of a restricted RemoteApp SSH identity.

System SSH handles the initial password, if needed. No passwords are stored.
"""
import argparse
import base64
import json
import os
from pathlib import Path
import re
import shlex
import stat
import subprocess

# Fixed installation program; endpoint data and the public key arrive as JSON,
# never as shell code. Preserve unrelated authorized_keys entries under a lock.
INSTALLER = r'''
import base64, fcntl, json, os, pathlib, stat, struct, tempfile
data = json.load(__import__('sys').stdin)
public = data['public_key'].split()
assert len(public) == 2 and public[0] == 'ssh-ed25519'
blob = base64.b64decode(public[1], validate=True)
assert blob[:19] == struct.pack('>I', 11) + b'ssh-ed25519' + struct.pack('>I', 32) and len(blob) == 51
home = pathlib.Path.home()
ssh = home/'.ssh'
ssh.mkdir(mode=0o700, exist_ok=True)
assert not ssh.is_symlink() and ssh.stat().st_uid == os.getuid()
os.chmod(ssh, 0o700)
with (ssh/'.lan-remoteapp.lock').open('a') as lock:
    os.chmod(lock.name, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX)
    target = ssh/'authorized_keys'
    if target.exists() or target.is_symlink():
        assert not target.is_symlink() and target.is_file() and target.stat().st_uid == os.getuid()
        assert target.stat().st_size <= 1024*1024
        original = target.read_text()
    else:
        original = ''
    lines = [line for line in original.splitlines() if public[1] not in line.split()]
    if data['action'] == 'enable':
        directory = home/'.local/bin'
        directory.mkdir(parents=True, exist_ok=True)
        dispatcher = directory/'remoteapp-ssh-command'
        assert not dispatcher.is_symlink()
        content = base64.b64decode(data['dispatcher'], validate=True)
        assert len(content) < 4096 and content.startswith(b'#!/bin/sh\n')
        fd, temporary = tempfile.mkstemp(prefix='.remoteapp-command-', dir=directory)
        with os.fdopen(fd, 'wb') as output:
            output.write(content); output.flush(); os.fsync(output.fileno()); os.fchmod(output.fileno(), 0o700)
        os.replace(temporary, dispatcher)
        options = r'restrict,command="exec \"$HOME/.local/bin/remoteapp-ssh-command\""'
        lines.append(options+' '+public[0]+' '+public[1]+' lan-remoteapp-auto')
    else:
        assert data['action'] == 'disable'
    updated = '\n'.join(lines)+'\n' if lines else ''
    fd, temporary = tempfile.mkstemp(prefix='.authorized_keys-', dir=ssh)
    with os.fdopen(fd, 'w') as output:
        output.write(updated); output.flush(); os.fsync(output.fileno()); os.fchmod(output.fileno(), 0o600)
    os.replace(temporary, target)
print('RemoteApp restricted key '+data['action']+'d')
'''


def endpoint(host, user, port):
    if (not re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9.-]{0,252}', host)
            or any(not label or len(label) > 63 or label.startswith('-') or label.endswith('-') for label in host.split('.'))
            or not re.fullmatch(r'[A-Za-z0-9_][A-Za-z0-9_-]{0,63}', user)
            or not 1 <= port <= 65535):
        raise ValueError('invalid SSH endpoint')


def identity(home, host, user, port):
    endpoint(host, user, port)
    return home/'.ssh/lan-remoteapp-keys'/f'{user}@{host}-{port}'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('host'); parser.add_argument('user')
    parser.add_argument('port', nargs='?', type=int, default=22)
    parser.add_argument('--disable', action='store_true', help='revoke only this key on Ubuntu and disable it locally')
    args = parser.parse_args()
    key = identity(Path.home(), args.host, args.user, args.port)
    marker = key.with_name(key.name+'.enabled')
    if args.disable:
        marker.unlink(missing_ok=True)
    else:
        key.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
        if key.parent.is_symlink():
            raise ValueError('identity directory must not be a symlink')
        key.parent.chmod(0o700)
        if not key.exists():
            subprocess.run(['/usr/bin/ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-C', 'lan-remoteapp-auto', '-f', str(key)], check=True)
        if key.is_symlink() or not stat.S_ISREG(key.stat().st_mode):
            raise ValueError('identity must be a regular file')
        key.chmod(0o600)
    public = key.with_name(key.name+'.pub').read_text().split()
    payload = {'action': 'disable' if args.disable else 'enable', 'public_key': ' '.join(public[:2]),
               'dispatcher': base64.b64encode(Path(__file__).with_name('remoteapp-ssh-command').read_bytes()).decode()}
    connection = ['-F', '/dev/null', '-T', '-oStrictHostKeyChecking=yes', '-oConnectTimeout=8',
                  '-oClearAllForwardings=yes', '-oForwardAgent=no', '-oForwardX11=no', '-oPermitLocalCommand=no',
                  '-p', str(args.port), '-l', args.user]
    # Use the existing password-authenticated master for one-time authorization;
    # never use the restricted identity to install or edit account configuration.
    subprocess.run(['/usr/bin/ssh', *connection, '-S', str(Path.home()/'.ssh/lan-remoteapp-%C'), '--', args.host,
                    '/usr/bin/python3 -c '+shlex.quote(INSTALLER)], input=json.dumps(payload).encode(), check=True)
    if not args.disable:
        # Fresh TCP authentication, not a successful reuse of the old master.
        subprocess.run(['/usr/bin/ssh', *connection, '-S', 'none', '-oBatchMode=yes', '-oIdentitiesOnly=yes',
                        '-i', str(key), '--', args.host, '/usr/bin/true'], check=True)
        temporary = marker.with_name(marker.name+'.pending')
        with temporary.open('w') as output:
            os.fchmod(output.fileno(), 0o600); output.write('RemoteApp restricted SSH identity v1\n')
        temporary.replace(marker)
    print('自动重连已关闭并撤销远端专用密钥。' if args.disable else '自动重连已启用；密码没有保存。')


if __name__ == '__main__':
    main()
