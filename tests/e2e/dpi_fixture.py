"""Test-owned GTK UI; records client logical geometry, not controller metadata."""
import json
import os
from pathlib import Path
import sys
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib

state = Path(sys.argv[1])
win = Gtk.Window(title='PaneFerry DPI fixture')
win.set_default_size(700, 450)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=12)
box.set_border_width(16)
entry = Gtk.Entry(text='preserved original contents')
label = Gtk.Label(label='Readable text / 界面文字 0123456789')
button = Gtk.Button(label='A normal control')
for widget in (label, entry, button):
    box.pack_start(widget, False, False, 0)
win.add(box)
win.show_all()

def record():
    size = win.get_size()
    temporary = state.with_suffix('.tmp')
    temporary.write_text(json.dumps(dict(pid=os.getpid(), width=size.width, height=size.height,
        scale=win.get_scale_factor(), button_height=button.get_allocated_height(),
        text_pixels=list(label.get_layout().get_pixel_size()), text=entry.get_text())))
    temporary.replace(state)
    return True

GLib.timeout_add(100, record)
win.connect('destroy', Gtk.main_quit)
Gtk.main()
