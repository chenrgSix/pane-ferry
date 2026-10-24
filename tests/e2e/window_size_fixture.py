#!/usr/bin/env python3
"""Private GTK fixture for tall minimums and recovery after impossible hints."""
import json
import os
from pathlib import Path
import sys

# Fixture dimensions are explicitly specified in pixels, independent of text DPI.
os.environ['GDK_SCALE'] = '1'
os.environ['GDK_DPI_SCALE'] = '1'
import gi
gi.require_version('Gtk', '3.0')
gi.require_version('GdkX11', '3.0')
from gi.repository import Gtk, Gdk, GdkX11, GLib

state, command = map(Path, sys.argv[1:3])
minimum = int(sys.argv[3])
window = Gtk.Window(title='PaneFerry Window Constraints Fixture')
window.set_default_size(3703, minimum)
area = Gtk.DrawingArea()
area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
window.add(area)
clicks = 0
last_click = None

def hints(height):
    geometry = Gdk.Geometry()
    geometry.min_width = 1920 if height == 2400 else 3000
    geometry.min_height = height
    window.set_geometry_hints(None, geometry, Gdk.WindowHints.MIN_SIZE)

def save(*_args):
    if window.get_window():
        value = dict(pid=os.getpid(), xid=window.get_window().get_xid(), width=window.get_window().get_width(),
                     height=window.get_window().get_height(), clicks=clicks, last_click=last_click)
        temporary = state.with_suffix('.tmp')
        temporary.write_text(json.dumps(value)); temporary.replace(state)
    return False

def draw(widget, context):
    context.set_source_rgb(.1, .3, .6); context.paint()
    context.set_source_rgb(1, 0, 1)
    context.rectangle(0, widget.get_allocated_height()-60, widget.get_allocated_width(), 60); context.fill()
    save()

def click(_widget, event):
    global clicks, last_click
    clicks += 1; last_click = [event.x, event.y]
    save(); return True

def poll():
    if command.exists():
        hints(json.loads(command.read_text())['minimum_height'])
        command.unlink()
    return True

area.connect('draw', draw)
area.connect('button-press-event', click)
window.connect('configure-event', save)
window.connect('destroy', Gtk.main_quit)
hints(minimum)
window.show_all()
GLib.timeout_add(100, poll)
Gtk.main()
