#!/usr/bin/env python3
"""Disposable GTK editor for real selection/input testing on a test-owned X11 display."""
import pathlib
import sys
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk

state = pathlib.Path(sys.argv[1])
window = Gtk.Window(title='ControllerTextFixture')
window.set_default_size(600, 400)
view = Gtk.TextView()
buffer = view.get_buffer()
buffer.connect('changed', lambda buf: state.write_text(buf.get_text(buf.get_start_iter(), buf.get_end_iter(), True)))
window.add(view)
window.connect('destroy', Gtk.main_quit)
window.show_all()
view.grab_focus()
state.write_text('')
Gtk.main()
