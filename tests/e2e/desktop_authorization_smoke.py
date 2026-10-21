#!/usr/bin/env python3
"""Linux opt-in: real private D-Bus + libei/libeis, no user desktop or consent UI.

Run with /usr/bin/python3. The test creates a fresh session bus before importing
Gio, and uses its own HOME. Consent responses are from the test-owned portal.
"""
import ctypes as C
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time

if '--private-bus' not in sys.argv:
    with tempfile.TemporaryDirectory(prefix='remoteapp-private-bus-') as folder:
        config = Path(folder)/'session.conf'
        config.write_text('''<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
"http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:tmpdir=/tmp</listen>
<policy context="default"><allow send_destination="*"/><allow receive_sender="*"/><allow own="*"/></policy>
</busconfig>''')
        env = {key: value for key, value in os.environ.items()
               if key not in ('DISPLAY', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER')}
        raise SystemExit(subprocess.run(['dbus-run-session', '--config-file='+str(config), '--',
            '/usr/bin/python3', str(Path(__file__).resolve()), '--private-bus'], env=env).returncode)

from gi.repository import Gio, GLib

SOURCE = Path(__file__).resolve().parents[2] / 'apps/linux-agent/src/desktop_input.py'
SERVICE = 'org.freedesktop.portal.Desktop'
PATH = '/org/freedesktop/portal/desktop'
REMOTE = 'org.freedesktop.portal.RemoteDesktop'
APP_ID = 'io.lanremoteapp.Agent'


def until(predicate, message):
    deadline = time.monotonic() + 6
    while time.monotonic() < deadline:
        while GLib.MainContext.default().pending():
            GLib.MainContext.default().iteration(False)
        value = predicate()
        if value:
            return value
        time.sleep(.002)
    raise AssertionError(message)


class Receiver:
    """Test-owned EIS receiver using the actual system libeis ABI."""
    def __init__(self):
        self.lib = C.CDLL('libeis.so.1')
        p, i, u, b, d = C.c_void_p, C.c_int, C.c_uint32, C.c_bool, C.c_double
        signatures = {
            'eis_new': (p, [p]), 'eis_unref': (p, [p]), 'eis_setup_backend_fd': (i, [p]),
            'eis_backend_fd_add_client': (i, [p]), 'eis_get_fd': (i, [p]),
            'eis_dispatch': (None, [p]), 'eis_get_event': (p, [p]),
            'eis_event_unref': (p, [p]), 'eis_event_get_type': (i, [p]),
            'eis_event_get_client': (p, [p]), 'eis_event_get_seat': (p, [p]),
            'eis_event_seat_has_capability': (b, [p, i]),
            'eis_client_connect': (None, [p]), 'eis_client_new_seat': (p, [p, C.c_char_p]),
            'eis_seat_configure_capability': (None, [p, i]), 'eis_seat_add': (None, [p]),
            'eis_seat_new_device': (p, [p]), 'eis_device_configure_type': (None, [p, i]),
            'eis_device_configure_name': (None, [p, C.c_char_p]),
            'eis_device_configure_capability': (None, [p, i]),
            'eis_device_new_region': (p, [p]), 'eis_region_set_size': (None, [p, u, u]),
            'eis_region_set_offset': (None, [p, u, u]),
            'eis_region_set_physical_scale': (None, [p, d]), 'eis_region_add': (None, [p]),
            'eis_device_add': (None, [p]), 'eis_device_resume': (None, [p]),
            'eis_device_pause': (None, [p]),
            'eis_device_remove': (None, [p]),
            'eis_event_keyboard_get_key': (u, [p]),
            'eis_event_keyboard_get_key_is_press': (b, [p]),
            'eis_event_pointer_get_absolute_x': (d, [p]),
            'eis_event_pointer_get_absolute_y': (d, [p]),
            'eis_event_button_get_button': (u, [p]), 'eis_event_button_get_is_press': (b, [p]),
            'eis_event_scroll_get_discrete_dx': (i, [p]), 'eis_event_scroll_get_discrete_dy': (i, [p]),
        }
        for name, (result, arguments) in signatures.items():
            function = getattr(self.lib, name)
            function.restype, function.argtypes = result, arguments
        self.context = self.lib.eis_new(None)
        assert self.lib.eis_setup_backend_fd(self.context) == 0
        self.events, self.devices, self.bound = [], [], set()
        GLib.io_add_watch(self.lib.eis_get_fd(self.context), GLib.IO_IN, self.dispatch)

    def add_client(self):
        fd = self.lib.eis_backend_fd_add_client(self.context)
        assert fd >= 0
        return fd

    def dispatch(self, *args):
        self.lib.eis_dispatch(self.context)
        while event := self.lib.eis_get_event(self.context):
            try:
                kind = self.lib.eis_event_get_type(event)
                if kind == 1:
                    client = self.lib.eis_event_get_client(event)
                    self.lib.eis_client_connect(client)
                    seat = self.lib.eis_client_new_seat(client, b'test-seat')
                    for capability in (1, 2, 4, 16, 32):
                        self.lib.eis_seat_configure_capability(seat, capability)
                    self.lib.eis_seat_add(seat)
                elif kind == 3:
                    seat = self.lib.eis_event_get_seat(event)
                    if seat in self.bound or not all(self.lib.eis_event_seat_has_capability(event, cap)
                                                     for cap in (1, 2, 4, 16, 32)):
                        continue
                    self.bound.add(seat)
                    self.last_seat = seat
                    self.add_devices(seat)
                elif kind == 700:
                    self.events.append(('key', self.lib.eis_event_keyboard_get_key(event),
                                        bool(self.lib.eis_event_keyboard_get_key_is_press(event))))
                elif kind == 400:
                    self.events.append(('motion', self.lib.eis_event_pointer_get_absolute_x(event),
                                        self.lib.eis_event_pointer_get_absolute_y(event)))
                elif kind == 500:
                    self.events.append(('button', self.lib.eis_event_button_get_button(event),
                                        bool(self.lib.eis_event_button_get_is_press(event))))
                elif kind == 603:
                    self.events.append(('scroll', self.lib.eis_event_scroll_get_discrete_dx(event),
                                        self.lib.eis_event_scroll_get_discrete_dy(event)))
            finally:
                self.lib.eis_event_unref(event)
        return True

    def add_devices(self, seat):
        for capabilities in ((4,), (1, 2, 16, 32)):
            device = self.lib.eis_seat_new_device(seat)
            self.lib.eis_device_configure_type(device, 1)
            self.lib.eis_device_configure_name(device, b'test-input')
            for capability in capabilities:
                self.lib.eis_device_configure_capability(device, capability)
            if 2 in capabilities:
                region = self.lib.eis_device_new_region(device)
                self.lib.eis_region_set_offset(region, 0, 0)
                self.lib.eis_region_set_size(region, 2048, 1200)
                self.lib.eis_region_set_physical_scale(region, 1)
                self.lib.eis_region_add(region)
            self.lib.eis_device_add(device)
            self.lib.eis_device_resume(device)
            self.devices.append(device)


XML = '''<node>
<interface name="org.freedesktop.host.portal.Registry"><method name="Register"><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/></method></interface>
<interface name="org.freedesktop.portal.RemoteDesktop">
<property name="version" type="u" access="read"/>
<method name="CreateSession"><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
<method name="SelectDevices"><arg type="o" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
<method name="Start"><arg type="o" direction="in"/><arg type="s" direction="in"/><arg type="a{sv}" direction="in"/><arg type="o" direction="out"/></method>
<method name="ConnectToEIS"><arg type="o" direction="in"/><arg type="a{sv}" direction="in"/><arg type="h" direction="out"/></method>
</interface></node>'''
SESSION_XML = '''<node><interface name="org.freedesktop.portal.Session">
<method name="Close"/><signal name="Closed"><arg type="a{sv}"/></signal></interface></node>'''


class FixturePortal:
    def __init__(self):
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.name('RequestName', GLib.Variant('(su)', (SERVICE, 0)))
        self.calls, self.prompts, self.valid, self.selected = [], 0, set(), {}
        self.pending, self.sessions = [], []
        self.receiver = Receiver()
        self.registrations = []
        for interface in Gio.DBusNodeInfo.new_for_xml(XML).interfaces:
            self.bus.register_object(PATH, interface, self.called,
                lambda *args: GLib.Variant('u', 2), None)

    def name(self, method, parameters):
        return self.bus.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus',
            'org.freedesktop.DBus', method, parameters, None, Gio.DBusCallFlags.NONE, 1000, None)

    def respond(self, sender, path, code, values):
        self.bus.emit_signal(sender, path, 'org.freedesktop.portal.Request', 'Response',
                             GLib.Variant('(ua{sv})', (code, values)))

    def called(self, bus, sender, path, interface, method, parameters, invocation):
        args = parameters.unpack()
        self.calls.append(method)
        if method == 'Register':
            assert args == (APP_ID, {})
            assert sender not in self.registrations, 'duplicate registration'
            self.registrations.append(sender)
            invocation.return_value(None)
            return
        if method == 'ConnectToEIS':
            fd = self.receiver.add_client()
            fds = Gio.UnixFDList.new()
            index = fds.append(fd)
            os.close(fd)
            invocation.return_value_with_unix_fd_list(GLib.Variant('(h)', (index,)), fds)
            return
        options = args[-1]
        token = options['handle_token']
        prefix = sender[1:].replace('.', '_')
        request = '/org/freedesktop/portal/desktop/request/' + prefix + '/' + token
        invocation.return_value(GLib.Variant('(o)', (request,)))
        result = {}
        if method == 'CreateSession':
            session = '/org/freedesktop/portal/desktop/session/' + prefix + '/' + options['session_handle_token']
            self.sessions.append((sender, session))
            self.bus.register_object(session, Gio.DBusNodeInfo.new_for_xml(SESSION_XML).interfaces[0],
                lambda bus, sender, path, interface, method, parameters, invocation: invocation.return_value(None), None, None)
            result = {'session_handle': GLib.Variant('s', session)}
        elif method == 'SelectDevices':
            assert options['types'] == 3 and options['persist_mode'] == 2
            assert set(options) <= {'types', 'persist_mode', 'restore_token', 'handle_token'}
            self.selected[args[0]] = options.get('restore_token') in self.valid
            self.valid.discard(options.get('restore_token'))
        elif method == 'Start':
            assert args[1] == ''
            if not self.selected[args[0]]:
                self.prompts += 1
                self.pending.append((sender, request))
                return
            result = self.grant()
        GLib.idle_add(lambda: (self.respond(sender, request, 0, result), False)[1])

    def grant(self):
        token = 'fixture-rotated-' + str(time.monotonic_ns())
        self.valid.add(token)
        return {'devices': GLib.Variant('u', 3), 'restore_token': GLib.Variant('s', token)}

    def consent(self, allow=True):
        assert len(self.pending) == 1
        sender, request = self.pending.pop()
        self.respond(sender, request, 0 if allow else 1, self.grant() if allow else {})

    def revoke(self):
        self.valid.clear()
        sender, session = self.sessions[-1]
        self.bus.emit_signal(sender, session, 'org.freedesktop.portal.Session', 'Closed',
                             GLib.Variant('(a{sv})', ({},)))


class Helper:
    def __init__(self, root):
        env = dict(os.environ, HOME=str(root), XDG_DATA_HOME=str(root/'data'))
        self.process = subprocess.Popen(['/usr/bin/python3', '-I', '-u', str(SOURCE),
            str(root), '/usr/bin/true'], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE)
        os.set_blocking(self.process.stdout.fileno(), False)
        self.pending = bytearray()

    def call(self, **request):
        self.process.stdin.write(json.dumps(request).encode()+b'\n')
        self.process.stdin.flush()

        def read():
            try:
                chunk = os.read(self.process.stdout.fileno(), 4096)
                if chunk: self.pending.extend(chunk)
            except BlockingIOError:
                pass
            if self.process.poll() is not None:
                raise AssertionError('helper exited: ' + self.process.stderr.read().decode()[-2000:])
            if b'\n' in self.pending:
                line, _, rest = self.pending.partition(b'\n')
                self.pending[:] = rest
                return json.loads(line)
        return until(read, 'helper response')

    def ready(self):
        until(lambda: self.call(type='status')['status'] == 'ready', 'EIS readiness')

    def close(self):
        self.process.stdin.close()
        until(lambda: self.process.poll() is not None, 'helper EOF cleanup')
        stderr = self.process.stderr.read().decode()
        assert self.process.returncode == 0 and 'Traceback' not in stderr, stderr


with tempfile.TemporaryDirectory(prefix='remoteapp-portal-test-') as temporary:
    root = Path(temporary)
    fixture = FixturePortal()
    helper = Helper(root)
    assert helper.call(type='ensure')['status'] == 'pending'
    until(lambda: fixture.prompts == 1, 'first consent request')
    assert helper.call(type='ensure')['status'] == 'pending'
    assert helper.call(type='key', owner=1, code=38, pressed=True)['error'] == 'desktop_authorization_pending'
    assert fixture.calls.count('CreateSession') == 1
    fixture.consent()
    helper.ready()
    assert fixture.receiver.events == [], 'pending key was replayed'
    for owner in (1, 2):
        assert 'error' not in helper.call(type='key', owner=owner, code=38, pressed=True)
    helper.call(type='release', owner=1)
    until(lambda: fixture.receiver.events == [('key', 30, True)], 'shared key ownership')
    helper.call(type='release', owner=2)
    until(lambda: fixture.receiver.events[-1:] == [('key', 30, False)], 'last owner key release')
    for pressed in (True, False):
        assert 'error' not in helper.call(type='pointer', owner=3, x=180, y=130, button=1, pressed=pressed)
    helper.call(type='pointer', owner=3, x=180, y=130, button=4, pressed=False)
    until(lambda: ('scroll', 0, -120) in fixture.receiver.events, 'wheel delivery')
    assert ('motion', 180.0, 130.0) in fixture.receiver.events
    assert ('button', 272, True) in fixture.receiver.events and ('button', 272, False) in fixture.receiver.events
    assert fixture.prompts == 1
    for device in fixture.receiver.devices[-2:]:
        fixture.receiver.lib.eis_device_remove(device)
    fixture.receiver.dispatch()
    until(lambda: helper.call(type='status')['status'] == 'paused', 'device pause not observed')
    assert helper.call(type='key', owner=3, code=38, pressed=True)['error'] == 'desktop_input_paused'
    before_resume = len(fixture.receiver.events)
    fixture.receiver.add_devices(fixture.receiver.last_seat)
    fixture.receiver.dispatch()
    helper.ready()
    assert len(fixture.receiver.events) == before_resume, 'paused input was replayed'
    helper.call(type='release', owner=3)
    helper.call(type='ensure')
    assert fixture.calls.count('CreateSession') == 1, 'new application/reconnect created another portal'
    path = root/'desktop-authorization.json'
    first = json.loads(path.read_text())['restore_token']
    assert path.stat().st_mode & 0o777 == 0o600
    helper.close()
    helper = Helper(root)
    helper.call(type='ensure')
    helper.ready()
    assert fixture.prompts == 1 and fixture.calls.count('CreateSession') == 2, 'restore prompted again'
    assert json.loads(path.read_text())['restore_token'] != first, 'single-use token not rotated'
    fixture.revoke()
    until(lambda: helper.call(type='status')['status'] == 'unavailable', 'revocation not observed')
    helper.call(type='ensure')
    until(lambda: fixture.prompts == 2, 'revocation did not require fresh consent')
    fixture.consent(False)
    until(lambda: helper.call(type='status')['status'] == 'denied', 'cancel not observed')
    assert helper.call(type='key', owner=3, code=38, pressed=True)['error'] == 'desktop_authorization_denied'
    assert fixture.prompts == 2, 'typing retriggered consent'
    helper.call(type='ensure')
    until(lambda: fixture.prompts == 3, 'explicit reconnect cannot retry cancelled consent')
    fixture.consent()
    helper.ready()
    # A service restart invalidates registration, but the saved grant survives.
    fixture.name('ReleaseName', GLib.Variant('(s)', (SERVICE,)))
    until(lambda: helper.call(type='status')['status'] == 'unavailable', 'portal restart not observed')
    fixture.registrations.clear()
    fixture.name('RequestName', GLib.Variant('(su)', (SERVICE, 0)))
    helper.call(type='ensure')
    helper.ready()
    assert fixture.prompts == 3, 'portal restart failed to restore consent'
    helper.close()
    assert set(fixture.calls) <= {'Register', 'CreateSession', 'SelectDevices', 'Start', 'ConnectToEIS'}
    print('PASS: one shared consent, real EIS keys/pointer/buttons/scroll, scoped releases, token rotation, restart restore, revocation and cancellation; no ScreenCast/Clipboard calls')
