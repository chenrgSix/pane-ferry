#!/usr/bin/env python3
"""Animated, test-owned GTK surface with a visible and timestamped key response."""
import json
from pathlib import Path
import sys
import time
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk, GLib

state = Path(sys.argv[1])
window = Gtk.Window(title='RemoteAppLatencyFixture')
window.set_default_size(2560, 1664)
area = Gtk.DrawingArea()
area.set_can_focus(True)
window.add(area)
count = 0
tick = 0
last_key = None

def draw(widget, context):
    width, height = widget.get_allocated_width(), widget.get_allocated_height()
    for y in range(0, height, 64):
        for x in range(0, width, 128):
            context.set_source_rgb((x % 251) / 251, (y % 239) / 239, .7)
            context.rectangle(x, y, 128, 64)
            context.fill()
            context.set_source_rgb(.1, .1, .1)
            context.move_to(x + 8, y + 30)
            context.show_text(f'Latency {x}:{y}')
    context.set_source_rgb(1 if count % 2 else 0, 0, 0)
    context.rectangle(0, 0, 64, 64)
    context.fill()
    context.set_source_rgb(0, (tick % 100) / 100, 0)
    context.rectangle(80, 0, 64, 64)
    context.fill()

def key(_widget, event):
    global count, last_key
    if event.keyval == last_key:
        return True
    last_key = event.keyval
    count += 1
    state.write_text(json.dumps(dict(count=count, received=time.monotonic(), key=last_key)))
    area.queue_draw()
    return True

def animate():
    global tick
    tick += 1
    area.queue_draw()
    return True

area.connect('draw', draw)
area.connect('key-press-event', key)
window.connect('destroy', Gtk.main_quit)
window.show_all()
area.grab_focus()
if '--static' not in sys.argv[2:]:
    GLib.timeout_add(33, animate)
Gtk.main()
