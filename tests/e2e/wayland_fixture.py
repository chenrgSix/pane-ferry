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
                      ('io.paneferry.IMFixture' if '--im' in sys.argv else
                       'io.paneferry.OtherFixture' if '--other' in sys.argv else 'io.paneferry.WaylandFixture'))

def activate(app):
    win = Gtk.ApplicationWindow(application=app, title='PaneFerry Wayland fixture')
    win.set_default_size(700, 450)
    entry = Gtk.Entry(text='preserved fixture contents')
    # The compositor's XSettings can override GTK_IM_MODULE. Select the entry's
    # actual context so native text-input and unsupported contexts are tested.
    entry.set_property('im-module', 'wayland' if '--im' in sys.argv else 'gtk-im-context-simple')
    activated = []
    entry.connect('activate', lambda widget: activated.append(widget.get_text()))
    if '--delayed-paste' in sys.argv:
        def paste(widget):
            widget.stop_emission_by_name('paste-clipboard')
            GLib.timeout_add(150, lambda: (Gtk.Entry.do_paste_clipboard(widget), False)[1])
        entry.connect('paste-clipboard', paste)
    win.add(entry)
    win.show_all()
    def record():
        size = win.get_size()
        temporary = state.with_suffix('.tmp')
        temporary.write_text(json.dumps({'width': size.width, 'height': size.height,
                                    'scale': win.get_scale_factor(), 'text': entry.get_text(),
                                    'entry_width': entry.get_allocated_width(),
                                    'activated': activated,
                                    'selection': list(entry.get_selection_bounds()),
                                    'origin': list(win.get_window().get_origin())}))
        temporary.replace(state)
        return True
    GLib.timeout_add(100, record)
app.connect('activate', activate)
app.run([])
