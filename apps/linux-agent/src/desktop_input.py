"""Private, persistent input-only portal broker, embedded in paneferry-agent.

No screen cast or portal clipboard is requested. The Agent validates target
windows before sending coordinates/keycodes over this inherited private pipe.
"""
import ctypes as C
import fcntl
import json
import os
from pathlib import Path
import secrets
import stat
import sys
import tempfile

APP_ID = 'io.lanremoteapp.Agent'
PORTAL = 'org.freedesktop.portal.Desktop'
PATH = '/org/freedesktop/portal/desktop'
REMOTE = 'org.freedesktop.portal.RemoteDesktop'


def atomic_write(path, data):
    fd, name = tempfile.mkstemp(prefix='.' + path.name + '-', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        if os.path.exists(name):
            os.unlink(name)


class TokenStore:
    def __init__(self, root):
        info = root.lstat()
        if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.geteuid() or info.st_mode & 0o077:
            raise ValueError('private state directory required')
        self.path = root / 'desktop-authorization.json'

    def load(self):
        try:
            fd = os.open(self.path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        except FileNotFoundError:
            return None
        with os.fdopen(fd, 'rb') as stream:
            info = os.fstat(stream.fileno())
            if (not stat.S_ISREG(info.st_mode) or info.st_uid != os.geteuid()
                    or info.st_mode & 0o077 or info.st_nlink != 1 or info.st_size > 8192):
                raise ValueError('private authorization file required')
            try:
                value = json.loads(stream.read(8193))
            except (ValueError, UnicodeError):
                return None
        token = value.get('restore_token') if isinstance(value, dict) else None
        return token if (isinstance(value, dict) and value.get('version') == 1 and value.get('app_id') == APP_ID
                         and isinstance(token, str) and 0 < len(token) <= 4096) else None

    def replace(self, token):
        if token is None:
            self.path.unlink(missing_ok=True)
        elif isinstance(token, str) and 0 < len(token) <= 4096:
            atomic_write(self.path, json.dumps({'version': 1, 'app_id': APP_ID,
                                              'restore_token': token}).encode())
        else:
            raise ValueError('invalid authorization result')


class Eis:
    """Small runtime binding to system libei 1.x; all calls run on one thread."""
    POINTER, ABSOLUTE, KEYBOARD, SCROLL, BUTTON = 1, 2, 4, 16, 32

    def __init__(self, fd):
        self.lib = C.CDLL('libei.so.1')
        ptr, integer, uint, boolean, real = C.c_void_p, C.c_int, C.c_uint32, C.c_bool, C.c_double
        signatures = {
            'ei_new_sender': (ptr, [ptr]), 'ei_unref': (ptr, [ptr]),
            'ei_configure_name': (None, [ptr, C.c_char_p]),
            'ei_setup_backend_fd': (integer, [ptr, integer]),
            'ei_get_fd': (integer, [ptr]), 'ei_dispatch': (None, [ptr]),
            'ei_get_event': (ptr, [ptr]), 'ei_event_unref': (ptr, [ptr]),
            'ei_event_get_type': (integer, [ptr]), 'ei_event_get_seat': (ptr, [ptr]),
            'ei_event_get_device': (ptr, [ptr]), 'ei_device_ref': (ptr, [ptr]),
            'ei_device_unref': (ptr, [ptr]), 'ei_device_has_capability': (boolean, [ptr, integer]),
            'ei_device_get_region_at': (ptr, [ptr, real, real]),
            'ei_device_start_emulating': (None, [ptr, uint]),
            'ei_device_frame': (None, [ptr, C.c_uint64]), 'ei_now': (C.c_uint64, [ptr]),
            'ei_device_keyboard_key': (None, [ptr, uint, boolean]),
            'ei_device_pointer_motion_absolute': (None, [ptr, real, real]),
            'ei_device_button_button': (None, [ptr, uint, boolean]),
            'ei_device_scroll_discrete': (None, [ptr, C.c_int32, C.c_int32]),
        }
        for name, (result, arguments) in signatures.items():
            function = getattr(self.lib, name)
            function.restype, function.argtypes = result, arguments
        # The final zero is the varargs sentinel (promoted capability enums).
        self.lib.ei_seat_bind_capabilities.restype = None
        self.lib.ei_seat_bind_capabilities.argtypes = [ptr]
        self.context = self.lib.ei_new_sender(None)
        self.devices, self.active, self.held, self.releases = {}, set(), {}, set()
        self.sequence = 0
        self.disconnected = False
        if not self.context:
            raise RuntimeError('libei context unavailable')
        self.lib.ei_configure_name(self.context, '窗渡 PaneFerry Agent'.encode('utf-8'))
        # libei takes ownership. Give it a duplicate; the caller closes the
        # descriptor received through Gio's UnixFDList on every path.
        if self.lib.ei_setup_backend_fd(self.context, os.dup(fd)) != 0:
            self.close()
            raise RuntimeError('libei connection unavailable')

    @property
    def fd(self):
        return self.lib.ei_get_fd(self.context)

    def dispatch(self):
        self.lib.ei_dispatch(self.context)
        while event := self.lib.ei_get_event(self.context):
            try:
                kind = self.lib.ei_event_get_type(event)
                device = self.lib.ei_event_get_device(event)
                if kind == 2:  # DISCONNECT
                    self.disconnected = True
                elif kind == 3:  # SEAT_ADDED
                    seat = self.lib.ei_event_get_seat(event)
                    self.lib.ei_seat_bind_capabilities(seat, C.c_int(self.POINTER),
                        C.c_int(self.ABSOLUTE), C.c_int(self.KEYBOARD),
                        C.c_int(self.SCROLL), C.c_int(self.BUTTON), C.c_void_p())
                elif kind == 5:  # DEVICE_ADDED
                    self.devices[device] = self.lib.ei_device_ref(device)
                elif kind in (6, 7):  # DEVICE_REMOVED / PAUSED
                    self.active.discard(device)
                    if kind == 6:
                        # Removal resets the device's state. A short pause keeps
                        # it, so releases during a pause must be sent on resume.
                        self.held = {key: owners for key, owners in self.held.items() if key[0] != device}
                        self.releases = {key for key in self.releases if key[0] != device}
                        if device in self.devices:
                            self.lib.ei_device_unref(self.devices.pop(device))
                elif kind == 8:  # DEVICE_RESUMED
                    self.active.add(device)
                    self.sequence = (self.sequence + 1) & 0xffffffff
                    self.lib.ei_device_start_emulating(device, self.sequence)
                    self.lib.ei_device_frame(device, self.lib.ei_now(self.context))
                    self.flush_releases(device)
            finally:
                self.lib.ei_event_unref(event)

    def device(self, capability, point=None):
        for device in self.active:
            if (self.lib.ei_device_has_capability(device, capability)
                    and (point is None or self.lib.ei_device_get_region_at(device, *point))):
                return device
        raise RuntimeError('desktop_input_paused')

    def ready(self):
        return all(any(self.lib.ei_device_has_capability(d, cap) for d in self.active)
                   for cap in (self.KEYBOARD, self.ABSOLUTE, self.BUTTON, self.SCROLL))

    def press(self, owner, device, code, pressed, key):
        identity = (device, code, key)
        owners = self.held.get(identity, set())
        was_pressed = bool(owners)
        if pressed:
            owners.add(owner)
        else:
            owners.discard(owner)
        if owners:
            self.held[identity] = owners
        else:
            self.held.pop(identity, None)
        if was_pressed != bool(owners):
            if device not in self.active:
                if not owners:
                    self.releases.add(identity)
                return
            function = self.lib.ei_device_keyboard_key if key else self.lib.ei_device_button_button
            function(device, code, bool(owners))
            self.lib.ei_device_frame(device, self.lib.ei_now(self.context))

    def flush_releases(self, device):
        for identity in list(self.releases):
            target, code, key = identity
            if target == device:
                function = self.lib.ei_device_keyboard_key if key else self.lib.ei_device_button_button
                function(device, code, False)
                self.lib.ei_device_frame(device, self.lib.ei_now(self.context))
                self.releases.remove(identity)

    def release(self, owner):
        for (device, code, key), owners in list(self.held.items()):
            if owner in owners:
                self.press(owner, device, code, False, key)

    def send(self, request):
        self.dispatch()
        owner = request['owner']
        if request['type'] == 'release':
            self.release(owner)
        elif request['type'] == 'key':
            code = request['code'] - 8
            if request['pressed']:
                self.press(owner, self.device(self.KEYBOARD), code, True, True)
            else:
                for (device, held_code, key), owners in list(self.held.items()):
                    if key and code == held_code and owner in owners:
                        self.press(owner, device, code, False, True)
        else:
            point = (request['x'], request['y'])
            device = self.device(self.ABSOLUTE, point)
            button, pressed = request['button'], request['pressed']
            # Validate the button device before moving, so failed clicks are atomic.
            button_device = self.device(self.SCROLL if button >= 4 else self.BUTTON) if button else None
            self.lib.ei_device_pointer_motion_absolute(device, *point)
            self.lib.ei_device_frame(device, self.lib.ei_now(self.context))
            if 1 <= button <= 3:
                self.press(owner, button_device, {1: 0x110, 2: 0x112, 3: 0x111}[button], pressed, False)
            elif button >= 4 and not pressed:
                dx, dy = {4: (0, -120), 5: (0, 120), 6: (-120, 0), 7: (120, 0)}[button]
                self.lib.ei_device_scroll_discrete(button_device, dx, dy)
                self.lib.ei_device_frame(button_device, self.lib.ei_now(self.context))

    def close(self):
        if self.context:
            for owner in set().union(*self.held.values()):
                self.release(owner)
            for device in self.devices.values():
                self.lib.ei_device_unref(device)
            self.devices.clear()
            self.lib.ei_unref(self.context)
            self.context = None


class Portal:
    def __init__(self, store, executable):
        from gi.repository import Gio, GLib
        self.Gio, self.GLib, self.store = Gio, GLib, store
        self.bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
        self.status, self.session, self.eis, self.source = 'unavailable', None, None, None
        self.registered, self.requests, self.closed_subscription = False, {}, None
        self.generation = 0
        apps = Path(os.environ.get('XDG_DATA_HOME', str(Path.home() / '.local/share'))) / 'applications'
        apps.mkdir(parents=True, exist_ok=True)
        executable = str(executable).replace('\\', '\\\\').replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
        atomic_write(apps / (APP_ID + '.desktop'), ('[Desktop Entry]\nType=Application\n'
            'Name=窗渡 PaneFerry Agent\nNoDisplay=true\nIcon=preferences-desktop-remote-desktop\n'
            'Exec="' + executable + '" serve\n').encode())
        self.bus.signal_subscribe('org.freedesktop.DBus', 'org.freedesktop.DBus',
            'NameOwnerChanged', '/org/freedesktop/DBus', PORTAL, Gio.DBusSignalFlags.NONE,
            self.owner_changed)

    def owner_changed(self, bus, sender, path, interface, signal, parameters):
        if not parameters.unpack()[1]:
            return  # Initial activation does not invalidate our registration.
        self.registered = False
        self.fail('unavailable')

    def reset(self):
        self.generation += 1
        for path, (subscription, timeout) in list(self.requests.items()):
            self.bus.signal_unsubscribe(subscription)
            self.GLib.source_remove(timeout)
            self.bus.call(PORTAL, path, 'org.freedesktop.portal.Request', 'Close', None,
                          None, self.Gio.DBusCallFlags.NONE, 1000, None, None)
        self.requests.clear()
        if self.closed_subscription:
            self.bus.signal_unsubscribe(self.closed_subscription)
            self.closed_subscription = None
        if self.source:
            self.GLib.source_remove(self.source)
            self.source = None
        if self.eis:
            self.eis.close()
            self.eis = None
        if self.session:
            self.bus.call(PORTAL, self.session, 'org.freedesktop.portal.Session', 'Close',
                          None, None, self.Gio.DBusCallFlags.NONE, 1000, None, None)
            self.session = None

    def fail(self, status):
        self.reset()
        self.status = status

    def ensure(self):
        if self.status in ('ready', 'pending', 'paused'):
            return
        try:
            if not self.registered:
                self.bus.call_sync(PORTAL, PATH, 'org.freedesktop.host.portal.Registry',
                    'Register', self.GLib.Variant('(sa{sv})', (APP_ID, {})), None,
                    self.Gio.DBusCallFlags.NONE, 1000, None)
                self.registered = True
            version = self.bus.call_sync(PORTAL, PATH, 'org.freedesktop.DBus.Properties',
                'Get', self.GLib.Variant('(ss)', (REMOTE, 'version')), None,
                self.Gio.DBusCallFlags.NONE, 1000, None).unpack()[0]
            if version < 2:
                raise RuntimeError('persistent portal unsupported')
            self.status = 'pending'
            self.request('CreateSession', (), {'session_handle_token': self.GLib.Variant('s', 'lra_' + secrets.token_hex(12))}, self.created)
        except Exception:
            self.fail('unavailable')

    def request(self, method, arguments, options, callback):
        token = 'lra_' + secrets.token_hex(12)
        options = dict(options, handle_token=self.GLib.Variant('s', token))
        sender = self.bus.get_unique_name()[1:].replace('.', '_')
        path = '/org/freedesktop/portal/desktop/request/' + sender + '/' + token
        generation = self.generation

        def response(bus, sender, response_path, interface, signal, parameters):
            entry = self.requests.pop(path, None)
            if not entry:
                return
            self.bus.signal_unsubscribe(entry[0])
            self.GLib.source_remove(entry[1])
            code, results = parameters.unpack()
            if code:
                self.fail('denied' if code == 1 else 'unavailable')
                return
            try:
                callback(results)
            except Exception:
                self.fail('unavailable')

        def expired():
            self.fail('unavailable')
            return False

        subscription = self.bus.signal_subscribe(PORTAL, 'org.freedesktop.portal.Request',
            'Response', path, None, self.Gio.DBusSignalFlags.NONE, response)
        self.requests[path] = (subscription, self.GLib.timeout_add_seconds(180, expired))

        def called(bus, result):
            try:
                returned = bus.call_finish(result).unpack()[0]
                if generation == self.generation and returned != path:
                    self.fail('unavailable')
            except Exception:
                if generation == self.generation:
                    self.fail('unavailable')

        signature = {'CreateSession': '(a{sv})', 'SelectDevices': '(oa{sv})', 'Start': '(osa{sv})'}[method]
        self.bus.call(PORTAL, PATH, REMOTE, method,
            self.GLib.Variant(signature, (*arguments, options)), None,
            self.Gio.DBusCallFlags.NONE, 5000, None, called)

    def created(self, results):
        session = results['session_handle']
        sender = self.bus.get_unique_name()[1:].replace('.', '_')
        if not session.startswith('/org/freedesktop/portal/desktop/session/' + sender + '/'):
            raise ValueError('invalid session handle')
        self.session = session
        self.closed_subscription = self.bus.signal_subscribe(PORTAL, 'org.freedesktop.portal.Session',
            'Closed', session, None, self.Gio.DBusSignalFlags.NONE,
            lambda *args: self.fail('unavailable'))
        options = {'types': self.GLib.Variant('u', 3), 'persist_mode': self.GLib.Variant('u', 2)}
        token = self.store.load()
        if token:
            options['restore_token'] = self.GLib.Variant('s', token)
        self.request('SelectDevices', (session,), options,
                     lambda result: self.request('Start', (session, ''), {}, self.started))

    def started(self, results):
        if results.get('devices', 0) & 3 != 3:
            self.fail('denied')
            return
        # Rotate before opening EIS. A failed subsequent connection must not
        # retain an already consumed token. The value never crosses the pipe.
        self.store.replace(results.get('restore_token'))
        generation = self.generation

        def connected(bus, result):
            try:
                reply, fds = bus.call_with_unix_fd_list_finish(result)
                fd = fds.get(reply.unpack()[0])
                if generation != self.generation:
                    os.close(fd)
                    return
                try:
                    self.eis = Eis(fd)
                finally:
                    os.close(fd)
                self.source = self.GLib.io_add_watch(self.eis.fd,
                    self.GLib.IO_IN | self.GLib.IO_HUP | self.GLib.IO_ERR, self.dispatch)
                self.dispatch()
            except Exception:
                if generation == self.generation:
                    self.fail('unavailable')

        self.bus.call_with_unix_fd_list(PORTAL, PATH, REMOTE, 'ConnectToEIS',
            self.GLib.Variant('(oa{sv})', (self.session, {})), None,
            self.Gio.DBusCallFlags.NONE, 5000, None, None, connected)

    def dispatch(self, *args):
        self.eis.dispatch()
        if self.eis.disconnected:
            self.fail('unavailable')
            return False
        self.status = 'ready' if self.eis.ready() else 'paused'
        return True

    def handle(self, request):
        kind = request['type']
        if kind == 'ensure':
            self.ensure()
        elif kind == 'release':
            if self.eis:
                self.eis.send(request)
        elif ((kind == 'key' or (kind == 'pointer' and request['button']))
              and not request['pressed'] and self.status != 'ready'):
            if self.eis:
                if kind == 'key':
                    self.eis.send(request)
                else:
                    self.eis.release(request['owner'])
        elif kind != 'status':
            # Dropped while awaiting consent; there is intentionally no replay.
            if self.status == 'unavailable':
                self.ensure()
            if self.status != 'ready':
                raise RuntimeError({'pending': 'desktop_authorization_pending',
                    'denied': 'desktop_authorization_denied', 'paused': 'desktop_input_paused'}
                    .get(self.status, 'desktop_input_unavailable'))
            self.eis.send(request)
        return {'status': self.status}


def main():
    from gi.repository import GLib
    root, executable = Path(sys.argv[1]), Path(sys.argv[2])
    store = TokenStore(root)
    lock = os.open(root / 'desktop-authorization.lock', os.O_CREAT | os.O_RDWR | os.O_NOFOLLOW, 0o600)
    fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    portal = Portal(store, executable)
    loop, pending = GLib.MainLoop(), bytearray()
    os.set_blocking(0, False)

    def readable(fd, condition):
        try:
            chunk = os.read(fd, 4096)
            if not chunk:
                loop.quit()
                return False
            pending.extend(chunk)
            if len(pending) > 8192:
                loop.quit()
                return False
            while b'\n' in pending:
                line, _, rest = pending.partition(b'\n')
                pending[:] = rest
                try:
                    request = json.loads(line)
                    kind = request['type']
                    if kind not in ('ensure', 'status', 'release', 'pointer', 'key'):
                        raise ValueError('invalid command')
                    if kind in ('release', 'pointer', 'key'):
                        if type(request['owner']) is not int or not 0 < request['owner'] < 2**64:
                            raise ValueError('invalid owner')
                    if kind in ('pointer', 'key'):
                        if type(request['pressed']) is not bool:
                            raise ValueError('invalid key state')
                        if kind == 'key':
                            if type(request['code']) is not int or not 8 <= request['code'] <= 255:
                                raise ValueError('invalid key')
                        elif (any(type(request[k]) is not int or not -32768 <= request[k] <= 32767 for k in ('x', 'y'))
                              or type(request['button']) is not int or not 0 <= request['button'] <= 7):
                            raise ValueError('invalid pointer')
                    reply = portal.handle(request)
                except Exception as error:
                    code = str(error)
                    if code not in ('desktop_authorization_pending', 'desktop_authorization_denied', 'desktop_input_paused'):
                        code = 'desktop_input_unavailable'
                    reply = {'status': portal.status, 'error': code}
                sys.stdout.write(json.dumps(reply) + '\n')
                sys.stdout.flush()
            return True
        except (OSError, ValueError):
            loop.quit()
            return False

    GLib.io_add_watch(0, GLib.IO_IN | GLib.IO_HUP | GLib.IO_ERR, readable)
    try:
        loop.run()
    finally:
        portal.reset()
        os.close(lock)


if __name__ == '__main__':
    main()
