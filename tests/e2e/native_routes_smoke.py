#!/usr/bin/env python3
"""Exercise actual Python device routes over three QUIC nodes, without GTK."""
from pathlib import Path
import json
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid
root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root/'apps/linux-client'))
import wire
binary = root/'target/debug/remoteapp-network'
subprocess.run(['cargo', 'build', '-p', 'remoteapp-network'], cwd=root, check=True)
with tempfile.TemporaryDirectory(prefix='rwn-routes-', dir='/tmp') as temporary:
    state = Path(temporary)
    processes, listeners = [], []
    wire.STATE = state/'a'

    def publisher(path, name):
        path.parent.mkdir(parents=True, exist_ok=True)
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(path)); listener.listen(); listeners.append(listener)
        def serve():
            while True:
                try: client, _ = listener.accept()
                except OSError: return
                def handle(client):
                    try:
                        with client, client.makefile('rb') as stream:
                            for line in stream:
                                event = json.loads(line)
                                client.sendall(json.dumps(dict(type=event['type'], version=1, name=name)).encode()+b'\n')
                    except OSError: pass
                threading.Thread(target=handle, args=(client,), daemon=True).start()
        threading.Thread(target=serve, daemon=True).start()

    def manage(directory, **body):
        with socket.socket(socket.AF_UNIX) as client:
            client.settimeout(10); client.connect(str(directory/'control.sock'))
            client.sendall(json.dumps(body).encode()+b'\n')
            response = json.loads(client.makefile('rb').readline())
            assert 'error' not in response, response
            return response

    try:
        directories = [wire.STATE/'network', state/'b', state/'c']
        for directory, name in zip(directories, ['a', 'b', 'c']):
            path = state/(name+'.sock'); publisher(path, name)
            processes.append(subprocess.Popen([str(binary), 'serve', '--state', str(directory),
                '--publisher', str(path), '--listen', '127.0.0.1:0'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL))
        for directory in directories:
            deadline = time.monotonic()+10
            while True:
                try: manage(directory, op='status'); break
                except OSError:
                    assert time.monotonic() < deadline
                    time.sleep(.02)
        ids = [manage(d, op='status')['device_id'] for d in directories]
        for directory in directories[1:]:
            remote = manage(directory, op='status')
            manage(directories[0], op='pair', invitation=manage(directory, op='invite')['invitation'], address=remote['listen'])
        deadline = time.monotonic()+5
        while not all((directories[0]/('r-'+peer+'.sock')).exists() for peer in ids[1:]):
            assert time.monotonic() < deadline
            time.sleep(.02)
        # A stale reverse-route record must never appear or provide a fallback.
        generation = uuid.uuid4().hex
        old_path = wire.STATE/'peers'/('p'+generation+'.sock')
        publisher(old_path, 'legacy')
        wire.save(wire.STATE/'peers'/(ids[1]+'.json'), dict(version=1, device_id=ids[1], name='legacy', generation=generation, socket=str(old_path)))
        assert len(wire.peers()) == 2
        for peer, name in zip(ids[1:], ['b', 'c']):
            assert wire.request('quic:'+peer, 'app.list')['name'] == name
        try: wire.route(ids[1]); raise AssertionError('legacy device route accepted')
        except ValueError: pass
        # Reverse calls use the same paired relationship, not a return tunnel.
        bridge = subprocess.Popen([str(binary), 'bridge', '--socket', str(directories[1]/('r-'+ids[0]+'.sock'))], stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        processes.append(bridge)
        bridge.stdin.write(b'{"type":"hello","version":1}\n'); bridge.stdin.flush()
        assert json.loads(bridge.stdout.readline())['name'] == 'a'
        manage(directories[0], op='revoke', device_id=ids[1])
        bridge.wait(timeout=5)  # Parent intentionally still holds stdin open.
        bridge.stdin.close(); bridge.stdout.close()
        try: wire.request('quic:'+ids[1], 'app.list'); raise AssertionError('revoked route accepted')
        except ValueError: pass
        assert wire.request('quic:'+ids[2], 'app.list')['name'] == 'c'
        print('PASS: three QUIC nodes; bidirectional Python routes; two peers isolated; revocation preserves other peer; legacy records ignored, no fallback')
    finally:
        for process in processes:
            if process.poll() is None: process.terminate()
            process.wait(timeout=5)
        for listener in listeners: listener.close()
