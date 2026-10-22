"""Bounded, private control bridge. Pixels stay in PipeWire/GStreamer, never JSON."""
import json
import sys
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
        answer = json.loads(call(BUS, PATH, IFACE, 'Call', GLib.Variant('(s)', (json.dumps(request, ensure_ascii=False),)))[0])
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
    try:
        while True:
            raw = sys.stdin.buffer.readline(65537)
            if not raw: break
            if len(raw) > 65536 or not raw.endswith(b'\n'): break
            try:
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
                        try:
                            stream = call(CAST, session, CAST+'.Session', 'RecordWindow', GLib.Variant('(a{sv})', ({'window-id': GLib.Variant('t', int(window['id'])), 'cursor-mode': GLib.Variant('u', 0)},)))[0]
                            node = []
                            signal = conn.signal_subscribe(CAST, CAST+'.Stream', 'PipeWireStreamAdded', stream, None, Gio.DBusSignalFlags.NONE,
                                lambda _c, _s, _p, _i, _n, params: node.append(params.unpack()[0]))
                            loop = GLib.MainLoop()
                            poll = GLib.timeout_add(10, lambda: (loop.quit(), False)[1] if node else True)
                            timeout = GLib.timeout_add(3000, lambda: (loop.quit(), False)[1])
                            try:
                                call(CAST, session, CAST+'.Session', 'Start')
                                loop.run()
                            finally:
                                conn.signal_unsubscribe(signal)
                                for source in (poll, timeout):
                                    if GLib.MainContext.default().find_source_by_id(source): GLib.source_remove(source)
                            if not node: raise ValueError('wayland_capture_unavailable')
                            captures[key] = (session, node[0])
                        except Exception:
                            try: call(CAST, session, CAST+'.Session', 'Stop')
                            except GLib.Error: pass
                            raise
                    result = {'node': captures[key][1]}
                elif op == 'stop_capture':
                    session = captures.pop(request['window_id'], None)
                    if session: call(CAST, session[0], CAST+'.Session', 'Stop')
                    result = {}
                else:
                    request['token'] = token
                    result = bridge(request)
                answer = {'result': result}
            except (ValueError, KeyError, TypeError, GLib.Error) as error:
                code = str(error) if isinstance(error, ValueError) else 'wayland_capture_unavailable'
                answer = {'error': code if len(code) < 80 and all(c.islower() or c == '_' for c in code) else 'wayland_bridge_failed'}
            print(json.dumps(answer, ensure_ascii=False, separators=(',', ':')), flush=True)
    finally:
        for session, _ in captures.values():
            try: call(CAST, session, CAST+'.Session', 'Stop')
            except GLib.Error: pass
        if token:
            try: bridge({'op': 'release', 'token': token})
            except ValueError: pass


if __name__ == '__main__':
    main()
