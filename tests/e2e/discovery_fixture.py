"""Three windows owned by one process on a test-only display."""
import json
from pathlib import Path
import sys
import subprocess
import gi
gi.require_version('Gtk', '3.0')
gi.require_version('GdkX11', '3.0')
from gi.repository import Gtk, GdkX11, GLib, GdkPixbuf, Gdk

GLib.set_prgname('paneferry-discovery-fixture')
path = Path(sys.argv[1])
icon = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, True, 8, 32, 32)
icon.fill(0x3C855FFF)
Gtk.Window.set_default_icon(icon)
windows, entries = {}, {}
child = '--child' in sys.argv
helpers = []
names = ('DiscoveryPreview',) if child else ('DiscoveryAlpha', 'DiscoveryBeta', 'DiscoveryDialog')
for i, name in enumerate(names):
    window = Gtk.Window(title=name)
    window.set_default_size(360, 220)
    window.move(30 + i*390, 50)
    entry = Gtk.Entry()
    window.add(entry)
    windows[name], entries[name] = window, entry
if child:
    windows['DiscoveryPreview'].connect('destroy', lambda *_: Gtk.main_quit())
else:
    windows['DiscoveryDialog'].set_transient_for(windows['DiscoveryAlpha'])
    windows['DiscoveryDialog'].set_destroy_with_parent(True)
    def open_preview(window, event):
        if event.keyval != Gdk.KEY_F2:
            return False
        if not any(p.poll() is None for p in helpers):
            helpers.append(subprocess.Popen([sys.executable, __file__, str(path.with_suffix('.child.json')), '--child']))
        return True
    for window in windows.values():
        window.connect('key-press-event', open_preview)
for window in windows.values():
    window.show_all()


def update():
    state = {}
    for name, window in windows.items():
        native = window.get_window()
        if native:
            geometry = native.get_geometry()
            state[name] = dict(id=native.get_xid(), width=geometry.width, height=geometry.height,
                               text=entries[name].get_text())
    temporary = path.with_suffix('.tmp')
    temporary.write_text(json.dumps(state))
    temporary.replace(path)
    return True


GLib.timeout_add(30, update)
Gtk.main()
if child:
    path.unlink(missing_ok=True)
