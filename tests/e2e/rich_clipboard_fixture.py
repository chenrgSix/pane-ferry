"""Own GTK selection owner/requestor for rich clipboard integration tests."""
import base64
import hashlib
import json
from pathlib import Path
import sys
import urllib.parse
import gi
gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
gi.require_version('GdkX11', '3.0')
from gi.repository import Gtk, Gdk, GdkX11, GLib
root = Path(sys.argv[1])
window = Gtk.Window(title='RemoteApp rich clipboard fixture')
window.set_default_size(500, 300)
area = Gtk.DrawingArea(); area.set_can_focus(True); window.add(area)
clipboard = Gdk.Atom.intern('CLIPBOARD', False)
result = root/'result.json'
state = root/'state.json'

def publish(value):
    temporary=result.with_suffix('.tmp'); temporary.write_text(json.dumps(value)); temporary.replace(result)

def selection_get(widget, data, info, timestamp):
    config = json.loads((root/'command.json').read_text())
    mime = config['mime']
    payload = (root/config['source']).read_bytes()
    data.set(Gdk.Atom.intern(mime, False), 8, payload)

area.connect('selection-get', selection_get)

def keys(widget, event):
    if not event.state & Gdk.ModifierType.CONTROL_MASK:
        return False
    config=json.loads((root/'command.json').read_text())
    if event.keyval in (Gdk.KEY_c, Gdk.KEY_x):
        mime=config['mime']
        Gtk.selection_clear_targets(area, clipboard)
        Gtk.selection_add_target(area, clipboard, Gdk.Atom.intern(mime, False), 0)
        Gtk.selection_owner_set(area, clipboard, event.time)
        if event.keyval==Gdk.KEY_x:
            (root/'cut-called').touch()
        return True
    if event.keyval == Gdk.KEY_v:
        def received(board, data, _):
            value=data.get_data()
            if value is None:
                publish({'error':'no data'}); return
            value=bytes(value)
            answer={'mime':config['request'], 'bytes':len(value), 'sha256':hashlib.sha256(value).hexdigest()}
            if config['request']=='text/uri-list':
                files={}
                for uri in value.decode().splitlines():
                    if not uri or uri.startswith('#'): continue
                    path=Path(urllib.parse.unquote(urllib.parse.urlparse(uri).path))
                    if path.is_dir():
                        for child in path.rglob('*'):
                            if child.is_file(): files[path.name+'/'+str(child.relative_to(path))]=hashlib.sha256(child.read_bytes()).hexdigest()
                    else: files[path.name]=hashlib.sha256(path.read_bytes()).hexdigest()
                answer['files']=files
            publish(answer)
        Gtk.Clipboard.get(clipboard).request_contents(Gdk.Atom.intern(config['request'],False),received,None)
        return True
    return False
window.connect('key-press-event', keys)
window.connect('destroy', Gtk.main_quit)
window.show_all(); area.grab_focus()
GLib.idle_add(lambda: (state.write_text(json.dumps({'window':window.get_window().get_xid()})),False)[1])
Gtk.main()
