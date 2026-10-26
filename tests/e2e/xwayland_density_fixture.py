"""Disposable HiDPI visual ruler. Never loads user profiles or documents."""
import json
import sys
from pathlib import Path
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib

state = Path(sys.argv[1])
window = Gtk.Window(title='PaneFerry density controls fixture')
window.set_decorated(False)
window.set_default_size(800, 600)
fixed = Gtk.Fixed()
window.add(fixed)
marker = Gtk.DrawingArea()
marker.set_size_request(80, 30)
def draw(widget, context):
    context.set_source_rgb(1, 0, 0)
    context.paint()
marker.connect('draw', draw)
fixed.put(marker, 40, 40)
button = Gtk.Button(label='100 × 36')
button.set_size_request(100, 36)
fixed.put(button, 40, 100)
window.show_all()
from window_tree_fixture import install
install(window, state)
def record():
    size = window.get_size()
    data = dict(width=size.width, height=size.height, scale=window.get_scale_factor(),
                button_width=button.get_allocated_width(), button_height=button.get_allocated_height())
    temporary = state.with_suffix('.tmp')
    temporary.write_text(json.dumps(data)); temporary.replace(state)
    return True
GLib.timeout_add(50, record)
window.connect('destroy', Gtk.main_quit)
Gtk.main()
