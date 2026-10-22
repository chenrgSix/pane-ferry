"""Ubuntu native window receiver. No browser canvas or whole-desktop stream."""
import base64
import json
import queue
import threading
import time
import uuid
import gi
gi.require_version('Gtk', '3.0')
gi.require_version('Gst', '1.0')
gi.require_version('GstApp', '1.0')
from gi.repository import Gtk, Gdk, GLib, GdkPixbuf, Gst
import clipboard
from wire import Wire, message

Gst.init(None)


def bounds(width, height):
    return type(width) is int and type(height) is int and 0 < width <= 4096 and 0 < height <= 4096 and width * height <= 8_000_000


def check_sps(data, width, height):
    """Validate SPS before permitting a decoder to allocate picture buffers."""
    units = data.replace(b'\0\0\0\1', b'\0\0\1').split(b'\0\0\1')
    for unit in units:
        if not unit or unit[0] & 31 != 7:
            continue
        payload = unit[1:].replace(b'\0\0\3', b'\0\0')
        bits = ''.join(f'{value:08b}' for value in payload)
        offset = 0
        def read(n):
            nonlocal offset
            if n > 64 or offset + n > len(bits):
                raise ValueError('invalid_video')
            result = int(bits[offset:offset+n] or '0', 2); offset += n
            return result
        def ue():
            count = 0
            while read(1) == 0:
                count += 1
                if count > 24:
                    raise ValueError('invalid_video')
            return (1 << count) - 1 + read(count)
        profile = read(8); read(16); ue()
        if profile not in (66, 77, 88):
            raise ValueError('invalid_video')
        ue(); order = ue()
        if order == 0:
            ue()
        elif order == 1:
            read(1); ue(); ue()
            for _ in range(min(ue(), 256)):
                ue()
        elif order > 2:
            raise ValueError('invalid_video')
        if ue() > 16:
            raise ValueError('invalid_video')
        read(1)
        w = (ue() + 1) * 16; h = (ue() + 1) * 16
        frame = read(1)
        if not frame:
            read(1); h *= 2
        read(1)
        if read(1):
            left, right, top, bottom = ue(), ue(), ue(), ue()
            w -= (left + right) * 2
            h -= (top + bottom) * 2 * (2 - frame)
        if (w, h) != (width, height) or not bounds(w, h):
            raise ValueError('invalid_video')


class Video:
    def __init__(self, receive):
        decoder = 'avdec_h264' if Gst.ElementFactory.find('avdec_h264') else 'openh264dec'
        if not Gst.ElementFactory.find(decoder):
            raise ValueError('video_decoder_unavailable')
        self.receive = receive
        self.stream = None; self.size = None
        self.pipeline = Gst.parse_launch('appsrc name=source is-live=true format=time block=true max-buffers=4 '
            'caps=video/x-h264,stream-format=byte-stream,alignment=au ! h264parse ! ' + decoder +
            ' ! videoconvert ! video/x-raw,format=RGB ! appsink name=sink emit-signals=true sync=false max-buffers=1 drop=true')
        self.source = self.pipeline.get_by_name('source')
        self.pipeline.get_by_name('sink').connect('new-sample', self.sample)
        self.pipeline.set_state(Gst.State.PLAYING)

    def push(self, event):
        width, height = event.get('width'), event.get('height')
        if not bounds(width, height) or (event.get('encoded_width'), event.get('encoded_height')) != (width + width % 2, height + height % 2):
            raise ValueError('invalid_video')
        data = base64.b64decode(event['data'], validate=True)
        if len(data) > 12 * 1024 * 1024 or not data.startswith((b'\0\0\1', b'\0\0\0\1')):
            raise ValueError('invalid_video')
        if event['stream_id'] != self.stream:
            if not event.get('key_frame'):
                raise ValueError('invalid_video')
            check_sps(data, width + width % 2, height + height % 2)
            # Flush the previous GOP and discard queued decoded images on resize.
            self.pipeline.set_state(Gst.State.READY)
            self.pipeline.set_state(Gst.State.PLAYING)
            self.stream = event['stream_id']
        elif event.get('key_frame'):
            check_sps(data, width + width % 2, height + height % 2)
        self.size = width, height
        buffer = Gst.Buffer.new_wrapped(data)
        if self.source.emit('push-buffer', buffer) != Gst.FlowReturn.OK:
            raise ValueError('video_decoder_failed')

    def sample(self, sink):
        sample = sink.emit('pull-sample')
        if not sample or not self.size:
            return Gst.FlowReturn.OK
        caps = sample.get_caps().get_structure(0)
        width, height = caps.get_value('width'), caps.get_value('height')
        w, h = self.size
        if (width, height) != (w + w % 2, h + h % 2):
            return Gst.FlowReturn.ERROR
        buffer = sample.get_buffer(); raw = buffer.extract_dup(0, buffer.get_size())
        stride = ((width * 3 + 3) // 4) * 4
        if len(raw) < stride * height:
            return Gst.FlowReturn.ERROR
        pixels = GdkPixbuf.Pixbuf.new_from_bytes(GLib.Bytes.new(raw), GdkPixbuf.Colorspace.RGB, False, 8, width, height, stride)
        self.receive(pixels.new_subpixbuf(0, 0, w, h))
        return Gst.FlowReturn.OK

    def close(self):
        self.pipeline.set_state(Gst.State.NULL)


class Viewer:
    def __init__(self, application, peer_id, name, session_id=None, app_id=None, owner=None):
        self.owner = owner or self
        self.application = application
        self.members = {} if owner is None else owner.members
        self.frame_token = None; self.destroyed = False
        self.peer_id, self.name = peer_id, name
        self.session_id, self.app_id = session_id, app_id
        self.request_id = uuid.uuid4().hex
        self.closed = threading.Event(); self.ready = False
        self.outbound = queue.Queue(maxsize=128) if owner is None else owner.outbound
        self.window_id = None; self.revision = 0; self.width = 0; self.height = 0
        self.pressed = set(); self.videos = {}; self.wire = None
        self.display_lock = threading.Lock(); self.pending_image = None; self.display_scheduled = False
        self.copy_request = None; self.waiting_paste = False; self.resize_timer = None
        self.last_requested = None; self.epoch = 0
        self.window = Gtk.ApplicationWindow(application=application, title=name + ' · 连接中')
        self.window.set_wmclass('lan-remoteapp-receiver', 'LAN RemoteApp')
        self.window.set_default_size(900, 650)
        self.box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.notice = Gtk.Label(label='正在连接 ' + name)
        self.view = Gtk.DrawingArea(); self.view.set_can_focus(True)
        self.box.pack_start(self.notice, False, False, 0); self.box.pack_start(self.view, True, True, 0)
        self.window.add(self.box)
        self.pixels = None; self.preedit = ''; self.anchor = (30, 30)
        self.im = Gtk.IMMulticontext(); self.im.set_use_preedit(True)
        self.im.connect('commit', lambda context, text: self.send(type='input.text.commit', text=text))
        self.im.connect('preedit-changed', self.preedit_changed)
        self.view.add_events(Gdk.EventMask.POINTER_MOTION_MASK | Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.BUTTON_RELEASE_MASK |
            Gdk.EventMask.SCROLL_MASK | Gdk.EventMask.SMOOTH_SCROLL_MASK | Gdk.EventMask.KEY_PRESS_MASK | Gdk.EventMask.KEY_RELEASE_MASK | Gdk.EventMask.FOCUS_CHANGE_MASK)
        self.view.connect('realize', lambda widget: self.im.set_client_window(widget.get_window()))
        self.view.connect('draw', self.draw)
        self.view.connect('size-allocate', self.allocated)
        self.view.connect('button-press-event', lambda widget, event: self.pointer(event, event.button, True))
        self.view.connect('button-release-event', lambda widget, event: self.pointer(event, event.button, False))
        self.view.connect('motion-notify-event', lambda widget, event: self.pointer(event, 0, False))
        self.view.connect('scroll-event', self.scroll)
        self.view.connect('key-press-event', lambda widget, event: self.key(event, True))
        self.view.connect('key-release-event', lambda widget, event: self.key(event, False))
        self.view.connect('focus-in-event', lambda *args: self.im.focus_in() or self.send(type='window.focus'))
        self.view.connect('focus-out-event', self.focus_out)
        self.window.connect('delete-event', self.close)
        self.window.show_all(); self.view.grab_focus()
        if owner is None:
            threading.Thread(target=self.run, daemon=True).start()

    def preedit_changed(self, context):
        self.preedit = context.get_preedit_string()[0]; self.view.queue_draw()

    def draw(self, widget, context):
        allocation = widget.get_allocation()
        context.set_source_rgb(0.1, 0.11, 0.12); context.paint()
        if self.pixels:
            context.save(); context.scale(allocation.width / self.pixels.get_width(), allocation.height / self.pixels.get_height())
            Gdk.cairo_set_source_pixbuf(context, self.pixels, 0, 0); context.paint(); context.restore()
        if self.preedit:
            x, y = self.anchor
            context.set_source_rgb(1, 1, 1); context.rectangle(x, y, min(300, len(self.preedit)*20+20), 28); context.fill()
            context.set_source_rgb(0, 0, 0); context.move_to(x+5, y+20); context.set_font_size(18); context.show_text(self.preedit)
        return False

    def image(self, pixels, epoch, revision, token):
        if epoch != self.owner.epoch or revision != self.revision or token is not self.frame_token or self.closed.is_set():
            return
        with self.display_lock:
            self.pending_image = (pixels, epoch, revision, token)
            if self.display_scheduled:
                return
            self.display_scheduled = True
        GLib.idle_add(self.display)

    def display(self):
        with self.display_lock:
            pending = self.pending_image; self.pending_image = None; self.display_scheduled = False
        if not self.closed.is_set() and pending:
            pixels, epoch, revision, token = pending
            if epoch == self.owner.epoch and revision == self.revision and token is self.frame_token:
                self.pixels = pixels; self.view.queue_draw()
        return False

    def status(self, text):
        if not self.destroyed:
            self.notice.set_text(text)
        return False

    def send(self, **value):
        if not self.ready or self.window_id is None or self.closed.is_set() or self.owner.closed.is_set():
            return False
        value['window_id'] = self.window_id
        try:
            self.outbound.put_nowait((self.owner.epoch, value))
        except queue.Full:
            # Never drop a key-up while keeping later input. Reattach cleanly.
            if self.owner.wire:
                try: self.owner.wire.socket.shutdown(2)
                except OSError: pass
        return False

    def metadata(self, value, epoch):
        if epoch != self.owner.epoch or self.closed.is_set():
            return False
        self.window_id = value['window_id']; self.width = value['width']; self.height = value['height']; self.revision = value['geometry_revision']
        self.window.set_title(str(value.get('title', self.name))[:512] + ' · ' + self.name)
        self.ready = True
        self.status('已连接 · H.264 · ' + ('关闭此窗口仅断开连接' if self is self.owner else '关闭此窗口将关闭远程预览'))
        if value.get('visible', True): self.window.show_all()
        else: self.window.hide()
        self.allocated(self.view, self.view.get_allocation())
        return False

    def on_ui(self, action, *args):
        """Serialize membership changes before allowing frames or input for them."""
        done = threading.Event(); result = []
        def apply():
            try: result.append((True, action(*args)))
            except Exception as error: result.append((False, error))
            finally: done.set()
            return False
        GLib.idle_add(apply)
        deadline = time.monotonic() + 10
        while not done.wait(.05):
            if self.closed.is_set() or time.monotonic() > deadline:
                raise EOFError('connection_lost')
        ok, value = result[0]
        if not ok: raise value
        return value

    @staticmethod
    def validate_windows(values):
        if not isinstance(values, list) or not 1 <= len(values) <= 16:
            raise ValueError('protocol_error')
        catalog = {}
        pixels = 0
        for value in values:
            if not isinstance(value, dict): raise ValueError('protocol_error')
            wid = value.get('window_id'); revision = value.get('geometry_revision')
            if type(wid) is not int or not 0 < wid <= 0xffffffff or wid in catalog or type(revision) is not int or not 0 < revision < 2**64 or not bounds(value.get('width'), value.get('height')):
                raise ValueError('protocol_error')
            pixels += value['width'] * value['height']; catalog[wid] = value
        if pixels > 16_000_000:
            raise ValueError('protocol_error')
        for wid, value in catalog.items():
            seen = {wid}; parent = value.get('parent_id')
            while parent is not None:
                if type(parent) is not int or parent not in catalog or parent in seen:
                    raise ValueError('protocol_error')
                seen.add(parent); parent = catalog[parent].get('parent_id')
        return catalog

    def apply_members(self, catalog, epoch):
        if epoch != self.epoch or self.closed.is_set(): return
        if self.window_id is None:
            self.window_id = next((wid for wid, value in catalog.items() if value.get('parent_id') is None), None)
        if self.window_id not in catalog:
            raise ValueError('session_exited')
        for wid in list(self.members):
            if wid not in catalog:
                self.members.pop(wid).destroy_local()
        for wid, metadata in catalog.items():
            viewer = self.members.get(wid)
            if viewer is None:
                viewer = self if wid == self.window_id else type(self)(self.application, self.peer_id, self.name, session_id=self.session_id, owner=self)
                self.members[wid] = viewer
            viewer.metadata(metadata, epoch)
        for wid, metadata in catalog.items():
            parent = self.members.get(metadata.get('parent_id'))
            self.members[wid].window.set_transient_for(parent.window if parent else None)

    def invalidate(self, text):
        for viewer in list(self.members.values()) or [self]:
            viewer.ready = False; viewer.frame_token = None; viewer.last_requested = None
            viewer.pressed.clear(); viewer.cancel_copy(); viewer.status(text)
            with viewer.display_lock: viewer.pending_image = None
        return False

    def allocated(self, widget, allocation):
        if self.resize_timer:
            GLib.source_remove(self.resize_timer)
        def resize():
            self.resize_timer = None
            scale = widget.get_scale_factor()
            width, height = allocation.width*scale, allocation.height*scale
            ratio = min(1, 4096/max(1,width), 4096/max(1,height), (8_000_000/max(1,width*height))**0.5)
            target = max(1,int(width*ratio)), max(1,int(height*ratio))
            if self.ready and target != self.last_requested:
                self.last_requested = target
                self.send(type='window.configure', width=target[0], height=target[1])
            return False
        self.resize_timer = GLib.timeout_add(100, resize)

    def pointer(self, event, button, pressed):
        if not self.ready or not self.width:
            return True
        if pressed:
            self.view.grab_focus(); self.anchor = (int(event.x), int(event.y))
            rect = Gdk.Rectangle(); rect.x, rect.y, rect.width, rect.height = int(event.x), int(event.y), 1, 24
            self.im.set_cursor_location(rect)
        allocation = self.view.get_allocation()
        x = max(0, min(self.width-1, int(event.x*self.width/max(1,allocation.width))))
        y = max(0, min(self.height-1, int(event.y*self.height/max(1,allocation.height))))
        self.send(type='input.pointer', x=x, y=y, button=button, pressed=pressed)
        return True

    def scroll(self, widget, event):
        button = {Gdk.ScrollDirection.UP:4, Gdk.ScrollDirection.DOWN:5, Gdk.ScrollDirection.LEFT:6, Gdk.ScrollDirection.RIGHT:7}.get(event.direction)
        if button is None:
            _, dx, dy = event.get_scroll_deltas()
            button = (4 if dy<0 else 5) if abs(dy)>=abs(dx) else (6 if dx<0 else 7)
        self.pointer(event, button, True); self.pointer(event, button, False)
        return True

    def release_keys(self):
        for key in self.pressed.copy():
            self.send(type='input.key.logical', key=key, pressed=False)
        self.pressed.clear()

    def focus_out(self, *args):
        self.im.focus_out(); self.im.reset(); self.release_keys()
        return False

    def key(self, event, pressed):
        name = Gdk.keyval_name(event.keyval) or ''
        control = event.state & Gdk.ModifierType.CONTROL_MASK
        if pressed and control and name.lower() in ('c', 'x', 'v'):
            self.release_keys()
            if name.lower() == 'v':
                if self.copy_request:
                    self.waiting_paste = True
                else:
                    self.paste()
            else:
                request_id = uuid.uuid4().hex
                self.copy_request = (request_id, clipboard.begin_copy())
                self.send(type='clipboard.inline.copy', request_id=request_id, cut=name.lower() == 'x')
            return True
        if not control and self.im.filter_keypress(event):
            return True
        if name:
            if pressed:
                self.pressed.add(name)
            else:
                if name not in self.pressed:
                    return True
                self.pressed.remove(name)
            self.send(type='input.key.logical', key=name, pressed=pressed)
        return True

    def paste(self):
        try:
            bundle = clipboard.read(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD))
            self.send(type='clipboard.inline.paste', bundle=bundle)
        except (ValueError, OSError, GLib.Error):
            self.status(message('clipboard_invalid'))

    def copied(self, event):
        pending = self.copy_request
        if not pending or event.get('request_id') != pending[0] or event.get('window_id') != self.window_id:
            return False
        self.copy_request = None
        if pending[1] == clipboard.generation:
            try:
                clipboard.write(Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD), event['bundle'])
            except (ValueError, OSError, GLib.Error):
                self.status(message('clipboard_invalid')); self.waiting_paste = False
        if self.waiting_paste:
            self.waiting_paste = False; self.paste()
        return False

    def run(self):
        attempt = 0
        retired = set()
        while not self.closed.is_set():
            self.epoch += 1; epoch = self.epoch
            wire = None; sender_stop = threading.Event(); sender = None
            catalog = {}; failure = message('connection_lost')
            try:
                wire = Wire(self.peer_id); self.wire = wire
                if 'input.key.logical' not in wire.capabilities or 'native.h264' not in wire.capabilities:
                    raise ValueError('protocol_error')
                wire.send(type='video.enable', codec='h264'); wire.until('video.ready')
                if not self.session_id:
                    wire.send(type='app.launch', app_id=self.app_id, request_id=self.request_id, backend='native', attach=False)
                    self.session_id = wire.until('session.started')['session_id']
                wire.send(type='session.resume', session_id=self.session_id)
                snapshot = wire.until('session.snapshot')
                if snapshot['session_id'] != self.session_id or not snapshot.get('windows'):
                    raise ValueError('session_exited')
                catalog = self.validate_windows(snapshot['windows'])
                if retired.intersection(catalog): raise ValueError('protocol_error')
                self.on_ui(self.apply_members, catalog, epoch)
                attempt = 0
                def send_loop(wire=wire, sender_stop=sender_stop):
                    while not sender_stop.is_set() and not self.closed.is_set():
                        try: queued_epoch, value = self.outbound.get(timeout=.2)
                        except queue.Empty: continue
                        if queued_epoch != epoch or value['window_id'] not in catalog: continue
                        try: wire.send(**value)
                        except (OSError, ValueError):
                            try: wire.socket.shutdown(2)
                            except OSError: pass
                            return
                sender = threading.Thread(target=send_loop, daemon=True); sender.start()
                while not self.closed.is_set():
                    event = wire.read(); kind = event.get('type'); wid = event.get('window_id')
                    if event.get('session_id') not in ('', self.session_id) or (kind != 'error' and event.get('session_id') != self.session_id):
                        raise ValueError('wrong_session')
                    if kind == 'frame':
                        metadata = catalog.get(wid)
                        if metadata is None or event.get('geometry_revision') != metadata['geometry_revision']:
                            continue
                        if (event.get('width'), event.get('height')) != (metadata['width'], metadata['height']) or event.get('codec') != 'h264':
                            raise ValueError('invalid_video')
                        if type(event.get('stream_id')) is not int or not 0 < event['stream_id'] < 2**64 or type(event.get('key_frame')) is not bool:
                            raise ValueError('invalid_video')
                        revision = metadata['geometry_revision']
                        video = self.videos.get(wid)
                        stamp = (revision, event.get('stream_id'))
                        if video is None or video.stamp != stamp:
                            if video: video.close()
                            viewer = self.members[wid]; token = object()
                            self.on_ui(setattr, viewer, 'frame_token', token)
                            video = self.videos[wid] = Video(lambda pixels, viewer=viewer, epoch=epoch, revision=revision, token=token: viewer.image(pixels, epoch, revision, token))
                            video.stamp = stamp
                        video.push(event)
                    elif kind in ('window.created', 'window.metadata'):
                        if wid in retired: continue
                        if kind == 'window.metadata' and wid not in catalog: continue
                        if wid in catalog and event.get('geometry_revision', 0) < catalog[wid]['geometry_revision']: continue
                        updated = dict(catalog); updated[wid] = event
                        updated = self.validate_windows(list(updated.values()))
                        self.on_ui(self.apply_members, updated, epoch); catalog = updated
                    elif kind == 'window.destroyed':
                        if wid not in catalog: continue
                        if wid == self.window_id: raise ValueError('session_exited')
                        removed = {wid}
                        while True:
                            descendants = {key for key, value in catalog.items() if value.get('parent_id') in removed}
                            if descendants <= removed: break
                            removed |= descendants
                        for key in removed:
                            catalog.pop(key, None); retired.add(key)
                            video = self.videos.pop(key, None)
                            if video: video.close()
                        if len(retired) > 128: raise ValueError('protocol_error')
                        self.on_ui(self.apply_members, catalog, epoch)
                    elif kind == 'clipboard.inline.data':
                        if wid in catalog: self.on_ui(self.members[wid].copied, event)
                    elif kind == 'error':
                        code = event.get('code', '')
                        viewer = self.members.get(wid, self)
                        self.on_ui(viewer.status, message(code))
                        if code in ('session_exited', 'session_expired', 'sharing_disabled', 'desktop_locked'):
                            raise ValueError(code)
                        if code.startswith('clipboard_'):
                            if wid in catalog: self.on_ui(viewer.cancel_copy)
                            else:
                                for member in list(self.members.values()): self.on_ui(member.cancel_copy)
            except (OSError, ValueError, EOFError, KeyError, TypeError, GLib.Error) as error:
                self.epoch += 1; failure = message(error)
                if str(error) in ('session_exited', 'session_expired', 'session_controlled', 'sharing_disabled', 'protocol_error', 'wrong_session'):
                    self.closed.set()
            finally:
                sender_stop.set(); self.epoch += 1
                if wire: wire.close()
                if sender: sender.join(timeout=1)
                self.wire = None
                for video in self.videos.values(): video.close()
                self.videos.clear()
                while not self.outbound.empty():
                    try: self.outbound.get_nowait()
                    except queue.Empty: break
                GLib.idle_add(self.invalidate, failure)
            if not self.closed.is_set():
                self.closed.wait([1, 2, 4, 8, 15][min(attempt, 4)]); attempt += 1

    def cancel_copy(self):
        self.copy_request = None; self.waiting_paste = False
        return False

    def destroy_local(self):
        self.ready = False; self.closed.set(); self.frame_token = None; self.destroyed = True
        if self.resize_timer:
            GLib.source_remove(self.resize_timer); self.resize_timer = None
        self.cancel_copy(); self.window.destroy()

    def close(self, *args):
        if self is not self.owner:
            self.release_keys(); self.send(type='window.close.request')
            return True
        self.ready = False; self.closed.set()
        if self.wire:
            try: self.wire.socket.shutdown(2)
            except OSError: pass
        for viewer in list(self.members.values()): viewer.destroy_local()
        self.members.clear(); self.destroy_local()
        return True
