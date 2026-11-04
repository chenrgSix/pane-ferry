#!/usr/bin/env python3
"""Launch the packaged Tauri GUI on Xvfb with private read-only IPC fixtures."""
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import threading
import time
import uuid

binary = Path(sys.argv[1]).resolve()
assert binary.is_file()
with tempfile.TemporaryDirectory(prefix='pf-shared-ui-') as temporary:
    home = Path(temporary); state = home / '.local/state/lan-remoteapp'
    (state / 'network').mkdir(parents=True)
    events = []; servers = []; connections = []; identity = str(uuid.uuid4())
    def serve(path, management):
        listener = socket.socket(socket.AF_UNIX); listener.bind(str(path)); listener.listen(); servers.append(listener)
        def client(sock):
            connections.append(sock)
            with sock, sock.makefile('rb') as source:
                try:
                    for line in source:
                        request = json.loads(line); op = request.get('op', request.get('type'))
                        events.append((management, op))
                        if management:
                            assert op == 'status', 'startup must not mutate pairing or sharing'
                            response = dict(device_id=identity, name='Fixture', listen='127.0.0.1:0', peers=[])
                        elif op == 'hello': response = dict(type='hello', version=1, capabilities=[])
                        else:
                            assert op == 'node.info', 'startup must only read local sharing status'
                            response = dict(type='node.info', version=1, platform='linux', device_id=identity, name='Fixture', sharing_enabled=False)
                        sock.sendall(json.dumps(response).encode() + b'\n')
                except (OSError, ValueError): pass
        def accept():
            while True:
                try: sock, _ = listener.accept()
                except OSError: return
                threading.Thread(target=client, args=(sock,), daemon=True).start()
        threading.Thread(target=accept, daemon=True).start()
    serve(state / 'agent.sock', False); serve(state / 'network/control.sock', True)
    env = dict(os.environ, HOME=str(home), XDG_DATA_HOME=str(home / '.local/share'),
               XDG_CONFIG_HOME=str(home / '.config'), XDG_CACHE_HOME=str(home / '.cache'),
               GDK_BACKEND='x11', LIBGL_ALWAYS_SOFTWARE='1', WEBKIT_DISABLE_DMABUF_RENDERER='1')
    with open(home / 'launcher.log', 'w+') as log:
        child = subprocess.Popen([str(binary)], env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 15
            while not ((False, 'node.info') in events and (True, 'status') in events):
                if child.poll() is not None:
                    log.seek(0); raise AssertionError(log.read())
                assert time.monotonic() < deadline, 'shared UI did not reach the native adapters'
                time.sleep(.05)
            # Both calls originate in App.vue's mounted lifecycle, confirming
            # that the packaged WebView loaded and executed the common frontend.
            assert child.poll() is None
            assert all(op in ('hello', 'node.info', 'status') for _, op in events)
            print('PASS packaged Tauri/Vue UI: WebView mounted, Linux local/network adapters reached, private HOME and no pairing/sharing mutations')
        finally:
            child.terminate(); child.wait(timeout=8)
            for listener in servers: listener.close()
            for sock in connections:
                try: sock.shutdown(socket.SHUT_RDWR)
                except OSError: pass
