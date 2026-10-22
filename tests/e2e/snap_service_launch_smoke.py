#!/usr/bin/env python3
"""Opt-in Ubuntu test: actual Snap startup under the shipped user-service policy.
Only runs Firefox --version; never opens a window or creates a browser profile.
"""
import configparser
from pathlib import Path
import subprocess

root = Path(__file__).resolve().parents[2]
unit = configparser.ConfigParser(interpolation=None)
unit.read(root/'deploy/linux-user-service/remoteapp-agent.service')
policy = unit['Service'].get('NoNewPrivileges', 'no')

def version(restriction):
    return subprocess.run([
        '/usr/bin/systemd-run', '--user', '--pipe', '--wait', '--collect', '--quiet',
        '--property=Type=exec', '--property=NoNewPrivileges='+restriction,
        '/usr/bin/systemd-run', '--user', '--scope', '--collect', '--quiet',
        '/snap/bin/firefox', '--version',
    ], capture_output=True, text=True, timeout=20)

blocked = version('yes')
assert blocked.returncode != 0 and 'snap-confine' in blocked.stderr, blocked
working = version(policy)
assert working.returncode == 0 and 'Mozilla Firefox' in working.stdout, working
print('PASS: inherited NoNewPrivileges reproduces snap-confine rejection; shipped user-service policy starts the same Snap and profile-free --version command')
