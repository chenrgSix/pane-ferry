#!/usr/bin/env python3
"""Exercise the real installer in a disposable home with a fake service manager."""
from pathlib import Path
import os
import shutil
import subprocess
import tempfile

root = Path(__file__).resolve().parents[2]
with tempfile.TemporaryDirectory(prefix='paneferry-install-test-') as temporary:
    scratch = Path(temporary)
    package, task_home, commands = scratch/'package', scratch/'home', scratch/'commands'
    for folder in (package, task_home, commands):
        folder.mkdir()
    shutil.copyfile(root/'deploy/install-linux.sh', package/'install.sh')
    for name, content in {
        'bin/paneferry-agent': 'new agent',
        'share/paneferry/main.py': '# fixture',
        'share/applications/io.lanremoteapp.launcher.desktop': 'Exec=paneferry\n',
        'share/icons/hicolor/128x128/apps/icon.png': 'fixture',
        'systemd/remoteapp-agent.service': 'fixture',
        'share/gnome-shell/extensions/paneferry@local/metadata.json': '{}',
    }.items():
        path = package/name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    for name, content in {
        'systemctl': 'printf "%s\\n" "$*" >> "$PANEFERRY_TEST_CALLS"\nexit 0',
        # Dependency/desktop-entry checks are outside this lifecycle test.
        'python3': 'exit 0',
        'gnome-shell': 'printf "GNOME Shell 50.1\\n"',
        'gnome-extensions': 'exit 1',
    }.items():
        path = commands/name
        path.write_text('#!/bin/sh\n'+content+'\n')
        path.chmod(0o755)
    calls = scratch/'calls'
    env = dict(os.environ, HOME=str(task_home), XDG_DATA_HOME=str(task_home/'.local/share'),
               PATH=str(commands)+os.pathsep+os.environ['PATH'], PANEFERRY_TEST_CALLS=str(calls))
    refused = subprocess.run(['/bin/bash', str(package/'install.sh')], env=env, capture_output=True, text=True)
    assert refused.returncode == 2, refused
    assert '--defer-restart' in refused.stderr
    assert not (task_home/'.local').exists(), 'active-service check must precede file writes'
    binary = task_home/'.local/bin/paneferry-agent'
    binary.parent.mkdir(parents=True)
    binary.write_text('old agent')
    with binary.open() as old_inode:
        installed = subprocess.run(['/bin/bash', str(package/'install.sh'), '--defer-restart'],
                                   env=env, capture_output=True, text=True)
        assert installed.returncode == 0, installed.stderr
        assert binary.read_text() == 'new agent' and old_inode.read() == 'old agent'
    assert calls.read_text().splitlines() == [
        '--user is-active --quiet remoteapp-agent.service', '--user daemon-reload'], calls.read_text()
    assert (task_home/'.local/share/gnome-shell/extensions/paneferry@local/metadata.json').exists()
    assert '保存工作' in installed.stdout and '下次登录' in installed.stdout
print('PASS: active service blocks non-deferred installation before writes; deferred update preserves the old binary inode and never stops or restarts applications')
