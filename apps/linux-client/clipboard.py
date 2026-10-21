"""Active clipboard operations, shared request ordering, safe bounded files."""
import base64
import os
from pathlib import Path
from urllib.parse import urlparse, unquote
import uuid
from wire import STATE, private

MAX = 8 * 1024 * 1024
generation = 0


def begin_copy():
    global generation
    generation += 1
    return generation


def validate(bundle):
    kind = bundle.get('kind')
    if kind == 'text':
        text = bundle.get('text')
        if not isinstance(text, str) or not text or len(text.encode()) > 8192 or '\0' in text:
            raise ValueError('clipboard_invalid')
        return []
    entries = bundle.get('entries')
    if kind not in ('image', 'files') or not isinstance(entries, list) or not 0 < len(entries) <= 256:
        raise ValueError('clipboard_invalid')
    names, directories, result, total = set(), set(), [], 0
    for entry in entries:
        path, size, directory = entry.get('path'), entry.get('size'), entry.get('directory')
        if not isinstance(path, str) or len(path.encode()) > 1024 or any(ord(c) < 32 or c == '\\' for c in path) or any(p in ('', '.', '..') for p in path.split('/')) or path.casefold() in names:
            raise ValueError('clipboard_invalid')
        if type(size) is not int or size < 0 or size > MAX - total or type(directory) is not bool:
            raise ValueError('clipboard_invalid')
        names.add(path.casefold())
        parent = path.rpartition('/')[0]
        if parent and parent not in directories:
            raise ValueError('clipboard_invalid')
        if directory:
            if size != 0:
                raise ValueError('clipboard_invalid')
            directories.add(path); data = b''
        else:
            encoded = entry.get('data', '')
            if not isinstance(encoded, str) or len(encoded) > 12 * 1024 * 1024:
                raise ValueError('clipboard_invalid')
            data = base64.b64decode(encoded, validate=True)
            if len(data) != size:
                raise ValueError('clipboard_invalid')
        total += size; result.append((path, directory, data))
    if kind == 'image':
        if len(result) != 1 or result[0][1]:
            raise ValueError('clipboard_invalid')
        png = result[0][2]
        if len(png) < 24 or not png.startswith(b'\x89PNG\r\n\x1a\n') or png[12:16] != b'IHDR':
            raise ValueError('clipboard_invalid')
        w, h = int.from_bytes(png[16:20], 'big'), int.from_bytes(png[20:24], 'big')
        if not 0 < w * h <= 8_000_000:
            raise ValueError('clipboard_invalid')
    return result


def read(board):
    uris = board.wait_for_uris()
    if uris:
        entries, total = [], 0
        def visit(path, name):
            nonlocal total
            if path.is_symlink() or not (path.is_dir() or path.is_file()) or len(entries) >= 256:
                raise ValueError('clipboard_invalid')
            directory = path.is_dir()
            size = 0 if directory else path.stat().st_size
            if size > MAX - total:
                raise ValueError('clipboard_invalid')
            entry = dict(path=name, size=size, directory=directory)
            if not directory:
                with path.open('rb') as source:
                    data = source.read(MAX - total + 1)
                if len(data) != size:
                    raise ValueError('clipboard_invalid')
                total += size; entry['data'] = base64.b64encode(data).decode()
            entries.append(entry)
            if directory:
                for child in sorted(path.iterdir()):
                    visit(child, name + '/' + child.name)
        for uri in uris:
            parsed = urlparse(uri)
            if parsed.scheme != 'file' or parsed.netloc not in ('', 'localhost'):
                raise ValueError('clipboard_invalid')
            path = Path(unquote(parsed.path)); visit(path, path.name)
        bundle = dict(kind='files', entries=entries)
    else:
        image = board.wait_for_image()
        if image:
            if image.get_width() * image.get_height() > 8_000_000:
                raise ValueError('clipboard_invalid')
            ok, data = image.save_to_bufferv('png', [], [])
            if not ok or len(data) > MAX:
                raise ValueError('clipboard_invalid')
            bundle = dict(kind='image', entries=[dict(path='image.png', size=len(data), directory=False, data=base64.b64encode(data).decode())])
        else:
            bundle = dict(kind='text', text=board.wait_for_text())
    validate(bundle)
    return bundle


def write(board, bundle):
    from gi.repository import GdkPixbuf, Gtk, Gdk
    entries = validate(bundle)
    if bundle['kind'] == 'text':
        board.set_text(bundle['text'], -1)
    elif bundle['kind'] == 'image':
        loader = GdkPixbuf.PixbufLoader.new_with_type('png')
        loader.write(entries[0][2]); loader.close()
        board.set_image(loader.get_pixbuf())
    else:
        directory = STATE / 'Clipboard' / uuid.uuid4().hex
        private(directory)
        for name, folder, data in entries:
            path = directory / name
            if folder:
                private(path)
            else:
                with open(path, 'xb', opener=lambda p, f: os.open(p, f, 0o600)) as target:
                    target.write(data)
        roots = [(directory / name).as_uri() for name, _, _ in entries if '/' not in name]
        # PyGObject doesn't expose gtk_clipboard_set_with_data callbacks on all
        # distro versions. Use a local text/uri-list selection owner window.
        owner = Gtk.Invisible(); owner.realize()
        targets = [Gtk.TargetEntry.new('text/uri-list', 0, 0), Gtk.TargetEntry.new('x-special/gnome-copied-files', 0, 1)]
        owner.selection_add_targets(Gdk.SELECTION_CLIPBOARD, targets)
        def provide(widget, selection, info, timestamp):
            text = ('copy\n' if info == 1 else '') + '\r\n'.join(roots) + '\r\n'
            selection.set(selection.get_target(), 8, text.encode())
        owner.connect('selection-get', provide)
        owner.connect('selection-clear-event', lambda *args: owner.destroy() or False)
        Gtk.selection_owner_set(owner, Gdk.SELECTION_CLIPBOARD, Gdk.CURRENT_TIME)
