#!/usr/bin/env python3
"""Single LAN RemoteApp entry: local publishing and remote native windows."""
import atexit
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time
import uuid

sys.path.insert(0, str(Path(__file__).resolve().parent))
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib, Gio, GdkPixbuf
from wire import STATE, Wire, peers, private, request, save, message
from windowhost import Viewer


def ensure_service():
    try:
        wire = Wire(local=True); wire.close(); return
    except (OSError, EOFError):
        pass
    agent = Path.home() / '.local/bin/remoteapp-agent'
    if not agent.is_file():
        raise ValueError('agent_unavailable')
    subprocess.Popen([str(agent), 'serve'], stdin=subprocess.DEVNULL,
                     stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, start_new_session=True)
    for _ in range(50):
        try:
            wire = Wire(local=True); wire.close(); return
        except (OSError, EOFError):
            time.sleep(.1)
    raise ValueError('agent_unavailable')


def receiver_identity():
    # Metadata registration prevents publishing any receiving window again.
    private(STATE / 'receivers')
    started = int(Path('/proc/self/stat').read_text().rsplit(')', 1)[1].split()[19])
    path = STATE / 'receivers' / f'{os.getpid()}.json'
    save(path, dict(pid=os.getpid(), started=started))
    atexit.register(lambda: path.unlink(missing_ok=True))


class Manager(Gtk.Application):
    def __init__(self):
        super().__init__(application_id='io.lanremoteapp.launcher', flags=Gio.ApplicationFlags.HANDLES_COMMAND_LINE)
        self.connect('command-line', self.command_line)
        self.connect('activate', self.activate)
        self.window = None; self.selected = None; self.catalogs = {}; self.viewers = []; self.revision = 0
        self.pending_launches = {}; self.peer_keys = []
        self.store = STATE / 'catalog-cache.json'
        try:
            if self.store.stat().st_size <= 8 * 1024 * 1024 and not self.store.is_symlink():
                self.catalogs = json.loads(self.store.read_text())
        except (OSError, ValueError):
            pass
        self.launch_store = STATE / 'pending-launches.json'
        try:
            if self.launch_store.stat().st_size <= 256*1024 and not self.launch_store.is_symlink():
                self.pending_launches = json.loads(self.launch_store.read_text())
        except (OSError, ValueError):
            pass

    def command_line(self, app, command):
        args = command.get_arguments()[1:]
        self.activate(app)
        if '--peer' in args and '--session' in args:
            try:
                peer = args[args.index('--peer')+1]; session = args[args.index('--session')+1]
                uuid.UUID(peer)
                if not session or len(session)>100 or not all(c.isalnum() or c in '-_' for c in session):
                    return 1
                name = next((p['name'] for p in peers() if p['device_id']==peer), 'Mac')
                self.open_viewer(peer, name, session_id=session)
            except (ValueError, IndexError):
                return 1
        return 0

    def activate(self, *args):
        if self.window:
            self.window.present(); return
        self.window = Gtk.ApplicationWindow(application=self, title='LAN RemoteApp')
        self.window.set_default_size(1020, 740)
        root = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=14, margin=20)
        title = Gtk.Label(label='LAN RemoteApp · 应用工作台', xalign=0); title.set_markup('<big><b>LAN RemoteApp · 应用工作台</b></big>')
        root.pack_start(title, False, False, 0)
        local = Gtk.Box(spacing=12)
        local.pack_start(Gtk.Label(label='本机应用共享', xalign=0), False, False, 0)
        self.sharing = Gtk.Switch(); self.sharing.connect('state-set', self.set_sharing)
        local.pack_start(self.sharing, False, False, 0)
        self.local_status = Gtk.Label(label='读取本机服务状态…', xalign=0)
        local.pack_start(self.local_status, True, True, 0); root.pack_start(local, False, False, 0)
        controls = Gtk.Box(spacing=10)
        self.selector = Gtk.ComboBoxText(); self.selector.connect('changed', self.selected_device)
        controls.pack_start(Gtk.Label(label='远端设备'), False, False, 0); controls.pack_start(self.selector, True, True, 0)
        refresh = Gtk.Button(label='刷新应用'); refresh.connect('clicked', lambda *_: self.load(True)); controls.pack_start(refresh, False, False, 0)
        refresh_windows = Gtk.Button(label='刷新运行窗口'); refresh_windows.connect('clicked', lambda *_: self.load(False)); controls.pack_start(refresh_windows, False, False, 0)
        root.pack_start(controls, False, False, 0)
        self.notice = Gtk.Label(label='在已连接 Ubuntu 的 Mac 上开启“本机共享”，设备会自动出现在这里。', xalign=0)
        self.notice.set_line_wrap(True);root.pack_start(self.notice, False, False, 0)
        scroll = Gtk.ScrolledWindow(); self.rows = Gtk.ListBox(); self.rows.set_selection_mode(Gtk.SelectionMode.NONE)
        scroll.add(self.rows);root.pack_start(scroll, True, True, 0)
        root.pack_start(Gtk.Label(label='本机与远端可同时使用 · 关闭接收窗口只断开连接 · 剪贴板仅在复制粘贴时传输', xalign=0), False, False, 0)
        self.window.add(root); self.window.show_all()
        self.window.connect('destroy', lambda *_: setattr(self, 'window', None))
        threading.Thread(target=self.initialize, daemon=True).start()
        GLib.timeout_add_seconds(3, self.refresh_peers)

    def initialize(self):
        try:
            ensure_service(); receiver_identity()
            info = request(None, 'node.info', local=True)
            GLib.idle_add(self.initialized, info)
        except (OSError, ValueError, EOFError) as error:
            GLib.idle_add(self.set_notice, message(error))

    def initialized(self, info):
        if self.window:
            self.sharing.set_active(info.get('sharing_enabled', True))
            self.local_status.set_text('Ubuntu · 发布与接收已就绪')
            self.refresh_peers()
        return False

    def set_notice(self, text):
        if self.window:
            self.notice.set_text(text)
        return False

    def set_sharing(self, switch, enabled):
        def change():
            try:
                request(None, 'sharing.set', 'sharing.changed', local=True, enabled=enabled)
                GLib.idle_add(self.set_notice, '本机共享已开启。' if enabled else '本机共享已停止；远端接收窗口继续运行。')
            except (OSError, ValueError, EOFError) as error:
                GLib.idle_add(self.set_notice, message(error))
        threading.Thread(target=change, daemon=True).start()
        return False

    def refresh_peers(self):
        if not self.window:
            return False
        available = peers(); keys = [(p['device_id'], p['name'], p['generation']) for p in available]
        if keys != self.peer_keys:
            selected = self.selected; self.peer_keys = keys
            self.selector.remove_all()
            for peer in available:
                self.selector.append(peer['device_id'], peer['name'])
            if selected and any(p['device_id']==selected for p in available):
                self.selector.set_active_id(selected)
            elif available:
                self.selector.set_active(0)
            else:
                self.selected = None; self.clear_rows()
                self.set_notice('等待 Mac 返回连接；网络恢复后会自动重新出现。')
        return True

    def selected_device(self, selector):
        self.selected = selector.get_active_id(); self.revision += 1
        if self.selected:
            self.load(False)

    def clear_rows(self):
        for row in self.rows.get_children():
            self.rows.remove(row)

    def load(self, refresh=False):
        if not self.selected:
            return
        peer = self.selected; self.revision += 1; revision = self.revision
        if peer in self.catalogs:
            self.render(peer, self.catalogs[peer]['apps'], [], [])
        def fetch():
            try:
                catalog = self.catalogs.get(peer)
                if refresh or not catalog:
                    catalog = dict(apps=request(peer, 'app.list')['apps'], saved_at=int(time.time()))
                    self.catalogs[peer] = catalog
                    save(self.store, self.catalogs)
                windows = request(peer, 'window.list')['windows']
                sessions = request(peer, 'session.list')['sessions']
                GLib.idle_add(self.loaded, peer, revision, catalog, windows, sessions)
            except (OSError, ValueError, EOFError, KeyError) as error:
                GLib.idle_add(self.failed, peer, revision, message(error))
        threading.Thread(target=fetch, daemon=True).start()

    def failed(self, peer, revision, error):
        if self.window and self.selected == peer and self.revision == revision:
            self.notice.set_text(error)
        return False

    def loaded(self, peer, revision, catalog, windows, sessions):
        if self.window and self.selected == peer and self.revision == revision:
            self.render(peer, catalog['apps'], windows, sessions)
            self.notice.set_text('已安装应用已缓存 · ' + time.strftime('%Y/%m/%d %H:%M', time.localtime(catalog['saved_at'])))
        return False

    def row(self, title, description, icon, action, callback, enabled=True):
        box = Gtk.Box(spacing=12, margin=10)
        image = Gtk.Image.new_from_icon_name('application-x-executable', Gtk.IconSize.DIALOG)
        try:
            if icon and len(icon) <= 24*1024:
                import base64
                raw = base64.b64decode(icon, validate=True)
                if len(raw)>=24 and len(raw)<=16*1024 and raw.startswith(b'\x89PNG\r\n\x1a\n'):
                    w,h=int.from_bytes(raw[16:20],'big'),int.from_bytes(raw[20:24],'big')
                    if 0<w<=128 and 0<h<=128:
                        loader=GdkPixbuf.PixbufLoader.new_with_type('png');loader.write(raw);loader.close();image.set_from_pixbuf(loader.get_pixbuf())
        except (ValueError, GLib.Error): pass
        box.pack_start(image, False, False, 0)
        label = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=4)
        label.pack_start(Gtk.Label(label=title[:256], xalign=0), False, False, 0)
        subtitle = Gtk.Label(label=description[:512], xalign=0);subtitle.set_ellipsize(3)
        label.pack_start(subtitle, False, False, 0);box.pack_start(label, True, True, 0)
        button = Gtk.Button(label=action);button.set_sensitive(enabled);button.connect('clicked', callback)
        box.pack_start(button, False, False, 0);self.rows.add(box)

    def render(self, peer, apps, windows, sessions):
        self.clear_rows(); known = set()
        for window in windows[:128]:
            sid = window.get('session_id'); known.add(sid)
            self.row(window['name'], window.get('title', ''), window.get('icon_png'), '已连接' if window.get('controlled') else '连接',
                lambda _, w=window: self.connect_window(peer, w), not window.get('controlled'))
        for session in sessions[:16]:
            if session['session_id'] in known: continue
            app = next((a for a in apps if a['id']==session['app_id']), {})
            self.row(app.get('name','Mac 应用'), '已有会话', app.get('icon_png'), '连接',
                lambda _, s=session, a=app: self.open_viewer(peer, a.get('name','Mac'), session_id=s['session_id']), not session.get('controlled'))
        for app in apps[:512]:
            self.row(app['name'], 'Mac 已安装应用', app.get('icon_png'), '打开', lambda _, a=app: self.launch(peer, a), not app.get('unavailable_reason'))
        self.rows.show_all()

    def connect_window(self, peer, window):
        if window.get('session_id'):
            self.open_viewer(peer, window['name'], session_id=window['session_id'])
        else:
            self.launch(peer, window)

    def launch(self, peer, app):
        key = peer + ':' + app['id']
        request_id = self.pending_launches.setdefault(key, uuid.uuid4().hex)
        save(self.launch_store, self.pending_launches)
        def start():
            try:
                response = request(peer, 'app.launch', 'session.started', app_id=app['id'], request_id=request_id, backend='native', attach=False)
                self.pending_launches.pop(key, None);save(self.launch_store, self.pending_launches)
                GLib.idle_add(self.open_viewer, peer, app['name'], response['session_id'])
            except (OSError, ValueError, EOFError) as error:
                GLib.idle_add(self.set_notice, message(error))
        threading.Thread(target=start, daemon=True).start()

    def open_viewer(self, peer, name, session_id=None, app_id=None):
        self.viewers = [v for v in self.viewers if not v.closed.is_set()]
        if len(self.viewers) >= 8:
            self.set_notice('当前接收窗口已达 8 个，请先断开不用的窗口。'); return False
        self.viewers.append(Viewer(self, peer, name, session_id=session_id, app_id=app_id))
        return False


if __name__ == '__main__':
    sys.exit(Manager().run(sys.argv))
