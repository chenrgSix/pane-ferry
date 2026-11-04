#!/usr/bin/env python3
"""Exercise persisted bookmarks and actual manager switching without GTK."""
import ast
import json
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace

root = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(root / 'apps/linux-client'))
from saved_connections import SavedConnections, normalize

identity = 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'
row = dict(peerId=identity, name='办公电脑', address='192.168.8.20:42424')
with tempfile.TemporaryDirectory() as directory:
    path = Path(directory) / 'saved.json'
    def write(path, values): path.write_text(json.dumps(values), encoding='utf-8')
    store = SavedConnections(path, write)
    store.save(dict(row, invitation='must-not-persist'))
    assert SavedConnections(path, write).entries == [row]
    store.save(dict(row, name='家里的 Windows', address='10.0.0.20:42424'))
    assert len(store.entries) == 1
    before = store.entries[:]
    def fail(*_): raise OSError('disk full')
    store.writer = fail
    for operation in (lambda: store.save(row), lambda: store.remove(identity)):
        try: operation(); raise AssertionError('expected save failure')
        except ValueError: pass
        assert store.entries == before
    path.write_text(json.dumps([None, {}, row, dict(row, peerId='host'), row]), encoding='utf-8')
    assert SavedConnections(path, write).entries == [row]
    store.writer = write; store.remove(identity)
    assert SavedConnections(path, write).entries == []
    assert normalize(dict(row, address='[2001:0db8::1]:42424'))['address'] == '[2001:db8::1]:42424'
    for address in ('localhost:42424', '127.1:42424', '[1.2.3.4]:42424', '1.2.3.4:0', '1.2.3.4:65536'):
        try: normalize(dict(row, address=address)); raise AssertionError(address)
        except ValueError: pass

source = ast.parse((root / 'tests/fixtures/legacy_linux_manager.py').read_text(encoding='utf-8'))
manager = next(node for node in source.body if isinstance(node, ast.ClassDef) and node.name == 'Manager')
manager.body = [node for node in manager.body if isinstance(node, ast.FunctionDef) and node.name in {'connect_saved', 'saved_connected'}]
manager.bases = []
peer = dict(device_id=identity, connection_id='quic:' + identity, address='192.168.8.20:42424')
inventory, jobs, callbacks, calls, notices, sensitive = [peer], [], [], [], [], []
def action(op, **fields):
    calls.append((op, fields))
    if op == 'status': return dict(peers=inventory)
    if op == 'address': peer['address'] = fields['address']; return dict(ok=True)
    raise AssertionError(op)
namespace = dict(peers=lambda: inventory,
    network=SimpleNamespace(ensure=lambda: None, action=action),
    threading=SimpleNamespace(Thread=lambda target, daemon: SimpleNamespace(start=lambda: jobs.append(target))),
    GLib=SimpleNamespace(idle_add=lambda callback, *args: callbacks.append((callback, args))))
exec(compile(ast.Module(body=[manager], type_ignores=[]), '<production manager>', 'exec'), namespace)
state = namespace['Manager']()
state.window = object(); state.connection_switch = None; state.selected = 'old-device'
state.saved_connections = SimpleNamespace(entries=[dict(row, address='10.0.0.20:42424')])
state.saved_selector = SimpleNamespace(get_active_id=lambda: identity)
state.selector = SimpleNamespace(set_active_id=lambda value: setattr(state, 'selected', value))
state.connection_controls = [SimpleNamespace(set_sensitive=sensitive.append)]
state.set_notice = notices.append; state.refresh_peers = lambda: None
state.fill_connection_editor = lambda: None; state.load = lambda _: calls.append(('load', {}))
state.connect_saved(); state.connect_saved()
assert len(jobs) == 1 and state.selected == 'old-device', 'switches must serialize without blocking GTK'
jobs.pop()()
assert [call[0] for call in calls] == ['status', 'address'] and state.selected == 'old-device'
callback, args = callbacks.pop(); assert callback(*args) is False
assert state.selected == peer['connection_id'] and state.connection_switch is None and sensitive == [False, True]
# A revoked bookmark must not issue an address mutation or select a device.
inventory.clear(); calls.clear(); state.selected = 'old-device'
state.connect_saved(); jobs.pop()(); callback, args = callbacks.pop(); callback(*args)
assert state.selected == 'old-device' and [call[0] for call in calls] == ['status']
assert '重新配对' in notices[-1]
# Address persistence failure must leave the previous selection usable.
inventory.append(dict(peer, address='192.168.8.20:42424'))
def fail_address(op, **fields):
    if op == 'address': raise OSError('address write failed')
    return action(op, **fields)
namespace['network'].action = fail_address
state.connect_saved(); jobs.pop()(); callback, args = callbacks.pop(); callback(*args)
assert state.selected == 'old-device' and state.connection_switch is None
assert sensitive[-1] is True and 'address write failed' in notices[-1]
# Closing/reopening the manager invalidates old callbacks.
state.connect_saved(); token = state.connection_switch; state.connection_switch = None
count = len(notices); state.saved_connected(token, identity, None)
assert len(notices) == count and state.selected == 'old-device'
print('PASS saved-device persistence, validation, failed-write retention, serialized switching and revoked/stale callback guards')
