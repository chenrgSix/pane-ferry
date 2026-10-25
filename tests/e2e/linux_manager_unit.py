#!/usr/bin/env python3
"""Run actual GTK manager lifecycle methods with deterministic widget fixtures."""
import ast
from pathlib import Path
from types import SimpleNamespace
from threading import Event

root = Path(__file__).resolve().parents[2]
source = ast.parse((root / 'apps/linux-client/main.py').read_text())
manager = next(node for node in source.body if isinstance(node, ast.ClassDef) and node.name == 'Manager')
methods = {'manager_destroyed', 'poll_peers', 'refresh_peers', 'open_viewer'}
manager.body = [node for node in manager.body if isinstance(node, ast.FunctionDef) and node.name in methods]
manager.bases = []
peer = dict(connection_id='quic:one', name='First device', generation='one')
removed = []
created = []

def viewer(application, identity, name, session_id=None, app_id=None):
    value = SimpleNamespace(peer_id=identity, session_id=session_id, closed=Event(), presented=0)
    def present(): value.presented += 1
    value.window = SimpleNamespace(present=present)
    created.append(value)
    return value

namespace = dict(peers=lambda: [peer], GLib=SimpleNamespace(source_remove=removed.append), Viewer=viewer)
exec(compile(ast.Module(body=[manager], type_ignores=[]), '<production manager>', 'exec'), namespace)
Manager = namespace['Manager']
state = Manager()
state.window = object()
state.selected = peer['connection_id']
state.peer_keys = []
state.peer_timer = 42
rows = []
state.selector = SimpleNamespace(remove_all=rows.clear, append=lambda *item: rows.append(item),
                                 set_active_id=lambda _: None, set_active=lambda _: None)
state.clear_rows = lambda: None
state.set_notice = lambda _: None
# GLib idle sources repeat whenever their callback returns True. Management
# completion must perform exactly one read; only the timer callback repeats.
assert state.refresh_peers() is False
assert rows == [('quic:one', 'First device')]
assert state.poll_peers() is True
assert state.refresh_peers() is False
state.manager_destroyed()
assert state.window is None and state.peer_timer is None and removed == [42]
assert state.poll_peers() is False
# activate() resets peer_keys before constructing the new selector. Keep this
# wiring covered because identical inventory must still populate new widgets.
original = (root / 'apps/linux-client/main.py').read_text()
activate = ast.parse(original).body
klass = next(n for n in activate if isinstance(n, ast.ClassDef) and n.name == 'Manager')
activate = next(n for n in klass.body if isinstance(n, ast.FunctionDef) and n.name == 'activate')
assert any(isinstance(n, ast.Assign) and any(isinstance(t, ast.Attribute) and t.attr == 'peer_keys' for t in n.targets)
           for n in ast.walk(activate))
state.window = object()
state.peer_keys = []
rows.clear()
state.refresh_peers()
assert rows == [('quic:one', 'First device')]
state.viewers = []
state.open_viewer('quic:one', 'First device', session_id='shared-id')
state.open_viewer('quic:one', 'First device', session_id='shared-id')
assert len(state.viewers) == 1 and len(created) == 1 and created[0].presented == 1
state.open_viewer('quic:two', 'Second device', session_id='shared-id')
assert len(state.viewers) == 2, 'session IDs must remain device-scoped'
created[0].closed.set()
state.open_viewer('quic:one', 'First device', session_id='shared-id')
assert len(state.viewers) == 2 and len(created) == 3, 'closed sessions may reconnect'
# Reopening an existing session should still work at the receiver count limit.
state.viewers += [viewer(state, 'quic:other', str(i), session_id=str(i)) for i in range(6)]
state.open_viewer('quic:one', 'First device', session_id='shared-id')
assert len(state.viewers) == 8 and state.viewers[1].presented == 1
print('PASS GTK one-shot refresh, poll cleanup, reopen inventory and device-scoped receiver deduplication')
