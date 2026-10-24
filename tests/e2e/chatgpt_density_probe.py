#!/usr/bin/env python3
"""Opt-in same-binary ChatGPT density experiment, never a production fix.

Runs under a bounded systemd user unit and bubblewrap. Private PID/network/IPC,
hidden user homes and desktop sockets, fresh profile, private D-Bus and displays.
No login, debug port, process injection, zoom or installed extension changes.
"""
import argparse
import ctypes as C
import hashlib
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time
import uuid

APP = Path('/usr/lib/chatgpt/ChatGPT')
LOGICAL = (1200, 800)


def wait(read, timeout=45):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = read()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError('isolated fixture timed out')


def command(args, env):
    return subprocess.run(args, env=env, capture_output=True, text=True,
                          check=True, timeout=15).stdout


class XSettings:
    """Private X server only. Intentionally screen-wide, not per-window."""
    def __init__(self, env):
        # XOpenDisplay accepts DISPLAY explicitly but reads XAUTHORITY itself.
        os.environ['XAUTHORITY'] = env['XAUTHORITY']
        self.x = C.CDLL('libX11.so.6')
        L, I, P = C.c_ulong, C.c_int, C.c_void_p
        signatures = {
            'XOpenDisplay': ([C.c_char_p], P), 'XDefaultRootWindow': ([P], L),
            'XInternAtom': ([P, C.c_char_p, I], L),
            'XCreateSimpleWindow': ([P, L, I, I, C.c_uint, C.c_uint, C.c_uint, L, L], L),
            'XChangeProperty': ([P, L, L, L, I, I, P, I], I),
            'XSetSelectionOwner': ([P, L, L, L], I), 'XGetSelectionOwner': ([P, L], L),
            'XSync': ([P, I], I), 'XResizeWindow': ([P, L, C.c_uint, C.c_uint], I),
            'XCloseDisplay': ([P], I),
        }
        for name, (args, result) in signatures.items():
            fn = getattr(self.x, name)
            fn.argtypes, fn.restype = args, result
        self.d = self.x.XOpenDisplay(env['DISPLAY'].encode())
        assert self.d
        root = self.x.XDefaultRootWindow(self.d)
        self.owner = self.x.XCreateSimpleWindow(self.d, root, 0, 0, 1, 1, 0, 0, 0)
        self.serial = 0
        selection = self.atom('_XSETTINGS_S0')
        assert self.x.XGetSelectionOwner(self.d, selection) == 0
        self.x.XSetSelectionOwner(self.d, selection, self.owner, 0)
        self.scale(1)

    def atom(self, name):
        return self.x.XInternAtom(self.d, name.encode(), 0)

    def scale(self, scale):
        import struct
        self.serial += 1
        entries = []
        for name, value in [('Gdk/WindowScalingFactor', scale),
                            ('Gdk/UnscaledDPI', 96 * 1024), ('Xft/DPI', 96 * 1024 * scale)]:
            b = name.encode()
            entries.append(struct.pack('<BBH', 0, 0, len(b)) + b + b'\0' * (-len(b) % 4)
                           + struct.pack('<Ii', self.serial, value))
        data = struct.pack('<B3xII', 0, self.serial, len(entries)) + b''.join(entries)
        atom = self.atom('_XSETTINGS_SETTINGS')
        self.x.XChangeProperty(self.d, self.owner, atom, atom, 8, 0, data, len(data))
        self.x.XSync(self.d, 0)

    def resize(self, xid, width, height):
        self.x.XResizeWindow(self.d, xid, width, height)
        self.x.XSync(self.d, 0)

    def close(self):
        self.x.XCloseDisplay(self.d)


class Bridge:
    def __init__(self, env):
        self.p = subprocess.Popen(['/usr/bin/python3', '/mnt/wayland_helper.py'], env=env,
                                  stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

    def call(self, **body):
        self.p.stdin.write(json.dumps(body) + '\n')
        self.p.stdin.flush()
        reply = json.loads(self.p.stdout.readline())
        assert 'result' in reply, reply
        return reply['result']

    def close(self):
        self.p.stdin.close()
        self.p.wait(timeout=10)
        self.p.stdout.close()


def inner(mode):
    assert Path(__file__).resolve() == Path('/mnt/probe.py')
    assert os.readlink('/proc/self/ns/pid') != os.environ['PROBE_HOST_PID_NS']
    assert {name for _, name in socket.if_nameindex()} <= {'lo'}
    assert not any(Path('/home').iterdir())
    run = Path('/mnt/result')
    run.mkdir(mode=0o700)
    for name in ['home', 'config', 'data', 'cache', 'state', 'run', 'profile', 'codex']:
        (run / name).mkdir(mode=0o700)
    env = dict(PATH='/usr/bin:/bin', HOME=str(run / 'home'), SHELL='/bin/sh',
               XDG_CONFIG_HOME=str(run / 'config'), XDG_DATA_HOME=str(run / 'data'),
               XDG_CACHE_HOME=str(run / 'cache'), XDG_STATE_HOME=str(run / 'state'),
               XDG_RUNTIME_DIR=str(run / 'run'), CODEX_HOME=str(run / 'codex'),
               CODEX_ELECTRON_USER_DATA_PATH=str(run / 'profile'), LC_ALL='C.UTF-8',
               DISPLAY=':71', XAUTHORITY=str(run / 'authority'), GTK_USE_PORTAL='0',
               GIO_USE_VFS='local', NO_AT_BRIDGE='1', GSETTINGS_BACKEND='keyfile')
    children, bridge, settings = [], None, None
    log = (run / 'run.log').open('w')

    def start(args, environment=env):
        child = subprocess.Popen(args, env=environment, stdin=subprocess.DEVNULL,
                                 stdout=log, stderr=log, start_new_session=True)
        children.append(child)
        return child

    try:
        conf = run / 'bus.conf'
        # AppArmor queries cannot write through the read-only namespace root.
        # This applies only to this private bus; there are no activation services.
        conf.write_text('<busconfig><type>session</type><apparmor mode="disabled"/>'
                        f'<listen>unix:tmpdir={run / "run"}</listen><policy context="default">'
                        '<allow send_destination="*"/><allow receive_sender="*"/>'
                        '<allow own="*"/></policy></busconfig>')
        bus = subprocess.Popen(['dbus-daemon', '--nofork', '--config-file=' + str(conf),
                                '--print-address=1'], env=env, stdout=subprocess.PIPE,
                               stderr=log, text=True, start_new_session=True)
        children.append(bus)
        env['DBUS_SESSION_BUS_ADDRESS'] = bus.stdout.readline().strip()
        assert env['DBUS_SESSION_BUS_ADDRESS'].startswith('unix:path=/mnt/result/run/')
        # GNOME constructs system-bus proxies even headless. Give it this empty
        # private bus, never the host's login, power or display services.
        env['DBUS_SYSTEM_BUS_ADDRESS'] = env['DBUS_SESSION_BUS_ADDRESS']
        command(['xauth', '-f', env['XAUTHORITY'], 'add', env['DISPLAY'], '.', secrets.token_hex(16)], env)
        start(['Xvfb', env['DISPLAY'], '-screen', '0', '3000x2000x24', '-dpi', '96',
               '-nolisten', 'tcp', '-auth', env['XAUTHORITY']])
        wait(lambda: Path('/tmp/.X11-unix/X71').exists())
        if mode == 'x11':
            start(['openbox'])
            wait(lambda: 'window id #' in command(['xprop', '-root', '_NET_SUPPORTING_WM_CHECK'], env))
            settings = XSettings(env)
        else:
            env.update(WAYLAND_DISPLAY='paneferry-probe', XDG_CURRENT_DESKTOP='GNOME',
                       XDG_SESSION_TYPE='wayland', GNOME_SHELL_SESSION_MODE='user', LIBGL_ALWAYS_SOFTWARE='1')
            ext = run / 'data/gnome-shell/extensions/paneferry@local'
            ext.parent.mkdir(parents=True)
            shutil.copytree('/mnt/extension', ext)
            (run / 'data/gnome-shell/update-check-50').touch()
            # Only the disposable copy: allow measuring an X11 window on the
            # existing output, and wait for placement instead of assuming DPI.
            replacements = [('extension.js', 'if (lease.root.get_client_type() !== Meta.WindowClientType.WAYLAND) return;', ''),
                            ('virtualOutput.js', 'this.requireRoot().get_compositor_private()?.get_resource_scale() === scale',
                             'this.requireRoot().get_monitor() === index')]
            for name, old, new in replacements:
                file = ext / name
                content = file.read_text()
                assert content.count(old) == 1, 'fixture no longer matches production source'
                file.write_text(content.replace(old, new))
            command(['gsettings', 'set', 'org.gnome.shell', 'enabled-extensions', "['paneferry@local']"], env)
            command(['gsettings', 'set', 'org.gnome.shell', 'disable-user-extensions', 'false'], env)
            command(['gsettings', 'set', 'org.gnome.mutter', 'experimental-features', '[]'], env)
            start(['pipewire'])
            wait(lambda: (run / 'run/pipewire-0').exists())
            shell = start(['gnome-shell', '--headless', '--wayland', '--wayland-display', env['WAYLAND_DISPLAY'],
                           '--virtual-monitor', '3000x2000'])
            wait(lambda: (run / 'run' / env['WAYLAND_DISPLAY']).exists())
            display = wait(lambda: re.search(r'Using public X11 display (:\d+)', (run / 'run.log').read_text()))
            env['DISPLAY'] = display[1]
            env['XAUTHORITY'] = str(wait(lambda: next((run / 'run').glob('.mutter-Xwaylandauth.*'), None)))
            if mode == 'gnome':
                start(['/usr/libexec/gsd-xsettings'], dict(env, GDK_BACKEND='x11'))
            else:
                settings = XSettings(env)
            bridge = Bridge(env)

            def ready():
                assert shell.poll() is None, 'isolated GNOME exited'
                try:
                    return bridge.call(op='list') is not None
                except AssertionError:
                    return False
            wait(ready)
        witness = run / 'witness.json'
        observer = start(['/usr/bin/python3', '/mnt/wayland_fixture.py', str(witness), '--x11'],
                         dict(env, GDK_BACKEND='x11'))
        wait(witness.exists)
        # Inner Chromium sandbox cannot start inside the outer PID namespace.
        # Bubblewrap still hides the desktop, user data and external network.
        app = start([str(APP), '--user-data-dir=' + str(run / 'profile'),
                     '--ozone-platform=x11', '--disable-gpu', '--no-sandbox'], dict(env, GDK_BACKEND='x11'))
        identity = lambda: Path(f'/proc/{app.pid}/stat').read_text().rsplit(')', 1)[1].split()[19]
        started = identity()

        def window():
            assert app.poll() is None, f'isolated ChatGPT exited: {app.returncode}'
            if bridge:
                return next((w for w in bridge.call(op='list') if w['pid'] == app.pid), None)
            for line in command(['wmctrl', '-lp'], env).splitlines():
                fields = line.split(None, 4)
                if len(fields) >= 3 and int(fields[2]) == app.pid:
                    return {'xwindow': int(fields[0], 16)}
        original = wait(window, 60)
        xid = original['xwindow']
        time.sleep(5)  # Allow the fresh, offline login UI to finish its initial layout.
        report = []

        def settle():
            for _ in range(15):
                if bridge:
                    bridge.call(op='scope')
                time.sleep(.2)

        def sample(label, expected_pixels=None):
            assert app.poll() is None and identity() == started and observer.poll() is None
            if mode == 'x11':
                command(['wmctrl', '-ia', hex(xid)], env)  # Avoid obscured drawable pixels on Openbox.
            actual = window()
            subprocess.run(['/mnt/quality_sample', str(xid), str(app.pid), str(run / label)],
                           env=env, check=True, timeout=30)
            captured = json.loads((run / label / 'sample.json').read_text())
            pixels = [captured['width'], captured['height']]
            if expected_pixels:
                assert pixels == list(expected_pixels), (label, expected_pixels, pixels)
            other = json.loads(witness.read_text())
            assert other['text'] == 'preserved fixture contents'
            report.append(dict(label=label, pid=app.pid, start_ticks=started, xid=xid, window=actual,
                               pixels=pixels, witness=other, xrandr=command(['xrandr', '--current'], env),
                               xrdb=command(['xrdb', '-query'], env)))
            print(label, 'actual pixels', pixels, 'witness scale', other['scale'], flush=True)

        if mode == 'x11':
            for label, scale in [('before', 1), ('density-2x', 2), ('restored', 1)]:
                settings.scale(scale)
                settings.resize(xid, LOGICAL[0] * scale, LOGICAL[1] * scale)
                settle()
                sample(label, (LOGICAL[0] * scale, LOGICAL[1] * scale))
        else:
            for label, scale in [('before', 1), ('density-2x', 2), ('restored', 1)]:
                bridge.call(op='acquire', id=original['id'], pid=app.pid, scale=scale)
                settle()  # Observe the output move before issuing the test's logical resize.
                bridge.call(op='resize', window_id=original['window_id'], width=LOGICAL[0], height=LOGICAL[1])
                settle()
                sample(label, (LOGICAL[0] * scale, LOGICAL[1] * scale))
                bridge.close()
                bridge = Bridge(env)
                time.sleep(3)
                if scale == 2:
                    sample('after-release')  # Record actual transition, never call it restored from metadata alone.
        (run / 'report.json').write_text(json.dumps(dict(mode=mode, logical=list(LOGICAL), samples=report,
            scope='isolated measurements, not an eligible per-window fix; no authenticated profile'), indent=2) + '\n')
        print('COMPLETED: same-process experiment; eligibility requires image and bystander analysis', flush=True)
    finally:
        if bridge:
            bridge.close()
        if settings:
            settings.close()
        for child in reversed(children):
            if child.poll() is None:
                os.killpg(child.pid, signal.SIGTERM)
                try:
                    child.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    os.killpg(child.pid, signal.SIGKILL)
                    child.wait(timeout=5)
        log.close()


def outer(args):
    assert sys.platform == 'linux', 'requires Ubuntu GNOME, bubblewrap and systemd user manager'
    assert args.output and args.probe and args.expected_sha256, '--output, --probe and --expected-sha256 required'
    digest = hashlib.sha256(APP.read_bytes()).hexdigest()
    assert digest == args.expected_sha256, 'installed app differs from the selected version'
    output = args.output.resolve()
    output.mkdir(mode=0o700)  # No overwriting earlier evidence.
    repo = Path(__file__).resolve().parents[2]
    with tempfile.TemporaryDirectory(prefix='paneferry-chatgpt-probe-') as folder:
        stage = Path(folder)
        shutil.copy2(__file__, stage / 'probe.py')
        shutil.copy2(args.probe.resolve(), stage / 'quality_sample')
        shutil.copy2(repo / 'apps/linux-agent/src/wayland_helper.py', stage / 'wayland_helper.py')
        shutil.copy2(repo / 'tests/e2e/wayland_fixture.py', stage / 'wayland_fixture.py')
        shutil.copytree(repo / 'apps/linux-agent/gnome-extension', stage / 'extension')
        unit = 'paneferry-chatgpt-probe-' + uuid.uuid4().hex
        cmd = ['systemd-run', '--user', '--collect', '--wait', '--pipe', '--unit=' + unit,
               '-p', 'MemoryMax=4G', '-p', 'CPUQuota=200%', '-p', 'TasksMax=512', '-p', 'RuntimeMaxSec=240',
               'bwrap', '--unshare-pid', '--unshare-net', '--unshare-ipc', '--unshare-uts', '--new-session',
               '--die-with-parent', '--ro-bind', '/', '/', '--tmpfs', '/home', '--tmpfs', '/tmp',
               '--tmpfs', '/run', '--dev', '/dev', '--proc', '/proc',
               '--bind', str(stage), '/mnt', '--chdir', '/mnt',
               '--setenv', 'PROBE_HOST_PID_NS', os.readlink('/proc/self/ns/pid'),
               '/usr/bin/python3', '/mnt/probe.py', args.mode, '--inside']
        try:
            with (output / 'execution.log').open('w') as log:
                result = subprocess.run(cmd, stdout=log, stderr=subprocess.STDOUT, timeout=270)
        finally:
            # Also stops descendants on client timeout or an interrupted run.
            subprocess.run(['systemctl', '--user', 'stop', unit], capture_output=True, timeout=15)
            if (stage / 'result').exists():
                for item in (stage / 'result').iterdir():
                    if item.name in ['run.log', 'report.json']:
                        shutil.copy2(item, output / item.name)
                    elif item.is_dir() and (item / 'sample.json').exists():
                        shutil.copytree(item, output / item.name)
        assert result.returncode == 0, (output / 'execution.log').read_text()[-4000:]
    (output / 'version.json').write_text(json.dumps(dict(sha256=digest,
        package_version=command(['dpkg-query', '-W', '-f=${Version}', 'chatgpt'], os.environ),
        runtime_version=Path('/usr/lib/chatgpt/version').read_text().strip()), indent=2) + '\n')
    print('Saved isolated measurements to', output)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=('x11', 'gnome', 'gnome-fixed'))
    parser.add_argument('--inside', action='store_true', help=argparse.SUPPRESS)
    parser.add_argument('--probe', type=Path)
    parser.add_argument('--output', type=Path)
    parser.add_argument('--expected-sha256')
    args = parser.parse_args()
    if args.inside:
        inner(args.mode)
    else:
        outer(args)
