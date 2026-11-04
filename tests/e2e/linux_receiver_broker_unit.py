#!/usr/bin/env python3
"""Production broker validation and per-session dispatch, without a display."""
import ast
import json
import os
from pathlib import Path
import socket
import sys
import threading
from types import SimpleNamespace
import uuid

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'apps/linux-client'))
from receiver_broker import Broker, validate, valid_id

group = uuid.uuid4().hex + uuid.uuid4().hex
request = dict(version=1, group=group, op='attach', session='session-1', target='org.example.App.desktop',
               fps=30, background=True, picture_mode='source', video_color='444-atlas-v1')
assert validate(request, group) is request
for field, value in [('version', True), ('version', 2), ('group', 'other'), ('fps', True), ('fps', 90),
                     ('session', '../bad'), ('target', '-option'), ('op', 'quit'), ('background', 'true'),
                     ('picture_mode', '2x'), ('video_color', 'unknown')]:
    try:
        validate(dict(request, **{field: value}), group)
        raise AssertionError((field, value))
    except ValueError: pass
assert valid_id('wayland-100-123-456') and valid_id('org.example.App.desktop')

class Window:
    def __init__(self): self.signals = {}; self.presented = 0
    def connect(self, signal, callback): self.signals[signal] = callback
    def present(self): self.presented += 1
class Viewer:
    def __init__(self, application, peer, name, session_id):
        self.closed = threading.Event(); self.window = Window(); self.peer = peer
        self.focused = 0
        self.view = SimpleNamespace(grab_focus=lambda: setattr(self, 'focused', self.focused + 1))
        self.preferences = application.source_size, application.video_color, application.requested_fps
    def close(self):
        self.closed.set(); self.window.signals['destroy']()

tree = ast.parse((root / 'apps/linux-client/receiver_main.py').read_text())
production = next(node for node in tree.body if isinstance(node, ast.ClassDef) and node.name == 'ReceiverApplication')
methods = [node for node in production.body if isinstance(node, ast.FunctionDef) and node.name in ('dispatch', 'closed', 'quit_if_idle')]
module = ast.Module(body=[ast.ClassDef(name='Application', bases=[], keywords=[], body=methods, decorator_list=[])], type_ignores=[])
namespace = dict(Broker=Broker, Viewer=Viewer, os=os, select=__import__('select'), socket=socket,
                 GLib=SimpleNamespace(timeout_add=lambda *_: None))
exec(compile(ast.fix_missing_locations(module), '<production-receiver-dispatch>', 'exec'), namespace)
app = namespace['Application'](); app.sessions = {}; app.options = SimpleNamespace(native_peer=str(uuid.uuid4()), name='Fixture')

if sys.platform == 'linux':
    app.broker = Broker(group, app.dispatch)
    def connect(body):
        client = socket.socket(socket.AF_UNIX); client.settimeout(3); client.connect(str(app.broker.path))
        client.sendall(json.dumps(body).encode() + b'\n')
        return client, client.makefile('rb')
    def read(source): return json.loads(source.readline(16 * 1024 + 1))
    clients = []
    try:
        for body in (request, dict(request), dict(request, session='session-2', fps=60, background=False, picture_mode='adapt', video_color='420')):
            client, source = connect(body); clients.append((client, source))
            assert read(source) == dict(version=1, pid=os.getpid(), session=body['session'])
        assert len(app.sessions) == 2, 'manual re-attach must reuse the same receiver'
        assert app.sessions['session-1']['viewer'].preferences == (True, '444-atlas-v1', 30)
        assert app.sessions['session-1']['viewer'].window.presented == 0, 'automatic attach must not steal focus'
        assert app.sessions['session-2']['viewer'].preferences == (False, '420', 60)
        assert app.sessions['session-2']['viewer'].focused == 1
        client, source = connect(dict(version=1, group=group, op='list'))
        assert len(read(source)['sessions']) == 2; source.close(); client.close()
        client, source = connect(dict(version=1, group=group, op='detach', session='session-1'))
        assert read(source)['detached']; source.close(); client.close()
        for _, source in clients[:2]: assert read(source)['closed'] is True
        assert list(app.sessions) == ['session-2'], 'closing one window must preserve sibling sessions'
        assert not app.sessions['session-2']['viewer'].closed.is_set()
        client, source = connect(dict(request, fps=100))
        assert source.readline() == b''; source.close(); client.close()
    finally:
        app.broker.close()
        for client, source in clients: source.close(); client.close()
    assert not app.broker.path.exists()
    print('PASS actual private broker: deduplication, independent close/watchers, per-window 30/60/source/color, background attach and malformed requests')
else:
    print('PASS broker validation; actual SO_PEERCRED socket lifecycle requires Linux')
