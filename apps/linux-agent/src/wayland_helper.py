"""Bounded, private control bridge. Pixels stay in PipeWire/GStreamer, never JSON."""
import json
import sys
import time
import gi
gi.require_version('Gio', '2.0')
from gi.repository import Gio, GLib

BUS = 'io.paneferry.Windows'
PATH = '/io/paneferry/Windows'
IFACE = 'io.paneferry.Windows1'
CAST = 'org.gnome.Mutter.ScreenCast'
conn = Gio.bus_get_sync(Gio.BusType.SESSION, None)


def call(bus, path, interface, method, args=None, timeout=2000):
    return conn.call_sync(bus, path, interface, method, args, None,
                          Gio.DBusCallFlags.NO_AUTO_START, timeout, None).unpack()


def bridge(request):
    try:
        answer = json.loads(call(BUS, PATH, IFACE, 'Call', GLib.Variant('(s)', (json.dumps(request, ensure_ascii=False),)),
                                 timeout=10000 if request.get('op') == 'commit_text' else 2000)[0])
    except GLib.Error:
        raise ValueError('wayland_bridge_unavailable') from None
    if 'error' in answer:
        raise ValueError(answer['error'])
    return answer['result']


def main():
    if len(sys.argv) == 2 and sys.argv[1] == 'list':
        try:
            print(json.dumps({'windows': bridge({'op': 'list'}), 'supported': True}))
        except ValueError as error:
            print(json.dumps({'windows': [], 'supported': False, 'error': str(error)}))
        return
    token = None
    captures = {}
    def stop_capture(key):
        capture = captures.pop(key, None)
        if capture:
            session, _node, signal, _started = capture
            conn.signal_unsubscribe(signal)
            try: call(CAST, session, CAST+'.Session', 'Stop')
            except GLib.Error: pass
    try:
        while True:
            raw = sys.stdin.buffer.readline(65537)
            if not raw: break
            if len(raw) > 65536 or not raw.endswith(b'\n'): break
            try:
                # Deliver node-ready signals without running a nested, seconds-
                # long main loop on the controller's synchronous input path.
                context = GLib.MainContext.default()
                for _ in range(32):
                    if not context.pending(): break
                    context.iteration(False)
                request = json.loads(raw)
                op = request['op']
                if op == 'acquire':
                    if token: raise ValueError('invalid_request')
                    result = bridge(request); token = result['token']
                elif op == 'capture':
                    window = next((w for w in bridge({'op': 'scope', 'token': token}) if w['window_id'] == request['window_id']), None)
                    if not window: raise ValueError('window_unavailable')
                    key = window['window_id']
                    if key not in captures:
                        session = call(CAST, '/org/gnome/Mutter/ScreenCast', CAST, 'CreateSession', GLib.Variant('(a{sv})', ({},)))[0]
                        signal = None
                        try:
                            stream = call(CAST, session, CAST+'.Session', 'RecordWindow', GLib.Variant('(a{sv})', ({'window-id': GLib.Variant('t', int(window['id'])), 'cursor-mode': GLib.Variant('u', 0)},)))[0]
                            node = []
                            signal = conn.signal_subscribe(CAST, CAST+'.Stream', 'PipeWireStreamAdded', stream, None, Gio.DBusSignalFlags.NONE,
                                lambda _c, _s, _p, _i, _n, params, nodes=node: nodes.append(params.unpack()[0]))
                            call(CAST, session, CAST+'.Session', 'Start')
                            captures[key] = (session, node, signal, time.monotonic())
                        except Exception:
                            if signal is not None: conn.signal_unsubscribe(signal)
                            try: call(CAST, session, CAST+'.Session', 'Stop')
                            except GLib.Error: pass
                            raise
                    _session, nodes, _signal, started = captures[key]
                    if not nodes and time.monotonic() - started > 3:
                        stop_capture(key)
                        raise ValueError('wayland_capture_unavailable')
                    result = {'node': nodes[0]} if nodes else {'pending': True}
                elif op == 'stop_capture':
                    stop_capture(request['window_id'])
                    result = {}
                else:
                    request['token'] = token
                    result = bridge(request)
                    if op == 'scope':
                        live = {w['window_id'] for w in result}
                        for key in list(captures):
                            if key not in live: stop_capture(key)
                answer = {'result': result}
            except (ValueError, KeyError, TypeError, GLib.Error) as error:
                code = str(error) if isinstance(error, ValueError) else 'wayland_capture_unavailable'
                answer = {'error': code if len(code) < 80 and all(c.islower() or c == '_' for c in code) else 'wayland_bridge_failed'}
            print(json.dumps(answer, ensure_ascii=False, separators=(',', ':')), flush=True)
    finally:
        for key in list(captures): stop_capture(key)
        if token:
            try: bridge({'op': 'release', 'token': token})
            except ValueError: pass


if __name__ == '__main__':
    main()
