"""Three windows owned by one process on a test-only display."""
import json
from pathlib import Path
import sys
import gi
gi.require_version('Gtk', '3.0')
gi.require_version('GdkX11', '3.0')
from gi.repository import Gtk, GdkX11, GLib, GdkPixbuf

GLib.set_prgname('remoteapp-discovery-fixture')
path = Path(sys.argv[1])
icon = GdkPixbuf.Pixbuf.new(GdkPixbuf.Colorspace.RGB, True, 8, 32, 32)
icon.fill(0x3C855FFF)
Gtk.Window.set_default_icon(icon)
windows, entries = {}, {}
for i, name in enumerate(('DiscoveryAlpha', 'DiscoveryBeta', 'DiscoveryDialog')):
    window = Gtk.Window(title=name)
    window.set_default_size(360, 220)
    window.move(30 + i*390, 50)
    entry = Gtk.Entry()
    window.add(entry)
    windows[name], entries[name] = window, entry
windows['DiscoveryDialog'].set_transient_for(windows['DiscoveryAlpha'])
windows['DiscoveryDialog'].set_destroy_with_parent(True)
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
