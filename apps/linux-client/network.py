"""Private management IPC; invitation secrets stay out of command arguments."""
import json
import socket
import subprocess
import time
from pathlib import Path
from wire import STATE, request


def action(op, **fields):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(20)
        client.connect(str(STATE / 'network/control.sock'))
        client.sendall(json.dumps(dict(op=op, **fields), separators=(',', ':')).encode() + b'\n')
        with client.makefile('rb') as source:
            line = source.readline(128 * 1024 + 1)
        if len(line) > 128 * 1024 or not line.endswith(b'\n'):
            raise ValueError('network_response_invalid')
        result = json.loads(line)
        if 'error' in result:
            raise ValueError('network_operation_failed')
        return result


def ensure():
    try:
        action('status'); return
    except (OSError, ValueError):
        pass
    info = request(None, 'node.info', local=True)
    binary = Path.home() / '.local/bin/remoteapp-network'
    agent = Path.home() / '.local/bin/remoteapp-agent'
    process = subprocess.Popen([str(binary), 'serve', '--state', str(STATE / 'network'),
        '--publisher', str(STATE / 'agent.sock'), '--device-id', info['device_id'],
        '--name', info.get('name', 'Ubuntu'), '--bulk-agent', str(agent)],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    for _ in range(40):
        try:
            action('status'); return
        except (OSError, ValueError):
            if process.poll() is not None:
                break
            time.sleep(.05)
    raise ValueError('network_unavailable')
