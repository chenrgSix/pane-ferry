#!/usr/bin/env python3
"""Linux-only: fake Secret Service, unrelated bus peer, and private Agent sessions.

Never contacts the real desktop keyring or reads credentials.
"""
import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time


def connection():
    from gi.repository import Gio
    return Gio.bus_get_sync(Gio.BusType.SESSION, None)


def call(bus, name, method='Ping'):
    from gi.repository import Gio, GLib
    return bus.call_sync(name, '/test', 'org.example.Probe', method, None,
                         GLib.VariantType.new('(s)'), Gio.DBusCallFlags.NONE, 3000, None).unpack()[0]


def service(name, output):
    from gi.repository import Gio, GLib
    bus = connection()
    bus.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                  'RequestName', GLib.Variant('(su)', (name, 0)), None, Gio.DBusCallFlags.NONE, 3000, None)
    interface = Gio.DBusNodeInfo.new_for_xml('''<node><interface name="org.example.Probe">
        <method name="Ping"><arg type="s" direction="out"/></method></interface></node>''').interfaces[0]
    bus.register_object('/test', interface, lambda *args: args[-1].return_value(GLib.Variant('(s)', ('fixture-only',))), None, None)
    Path(output).write_text(bus.get_unique_name())
    GLib.MainLoop().run()


def application(output, unrelated, policy):
    with open(output + '.stderr', 'w') as log:
        os.dup2(log.fileno(), 2)
    import gi
    gi.require_version('Gtk', '3.0')
    from gi.repository import Gio, GLib, Gtk
    bus = connection()
    def permitted(name):
        try:
            return call(bus, name) == 'fixture-only'
        except GLib.Error:
            return False
    own = True
    try:
        bus.call_sync('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus',
                      'RequestName', GLib.Variant('(su)', ('org.example.LaunchAnotherApp', 0)),
                      None, Gio.DBusCallFlags.NONE, 3000, None)
    except GLib.Error:
        own = False
    data = dict(address=os.environ['DBUS_SESSION_BUS_ADDRESS'],
                secret=permitted('org.freedesktop.secrets'), unrelated=permitted('org.example.Unrelated'),
                unique=permitted(Path(unrelated).read_text()), own=own, display=os.environ['DISPLAY'])
    assert data['secret'] == (policy == 'desktop_secrets'), data
    assert not data['unrelated'] and not data['unique'], data
    if policy == 'desktop_secrets':
        assert not own, data
    Path(output).write_text(json.dumps(data))
    window = Gtk.Window(title='Session bus fixture')
    window.set_default_size(320, 180)
    window.add(Gtk.Label(label='Independent session'))
    window.connect('destroy', Gtk.main_quit)
    window.show_all()
    Gtk.main()


def wait(predicate):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.03)
    raise AssertionError('readiness timeout')


class Peer:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(15)
        self.socket.connect(str(path))
        self.file = self.socket.makefile('rwb', buffering=0)
        self.request('hello')

    def request(self, kind, expected=None, **data):
        self.file.write((json.dumps(dict(data, type=kind, version=1)) + '\n').encode())
        while True:
            value = json.loads(self.file.readline(16 * 1024 * 1024))
            assert value['type'] != 'error' or expected == 'error', value
            if value['type'] == (expected or kind):
                return value

    def close(self):
        self.file.close()
        self.socket.close()


def main(agent):
    children, peers = [], []
    def start(argv, env):
        child = subprocess.Popen(argv, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                 stderr=subprocess.DEVNULL, start_new_session=True)
        children.append(child)
        return child
    with tempfile.TemporaryDirectory(prefix='lra-bus-') as temporary:
        root = Path(temporary)
        # Exercise D-Bus address escaping, not just alphanumeric paths.
        home, runtime = root/'home,space', root/'run,space'
        home.mkdir(); runtime.mkdir(mode=0o700)
        (root/'data').mkdir()
        for name in ('themes', 'openbox', 'icons', 'mime', 'glycin-loaders'):
            source = Path('/usr/share')/name
            if source.is_dir():
                (root/'data'/name).symlink_to(source, target_is_directory=True)
        env = dict(os.environ, HOME=str(home), XDG_RUNTIME_DIR=str(runtime),
                   XDG_DATA_HOME=str(root/'data'), XDG_DATA_DIRS=str(root/'data'),
                   DBUS_SESSION_BUS_ADDRESS='unix:path=' + str(runtime/'bus').replace(',', '%2c'))
        for key in ('DISPLAY', 'WAYLAND_DISPLAY', 'SESSION_MANAGER'):
            env.pop(key, None)
        try:
            start(['dbus-daemon', '--session', '--nofork', '--address='+env['DBUS_SESSION_BUS_ADDRESS']], env)
            wait(lambda: (runtime/'bus').exists())
            for name, file in [('org.freedesktop.secrets', 'secrets'), ('org.example.Unrelated', 'unrelated')]:
                start(['/usr/bin/python3', __file__, 'service', name, str(root/file)], env)
                wait(lambda: (root/file).exists())
            catalog = home/'.config/lan-remoteapp/apps.json'
            catalog.parent.mkdir(parents=True)
            apps = [dict(id=name, name=name, executable='/usr/bin/python3', session_bus=policy,
                         argv=[str(Path(__file__).resolve()), 'app', str(root/name), str(root/'unrelated'), policy])
                    for name, policy in [('first', 'desktop_secrets'), ('second', 'desktop_secrets'), ('isolated', 'isolated')]]
            catalog.write_text(json.dumps(apps))
            process = start([str(agent), 'serve'], env)
            path = home/'.local/state/lan-remoteapp/agent.sock'
            wait(path.exists)
            sessions = []
            for app in apps:
                peer = Peer(path); peers.append(peer)
                response = peer.request('app.launch', 'session.started', app_id=app['id'], request_id=app['id'], backend='native', attach=False)
                sessions.append(response['session_id'])
                wait(lambda: (root/app['id']).exists())
                snapshot = peer.request('session.resume', 'session.snapshot', session_id=response['session_id'])
                if not snapshot['windows']:
                    # GTK mapping can follow the report file by a few milliseconds.
                    while True:
                        event = json.loads(peer.file.readline(16 * 1024 * 1024))
                        assert event['type'] not in ('session.exited', 'error'), (event, (root/(app['id']+'.stderr')).read_text())
                        if event['type'] == 'window.created':
                            break
            data = [json.loads((root/name).read_text()) for name in ['first', 'second', 'isolated']]
            assert len({x['address'] for x in data}) == 3
            assert len({x['display'] for x in data}) == 3
            proxies = list(path.parent.glob('secrets-*.sock'))
            assert len(proxies) == 2
            for peer in peers:
                peer.close()
            peers.clear()
            process.terminate(); process.wait(timeout=10)
            assert all(not p.exists() for p in proxies), 'proxy sockets leaked on normal shutdown'
            # A missing desktop bus must reject launch and roll back its private resources.
            failed_env = dict(env, XDG_RUNTIME_DIR=str(root/'missing'))
            failed = start([str(agent), 'serve'], failed_env)
            wait(path.exists)
            peer = Peer(path); peers.append(peer)
            result = peer.request('app.launch', 'error', app_id='first', request_id='missing-bus', backend='native', attach=False)
            assert result['code'] == 'desktop_secrets_unavailable', result
            assert not peer.request('session.list')['sessions']
            assert not list(path.parent.glob('secrets-*.sock'))
            peer.close(); peers.clear()
            failed.terminate(); failed.wait(timeout=10)
            print('PASS: keyring-only access, blocked unrelated names/unique IDs/activation, independent sessions, default isolated bus, cleanup, missing-bus rollback')
        finally:
            for peer in peers:
                peer.close()
            for process in reversed(children):
                if process.poll() is None:
                    os.killpg(process.pid, signal.SIGTERM)
                    process.wait(timeout=10)


if __name__ == '__main__':
    if len(sys.argv) > 1 and sys.argv[1] == 'service':
        service(*sys.argv[2:])
    elif len(sys.argv) > 1 and sys.argv[1] == 'app':
        application(*sys.argv[2:])
    else:
        parser = argparse.ArgumentParser(description=__doc__)
        parser.add_argument('--agent', required=True, type=Path)
        main(parser.parse_args().agent.resolve())
