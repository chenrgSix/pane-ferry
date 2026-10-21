#!/usr/bin/env python3
"""Linux opt-in: isolated XDG files, HOME, Xvfb and windows; never the real desktop."""
import argparse
import base64
import json
import os
from pathlib import Path
import secrets
import signal
import socket
import subprocess
import sys
import tempfile
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--agent', required=True, type=Path)
args = parser.parse_args()
children = []


def start(argv, env, stderr=subprocess.DEVNULL):
    process = subprocess.Popen(argv, env=env, start_new_session=True,
                               stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=stderr)
    children.append(process)
    return process


def wait(predicate, message):
    deadline = time.monotonic() + 15
    while time.monotonic() < deadline:
        result = predicate()
        if result:
            return result
        time.sleep(.03)
    raise AssertionError(message)


class Peer:
    def __init__(self, path):
        self.socket = socket.socket(socket.AF_UNIX)
        self.socket.settimeout(15)
        self.socket.connect(str(path))
        self.file = self.socket.makefile('rwb')
        self.send(type='hello')
        assert self.until('hello')['version'] == 1

    def send(self, **data):
        self.file.write(json.dumps(dict(data, version=1)).encode()+b'\n')
        self.file.flush()

    def until(self, kind):
        for _ in range(128):
            line = self.file.readline(16*1024*1024+1)
            assert line.endswith(b'\n') and len(line) <= 16*1024*1024
            value = json.loads(line)
            assert value['type'] != 'error' or kind == 'error', value
            if value['type'] == kind:
                return value
        raise AssertionError('missing event '+kind)

    def request(self, kind, expected=None, **data):
        self.send(type=kind, **data)
        return self.until(expected or kind)

    def close(self):
        self.file.close()
        self.socket.close()


try:
    with tempfile.TemporaryDirectory(prefix='remoteapp-discovery-') as temporary:
        root = Path(temporary)
        home, user_data, system_data = root/'home', root/'user-data', root/'system-data'
        for directory in (home, user_data/'applications', system_data/'applications'):
            directory.mkdir(parents=True)
        # Keep public image type/loader configuration while isolating app entries.
        # Ubuntu 26's GdkPixbuf uses Glycin's XDG loader configuration for SVG.
        for name in ('mime', 'glycin-loaders', 'themes', 'openbox', 'icons'):
            source = Path('/usr/share')/name
            if source.is_dir():
                (system_data/name).symlink_to(source, target_is_directory=True)
        authority = root/'Xauthority'
        authority.touch(mode=0o600)
        display = next(n for n in range(2100, 3000) if not Path(f'/tmp/.X11-unix/X{n}').exists())
        subprocess.run(['xauth', '-f', str(authority), 'add', f':{display}',
                        'MIT-MAGIC-COOKIE-1', secrets.token_hex(16)], check=True)
        env = dict(os.environ, HOME=str(home), XDG_DATA_HOME=str(user_data), XDG_DATA_DIRS=str(system_data),
                   DISPLAY=f':{display}', XAUTHORITY=str(authority), XDG_CURRENT_DESKTOP='GNOME',
                   LANGUAGE='zh_CN', LANG='zh_CN.UTF-8', GDK_BACKEND='x11', GTK_IM_MODULE='simple',
                   XMODIFIERS='@im=none', QT_IM_MODULE='compose')
        for key in ('LC_ALL', 'WAYLAND_DISPLAY', 'DBUS_SESSION_BUS_ADDRESS', 'SESSION_MANAGER'):
            env.pop(key, None)
        start(['Xvfb', f':{display}', '-screen', '0', '1600x900x24', '-nolisten', 'tcp', '-auth', str(authority)], env)
        wait(lambda: Path(f'/tmp/.X11-unix/X{display}').exists(), 'Xvfb ready')
        wm_log = root/'openbox.log'
        with wm_log.open('w') as log:
            wm = start(['openbox'], env, stderr=log)
        time.sleep(.4)
        state = root/'windows.json'
        fixture = start([sys.executable, str(Path(__file__).with_name('discovery_fixture.py')), str(state)], env)
        initial = wait(lambda: json.loads(state.read_text()) if state.exists() else None, 'fixture windows ready')
        assert len(initial) == 3
        # A real vector icon is rasterized, not forwarded as SVG or a remote path.
        icon = root/'fixture.svg'
        icon.write_text('<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" fill="#24805c"/></svg>')
        terminal_input = root/'terminal-input.txt'
        terminal_program = root/'terminal-input.py'
        terminal_program.write_text('import sys,time\nfrom pathlib import Path\n'
                                    f'Path({str(terminal_input)!r}).write_text(sys.stdin.readline())\n'
                                    'time.sleep(60)\n')
        def desktop(directory, name, extra=''):
            (directory/'applications'/name).write_text('[Desktop Entry]\nType=Application\nName=Base Name\nName[zh_CN]=发现测试\n'
                f'Icon={icon}\nExec=/usr/bin/xterm -title "Discovered-only" -e /usr/bin/python3 {terminal_program} %U\n'+extra)
        desktop(system_data, 'sample.desktop')
        desktop(user_data, 'sample.desktop', 'Comment=User override\n')
        desktop(system_data, 'masked.desktop')
        desktop(user_data, 'masked.desktop', 'Hidden=true\n')
        desktop(system_data, 'nodisplay.desktop', 'NoDisplay=true\n')
        desktop(system_data, 'missing.desktop', 'TryExec=/not/a/real/program\n')
        desktop(system_data, 'other-desktop.desktop', 'OnlyShowIn=KDE;\n')
        # No apps.json: installation discovery must work without a whitelist.
        agent = start([str(args.agent.resolve()), 'serve'], env)
        sock = home/'.local/state/lan-remoteapp/agent.sock'
        wait(sock.exists, 'Agent socket ready')
        peer = Peer(sock)
        catalog = peer.request('app.list')
        assert catalog['discovery_supported'] and catalog['discovery_error'] is None, catalog
        apps = catalog['apps']
        assert len(apps) == 1 and apps[0]['source'] == 'desktop', apps
        app = apps[0]
        assert app['name'] == '发现测试' and app['description'] == 'User override', app
        assert 'executable' not in app and 'argv' not in app and 'desktop_id' not in app
        assert base64.b64decode(app['icon_png']).startswith(b'\x89PNG\r\n\x1a\n')
        # Refresh really rereads the installation directories.
        desktop(system_data, 'new.desktop')
        assert len(peer.request('app.list')['apps']) == 2
        (system_data/'applications/new.desktop').unlink()
        assert [a['id'] for a in peer.request('app.list')['apps']] == [app['id']]
        before = json.loads(state.read_text())
        listed = peer.request('window.list')
        assert listed['unavailable_reason'] is None, listed
        windows = {value['title']: value for value in listed['windows']}
        assert set(windows) == set(before), (windows, 'WM status', wm.poll(), wm_log.read_text())
        assert all(value['icon_png'] for value in windows.values()), windows
        assert json.loads(state.read_text()) == before, 'listing changed window geometry or text'
        again = {value['title']: value['id'] for value in peer.request('window.list')['windows']}
        assert again == {title:value['id'] for title,value in windows.items()}, 'unstable running IDs'
        peer.close()
        # Select Alpha. Beta has the same PID, but must never enter its session.
        selected = Peer(sock)
        selected.request('app.launch', 'session.started', app_id=windows['DiscoveryAlpha']['id'],
                         request_id='take-alpha', backend='native')
        frame = selected.until('frame')
        allowed = {before['DiscoveryAlpha']['id'], before['DiscoveryDialog']['id']}
        assert frame['window_id'] in allowed
        selected.send(type='input.key', window_id=before['DiscoveryBeta']['id'], keysym=ord('z'), pressed=True)
        assert selected.until('error')['code'] == 'window_unavailable'
        alpha = before['DiscoveryAlpha']['id']
        text = '接管窗口中文🙂'
        selected.send(type='input.text.commit', window_id=alpha, text=text)
        wait(lambda: json.loads(state.read_text())['DiscoveryAlpha']['text'] == text, 'selected-window Unicode commit')
        assert json.loads(state.read_text())['DiscoveryBeta']['text'] == ''
        for keysym, pressed in ((0xffe3, True), (ord('a'), True), (ord('a'), False), (0xffe3, False)):
            selected.send(type='input.key', window_id=alpha, keysym=keysym, pressed=pressed)
        selected.send(type='clipboard.copy', window_id=alpha, request_id='copy-selected')
        assert selected.until('clipboard.data')['text'] == text
        replacement = '控制端主动粘贴🙂'
        selected.send(type='clipboard.paste', window_id=alpha, text=replacement)
        wait(lambda: json.loads(state.read_text())['DiscoveryAlpha']['text'] == replacement, 'selected-window explicit paste')
        selected.send(type='window.close.request', window_id=before['DiscoveryAlpha']['id'])
        selected.until('session.exited')
        assert fixture.poll() is None and 'DiscoveryBeta' in json.loads(state.read_text())
        selected.close()
        # Launch an actual discovered entry into its own virtual desktop.
        launch = Peer(sock)
        started = launch.request('app.launch', 'session.started', app_id=app['id'], request_id='desktop-test', backend='native')
        frame = launch.until('frame')
        assert frame.get('png') and started['session_id']
        launch.send(type='input.text.commit', window_id=frame['window_id'], text='终端输入中文🙂')
        launch.send(type='input.key', window_id=frame['window_id'], keysym=0xff0d, pressed=True)
        launch.send(type='input.key', window_id=frame['window_id'], keysym=0xff0d, pressed=False)
        wait(terminal_input.exists, 'owned xterm accepted controller text')
        assert terminal_input.read_text().strip() == '终端输入中文🙂'
        sessions = Peer(sock)
        assert len(sessions.request('session.list')['sessions']) == 1
        # Private RemoteApp desktops must not appear as original desktop windows.
        assert all(w['title'] != 'Discovered-only' for w in sessions.request('window.list')['windows'])
        sessions.close()
        launch.close()
        agent.terminate(); agent.wait(timeout=10)
        assert fixture.poll() is None, 'Agent killed an existing process'
        print('PASS: real localized desktop discovery, theme/SVG PNG icons, XDG overrides/Hidden/NoDisplay/TryExec, live refresh, discovered launch')
        print('PASS: read-only window inventory, stable IDs, same-PID window scope with dialogs, unrelated input rejection, scoped Unicode and explicit copy/paste, target close and existing-process survival')
finally:
    for process in reversed(children):
        if process.poll() is None:
            os.killpg(process.pid, signal.SIGTERM)
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                os.killpg(process.pid, signal.SIGKILL)
                process.wait(timeout=5)
