#!/usr/bin/env python3
"""Both installation layouts must select matching native helpers."""
from pathlib import Path
import importlib.util
import os
import tempfile
from unittest.mock import patch

root = Path(__file__).resolve().parents[2]
spec = importlib.util.spec_from_file_location('runtime', root/'apps/linux-client/runtime.py')
runtime = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runtime)
with tempfile.TemporaryDirectory(prefix='paneferry-runtime-') as temporary:
    directory = Path(temporary).resolve()
    client = directory/'system'
    client.mkdir()
    home = directory/'home'
    legacy = home/'.local/bin'
    legacy.mkdir(parents=True)
    for folder in (client, legacy):
        for name in ('paneferry-agent', 'paneferry-network'):
            binary = folder/name
            binary.write_text('#!/bin/sh\nexit 0\n')
            binary.chmod(0o755)
    with patch.object(runtime, '__file__', str(client/'runtime.py')), patch.dict(os.environ, HOME=str(home)):
        for name in ('paneferry-agent', 'paneferry-network'):
            assert runtime.helper_binary(name) == client/name, 'system client must not choose an old user helper'
            (client/name).unlink()
            assert runtime.helper_binary(name) == legacy/name
        with patch.object(runtime.subprocess, 'Popen') as spawn:
            runtime.start_agent()
            assert spawn.call_args.args[0] == [str(legacy/'paneferry-agent'), 'serve']
        (legacy/'paneferry-network').chmod(0o644)
        try:
            runtime.helper_binary('paneferry-network')
            raise AssertionError('non-executable helper accepted')
        except ValueError as error:
            assert str(error) == 'network_unavailable'
        try:
            runtime.helper_binary('../other')
            raise AssertionError('unknown helper accepted')
        except ValueError:
            pass
with patch.object(runtime, 'helper_binary', return_value=Path('/usr/lib/paneferry/paneferry-agent')):
    with patch.object(runtime.subprocess, 'run') as service, patch.object(runtime.subprocess, 'Popen') as spawn:
        service.return_value.returncode = 0
        runtime.start_agent()
        assert service.call_args.args[0] == ['systemctl', '--user', 'start', 'remoteapp-agent.service']
        spawn.assert_not_called()
        service.return_value.returncode = 1
        runtime.start_agent()
        assert spawn.call_args.args[0] == ['/usr/lib/paneferry/paneferry-agent', 'serve']
print('PASS: Debian/user helper selection, executable checks, user service start and container fallback')
