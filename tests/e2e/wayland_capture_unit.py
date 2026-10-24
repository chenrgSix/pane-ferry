#!/usr/bin/env python3
"""Exercise the real helper loop with delayed nodes and disappearing transients."""
import importlib.util
import io
import json
from pathlib import Path
import sys
from types import SimpleNamespace, ModuleType
from unittest.mock import patch

source = Path(__file__).resolve().parents[2] / 'apps/linux-agent/src/wayland_helper.py'


def run(deliver=True, timeout=False, disappear=False):
    callbacks, ready, stopped, unsubscribed = {}, [], [], []
    clock = [0]
    class Connection:
        def signal_subscribe(self, *args):
            key = len(callbacks) + 1
            callbacks[key] = args[-1]
            return key
        def signal_unsubscribe(self, key):
            unsubscribed.append(key)
    connection = Connection()
    context = SimpleNamespace(pending=lambda: bool(ready), iteration=lambda _: ready.pop(0)())
    glib = SimpleNamespace(Variant=lambda _signature, value: value, Error=RuntimeError,
                           MainContext=SimpleNamespace(default=lambda: context))
    gio = SimpleNamespace(bus_get_sync=lambda *_: connection, BusType=SimpleNamespace(SESSION=1),
                          DBusSignalFlags=SimpleNamespace(NONE=0))
    gi = ModuleType('gi'); gi.require_version = lambda *_: None
    repo = ModuleType('gi.repository'); repo.Gio = gio; repo.GLib = glib
    with patch.dict(sys.modules, {'gi': gi, 'gi.repository': repo}):
        spec = importlib.util.spec_from_file_location('capture_fixture', source)
        module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
    created, scopes = [], []
    def call(_bus, path, _interface, method, *_args, **_kw):
        if method == 'CreateSession':
            name = f'/capture/{len(created)+1}'; created.append(name); return (name,)
        if method == 'RecordWindow': return (path + '/stream',)
        if method == 'Stop': stopped.append(path)
        if method == 'Start' and deliver:
            key = int(path.rsplit('/', 1)[1])
            ready.append(lambda: callbacks[key](None, None, None, None, None, SimpleNamespace(unpack=lambda: (100 + key,))))
        return ()
    def bridge(request):
        if request['op'] == 'acquire': return {'token': 'owned'}
        if request['op'] == 'release': return {}
        assert request['op'] == 'scope'
        scopes.append(True)
        # The fourth explicit capture sees a child disappear between requests.
        ids = [1] if len(scopes) >= (4 if disappear else 5) else [1, 2]
        return [{'window_id': key, 'id': str(key)} for key in ids]
    requests = [{'op': 'acquire'}, {'op': 'capture', 'window_id': 1},
                {'op': 'capture', 'window_id': 2}, {'op': 'capture', 'window_id': 1},
                {'op': 'capture', 'window_id': 2}, {'op': 'scope'}]
    class Input(io.BytesIO):
        reads = 0
        def readline(self, *args):
            self.reads += 1
            if timeout and self.reads == 4: clock[0] = 4
            return super().readline(*args)
    reader = SimpleNamespace(buffer=Input(''.join(json.dumps(r)+'\n' for r in requests).encode()))
    output = io.StringIO()
    module.call = call; module.bridge = bridge
    module.time = SimpleNamespace(monotonic=lambda: clock[0])
    with patch.object(sys, 'stdin', reader), patch.object(sys, 'stdout', output), patch.object(sys, 'argv', ['fixture']):
        module.main()
    answers = [json.loads(line) for line in output.getvalue().splitlines()]
    assert answers[1]['result'] == {'pending': True}, answers
    assert answers[2]['result'] == {'pending': True}, answers
    assert len(stopped) == len(created) == len(unsubscribed) == 2
    assert len(set(stopped)) == 2, stopped
    assert 'result' in answers[-1], 'one failed child must not stop the helper'
    if timeout:
        assert answers[3]['error'] == answers[4]['error'] == 'wayland_capture_unavailable'
    else:
        assert answers[3]['result'] == {'node': 101}
        if disappear: assert answers[4]['error'] == 'window_unavailable'
        else: assert answers[4]['result'] == {'node': 102}, 'each callback belongs to its own capture'


run()
run(disappear=True)
run(deliver=False, timeout=True)
print('PASS: nonblocking PipeWire startup, independent node callbacks, disappearing children, timeout and exact resource cleanup')
