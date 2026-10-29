#!/usr/bin/env python3
"""Native title latency via private status sockets; no user devices or inputs."""
from pathlib import Path
import json
import socket
import subprocess
import tempfile
import threading
import time

root = Path(__file__).resolve().parents[2]
source = root/'apps/mac-windowhost/Sources/PaneFerryWindowHost'
with tempfile.TemporaryDirectory(prefix='pf-latency-', dir='/tmp') as directory:
    scratch = Path(directory)
    types = scratch/'Types.swift'
    types.write_text((source/'main.swift').read_text().split('\n// WINDOWHOST_ENTRYPOINT\n')[0])
    binary = scratch/'LatencySmoke'
    subprocess.run(['swiftc', '-module-cache-path', '/tmp/paneferry-latency-test-modules',
        *map(str, sorted(p for p in source.glob('*.swift') if p.name != 'main.swift')),
        str(types), str(root/'tests/swift/NetworkLatencySmoke.swift'), '-o', str(binary)], check=True, timeout=120)
    for mode in ('sample', 'slow'):
        state = scratch/mode
        state.mkdir()
        listener = socket.socket(socket.AF_UNIX)
        listener.bind(str(state/'control.sock'))
        listener.listen()
        requests = []
        def serve():
            with listener.accept()[0] as client:
                request = json.loads(client.makefile('rb').readline(4096))
                requests.append(request)
                if mode == 'slow':
                    time.sleep(1.3)
                else:
                    data = json.dumps({'peers':[
                        {'device_id':'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb','connected':True,'rtt_ms':999},
                        {'device_id':'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa','connected':True,'rtt_ms':24.75}]}).encode()+b'\n'
                    for offset in range(0, len(data), 11):
                        client.sendall(data[offset:offset+11])
        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        result = subprocess.run([str(binary), str(state), mode], capture_output=True, text=True, timeout=10)
        thread.join(timeout=3)
        listener.close()
        assert result.returncode == 0, (result.stdout, result.stderr[-6000:])
        assert requests == [{'op':'status'}], requests
        print(result.stdout, end='')
