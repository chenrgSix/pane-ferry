"""Private GNOME monitor capture bridge. Never changes the source display layout.

Mutter owns monitor identity and provides PipeWire nodes. The Agent owns the
paired-device decision and the separate, user-authorized EIS input lease.
"""
import json
import sys


def value(item):
    return item.unpack() if hasattr(item, 'unpack') else item


def monitor_list(state):
    _serial, physical, logical, properties = state
    layout = value(properties.get('layout-mode', 1))
    placement = {}
    for x, y, scale, transform, primary, specs, _props in logical:
        for spec in specs:
            placement[spec[0]] = (x, y, scale, transform, primary)
    result = []
    for spec, modes, properties in physical:
        connector, vendor, product, _serial = spec
        # Mutter RecordVirtual outputs are temporary capture surfaces, not
        # desktop sources. Match their compositor identity, never a user-facing
        # display name or resolution. Keep normal/headless outputs available.
        # start() uses this same inventory, so stale IDs cannot bypass the rule.
        if (connector.startswith('Meta-') and connector[5:].isascii()
                and connector[5:].isdigit() and vendor == 'MetaVendor'
                and product == 'Virtual remote monitor'):
            continue
        place = placement.get(connector)
        if not place:
            continue
        current = next((mode for mode in modes if value(mode[6].get('is-current', False))), None)
        if not current:
            continue
        x, y, scale, transform, primary = place
        width, height = current[1:3]
        if transform in (1, 3, 5, 7):
            width, height = height, width
        logical_width = round(width / scale) if layout == 1 else width
        logical_height = round(height / scale) if layout == 1 else height
        if (not connector or not connector.isascii()
                or not connector.replace('-', '').replace('_', '').isalnum()
                or len(connector) > 90 or not 0 < width <= 4096 or not 0 < height <= 4096
                or width * height > 8_000_000
                or not 0 < logical_width <= 8192 or not 0 < logical_height <= 8192):
            continue
        name = value(properties.get('display-name', product or connector))
        result.append({'id': connector, 'name': (str(name).strip() or connector)[:128],
                       'width': width, 'height': height, 'logical_width': logical_width,
                       'logical_height': logical_height, 'x': x, 'y': y, 'primary': bool(primary)})
    return sorted(result, key=lambda monitor: (not monitor['primary'], monitor['id']))[:16]


def main():
    import gi
    gi.require_version('Gio', '2.0')
    from gi.repository import Gio, GLib

    bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
    cast = 'org.gnome.Mutter.ScreenCast'
    display = 'org.gnome.Mutter.DisplayConfig'
    session = None
    text_token = None

    def call(name, path, interface, method, args=None, timeout=3000):
        return bus.call_sync(name, path, interface, method, args, None,
                             Gio.DBusCallFlags.NO_AUTO_START, timeout, None).unpack()

    def monitors():
        return monitor_list(call(display, '/org/gnome/Mutter/DisplayConfig', display,
                                 'GetCurrentState'))

    def release_text():
        nonlocal text_token
        if text_token:
            token, text_token = text_token, None
            try:
                call('io.paneferry.Windows', '/io/paneferry/Windows', 'io.paneferry.Windows1',
                     'Call', GLib.Variant('(s)', (json.dumps({'op': 'release', 'token': token}),)))
            except GLib.Error:
                pass

    def stop():
        nonlocal session
        if session:
            try:
                call(cast, session, cast + '.Session', 'Stop')
            except GLib.Error:
                pass
            session = None
        release_text()

    def start(connector):
        nonlocal session
        monitor = next((item for item in monitors() if item['id'] == connector), None)
        if not monitor:
            raise ValueError('desktop_monitor_unavailable')
        stop()
        session = call(cast, '/org/gnome/Mutter/ScreenCast', cast, 'CreateSession',
                       GLib.Variant('(a{sv})', ({},)))[0]
        try:
            stream = call(cast, session, cast + '.Session', 'RecordMonitor',
                          GLib.Variant('(sa{sv})', (connector, {'cursor-mode': GLib.Variant('u', 0)})))[0]
            nodes = []
            signal = bus.signal_subscribe(cast, cast + '.Stream', 'PipeWireStreamAdded',
                                          stream, None, Gio.DBusSignalFlags.NONE,
                                          lambda _c, _s, _p, _i, _n, params: nodes.append(params.unpack()[0]))
            loop = GLib.MainLoop()
            poll = GLib.timeout_add(10, lambda: (loop.quit(), False)[1] if nodes else True)
            timeout = GLib.timeout_add(6000, lambda: (loop.quit(), False)[1])
            try:
                call(cast, session, cast + '.Session', 'Start')
                loop.run()
            finally:
                bus.signal_unsubscribe(signal)
                for source in (poll, timeout):
                    if GLib.MainContext.default().find_source_by_id(source):
                        GLib.source_remove(source)
            if not nodes:
                raise ValueError('desktop_capture_unavailable')
            return {'node': nodes[0], 'monitor': monitor}
        except Exception:
            stop()
            raise

    def text(request):
        nonlocal text_token
        path, interface = '/io/paneferry/Windows', 'io.paneferry.Windows1'
        try:
            if not text_token:
                answer = json.loads(call('io.paneferry.Windows', path, interface, 'Call',
                                         GLib.Variant('(s)', (json.dumps({'op': 'acquire_desktop'}),)))[0])
                if 'error' in answer:
                    raise ValueError(answer['error'])
                text_token = answer['result']['token']
            request['token'] = text_token
            answer = json.loads(call('io.paneferry.Windows', path, interface, 'Call',
                                     GLib.Variant('(s)', (json.dumps(request, ensure_ascii=False),)),
                                     timeout=10000 if request['op'] == 'commit_text' else 3000)[0])
        except GLib.Error:
            raise ValueError('wayland_text_unavailable') from None
        if 'error' in answer:
            raise ValueError(answer['error'] if answer['error'] in ('desktop_locked', 'clipboard_copy_failed')
                             else 'wayland_text_unavailable')
        return answer['result']

    try:
        while True:
            raw = sys.stdin.buffer.readline(65537)
            if not raw or len(raw) > 65536 or not raw.endswith(b'\n'):
                break
            try:
                request = json.loads(raw)
                op = request['op']
                if op == 'list':
                    result = {'monitors': monitors()}
                elif op == 'start':
                    result = start(request['monitor_id'])
                elif op == 'stop':
                    stop(); result = {}
                elif op in ('commit_text', 'set_text', 'get_text'):
                    result = text(request)
                else:
                    raise ValueError('invalid_request')
                answer = {'result': result}
            except (GLib.Error, KeyError, TypeError, ValueError) as error:
                code = str(error) if isinstance(error, ValueError) else 'desktop_capture_unavailable'
                allowed = {'desktop_monitor_unavailable', 'desktop_capture_unavailable',
                           'wayland_text_unavailable', 'desktop_locked', 'clipboard_copy_failed'}
                answer = {'error': code if code in allowed else 'desktop_capture_unavailable'}
            print(json.dumps(answer, ensure_ascii=False, separators=(',', ':')), flush=True)
    finally:
        stop()


if __name__ == '__main__':
    main()
