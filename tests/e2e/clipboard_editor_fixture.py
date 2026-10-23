#!/usr/bin/env python3
"""Disposable GTK editor for real selection/input testing on a test-owned X11 display."""
import pathlib
import json
import sys
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib, Pango

state = pathlib.Path(sys.argv[1])
window = Gtk.Window(title='ControllerTextFixture')
window.set_default_size(600, 400)
view = Gtk.TextView()
buffer = view.get_buffer()
def changed(buf):
    text = buf.get_text(buf.get_start_iter(), buf.get_end_iter(), True)
    state.write_text(text)
    if len(sys.argv) > 3:
        import time
        pathlib.Path(sys.argv[3]).write_text(json.dumps(dict(text=text, received=time.monotonic())))
buffer.connect('changed', changed)
scroll = Gtk.ScrolledWindow()
scroll.add(view)
window.add(scroll)
window.connect('destroy', Gtk.main_quit)
window.show_all()
view.grab_focus()
state.write_text('')
if len(sys.argv) > 2:
    metrics = pathlib.Path(sys.argv[2])
    def dimensions(*_):
        context = view.get_pango_context()
        font = context.get_metrics(context.get_font_description(), context.get_language())
        font_height = round((font.get_ascent() + font.get_descent()) / Pango.SCALE, 2)
        metrics.write_text(json.dumps(dict(scale=window.get_scale_factor(), width=window.get_allocated_width(), height=window.get_allocated_height(), font_height=font_height)))
        return False
    window.connect('size-allocate', dimensions)
    GLib.idle_add(dimensions)
Gtk.main()
