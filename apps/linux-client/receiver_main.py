"""GTK receiving windows controlled by the shared Tauri management UI."""
import argparse
import atexit
import concurrent.futures
import json
import os
from pathlib import Path
import select
import socket
import sys
import uuid

# -I keeps user/site paths out; only this installed component directory is added.
sys.path.insert(0, str(Path(__file__).resolve().parent))
from receiver_broker import Broker, valid_id
from wire import STATE, private, save
from windowhost import Viewer, Gtk, GLib, GdkPixbuf
from gi.repository import Gio


class ReceiverApplication(Gtk.Application):
    def __init__(self, options):
        super().__init__(application_id='io.lanremoteapp.host.g' + options.application_group,
                         flags=Gio.ApplicationFlags.NON_UNIQUE)
        self.options = options; self.sessions = {}; self.broker = None
        self.source_size = options.picture_mode == 'source'
        self.video_color = options.video_color; self.requested_fps = options.fps
        self.background_start = options.background_start
        self.connect('activate', self.activate)
        self.connect('shutdown', self.shutdown)

    def activate(self, *_):
        if self.broker: return
        self.hold()
        self.broker = Broker(self.options.application_group, self.handle_request)
        # The launcher's attach request creates the first window, carrying its
        # exact presentation and frame-rate settings. No remote app is launched.
        GLib.timeout_add_seconds(10, self.quit_if_idle)

    def handle_request(self, request, client):
        pending = concurrent.futures.Future()
        def on_ui():
            if not pending.set_running_or_notify_cancel(): return False
            try: pending.set_result(self.dispatch(request, client))
            except Exception as error: pending.set_exception(error)
            return False
        GLib.idle_add(on_ui)
        try: return pending.result(timeout=4)
        except concurrent.futures.TimeoutError:
            pending.cancel(); return False

    def dispatch(self, request, client):
        op = request['op']
        if op == 'list':
            Broker.send(client, dict(sessions=[dict(session=session, target=value['target'])
                for session, value in self.sessions.items() if not value['viewer'].closed.is_set()]))
            return False
        session = request['session']; entry = self.sessions.get(session)
        if op == 'detach':
            if entry: entry['viewer'].close()
            Broker.send(client, dict(session=session, detached=True)); return False
        if entry and entry['viewer'].closed.is_set():
            self.closed(session, entry['viewer']); entry = None
        if not entry:
            if len(self.sessions) >= 64: raise ValueError('receiver_limit')
            self.source_size = request['picture_mode'] == 'source'
            self.video_color = request['video_color']; self.requested_fps = request['fps']
            self.background_start = request['background']
            viewer = Viewer(self, 'quic:' + self.options.native_peer, self.options.name, session_id=session)
            entry = dict(viewer=viewer, target=request.get('target'), watchers=set())
            self.sessions[session] = entry
            viewer.window.connect('destroy', lambda *_: self.closed(session, viewer))
        watchers = entry['watchers']
        for watcher in list(watchers):
            try:
                if select.select([watcher], [], [], 0)[0] and not watcher.recv(1, socket.MSG_PEEK):
                    watchers.remove(watcher); self.broker.drop(watcher)
            except OSError:
                watchers.remove(watcher); self.broker.drop(watcher)
        if len(watchers) >= 16: raise ValueError('receiver_watch_limit')
        Broker.send(client, dict(pid=os.getpid(), session=session))
        watchers.add(client)
        if not request['background']:
            entry['viewer'].window.present()
            entry['viewer'].view.grab_focus()
        return True

    def closed(self, session, viewer):
        entry = self.sessions.get(session)
        if not entry or entry['viewer'] is not viewer: return
        del self.sessions[session]
        for watcher in entry['watchers']:
            try: Broker.send(watcher, dict(session=session, closed=True))
            except OSError: pass
            self.broker.drop(watcher)
        if not self.sessions: GLib.timeout_add(1000, self.quit_if_idle)

    def quit_if_idle(self):
        if not self.sessions: self.quit()
        return False

    def shutdown(self, *_):
        for entry in list(self.sessions.values()): entry['viewer'].close()
        if self.broker: self.broker.close()


def parse_options(arguments):
    parser = argparse.ArgumentParser(description='PaneFerry native GTK receiver')
    parser.add_argument('--native-peer', required=True)
    parser.add_argument('--native-state', type=Path, required=True)
    parser.add_argument('--native-helper', type=Path, required=True)
    parser.add_argument('--session', required=True)
    parser.add_argument('--application-group', required=True)
    parser.add_argument('--source-target')
    parser.add_argument('--name', default='远端应用')
    parser.add_argument('--fps', type=int, choices=(30, 60), default=60)
    parser.add_argument('--picture-mode', choices=('adapt', 'source'), default='adapt')
    parser.add_argument('--video-color', choices=('420', '444-atlas-v1'), default='420')
    parser.add_argument('--background-start', action='store_true')
    parser.add_argument('--icon-stdin', action='store_true')
    options = parser.parse_args(arguments)
    if str(uuid.UUID(options.native_peer)) != options.native_peer or not valid_id(options.session):
        raise ValueError('invalid_receiver_identity')
    if options.native_state != STATE / 'network': raise ValueError('invalid_receiver_state')
    options.name = ''.join(c for c in options.name if c.isprintable())[:128] or '远端应用'
    return options


def main(arguments=None):
    options = parse_options(sys.argv[1:] if arguments is None else arguments)
    private(STATE / 'receivers')
    started = int(Path('/proc/self/stat').read_text().rsplit(')', 1)[1].split()[19])
    registration = STATE / 'receivers' / f'{os.getpid()}.json'
    save(registration, dict(pid=os.getpid(), started=started))
    atexit.register(lambda: registration.unlink(missing_ok=True))
    if options.icon_stdin:
        import base64
        try:
            raw = base64.b64decode(sys.stdin.buffer.read(24 * 1024 + 1), validate=True)
            if len(raw) <= 16 * 1024 and raw.startswith(b'\x89PNG\r\n\x1a\n') and len(raw) >= 24:
                width, height = int.from_bytes(raw[16:20], 'big'), int.from_bytes(raw[20:24], 'big')
                if 0 < width <= 128 and 0 < height <= 128:
                    loader = GdkPixbuf.PixbufLoader.new_with_type('png'); loader.write(raw); loader.close()
                    Gtk.Window.set_default_icon(loader.get_pixbuf())
        except (ValueError, GLib.Error): pass
    return ReceiverApplication(options).run([])


if __name__ == '__main__': raise SystemExit(main())
