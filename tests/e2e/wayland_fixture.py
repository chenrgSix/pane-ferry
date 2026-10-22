"""A native Wayland application used only by the isolated compositor test."""
import json
import sys
from pathlib import Path
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib
state = Path(sys.argv[1])
x11 = '--x11' in sys.argv
app = Gtk.Application(application_id='io.paneferry.X11Fixture' if x11 else
                      ('io.paneferry.OtherFixture' if '--other' in sys.argv else 'io.paneferry.WaylandFixture'))

def activate(app):
    win = Gtk.ApplicationWindow(application=app, title='PaneFerry Wayland fixture')
    win.set_default_size(700, 450)
    entry = Gtk.Entry(text='preserved fixture contents')
    win.add(entry)
    win.show_all()
    def record():
        size = win.get_size()
        state.write_text(json.dumps({'width': size.width, 'height': size.height,
                                    'scale': win.get_scale_factor(), 'text': entry.get_text(),
                                    'origin': list(win.get_window().get_origin())}))
        return True
    GLib.timeout_add(100, record)
app.connect('activate', activate)
app.run([])
