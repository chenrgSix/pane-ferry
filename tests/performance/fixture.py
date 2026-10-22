#!/usr/bin/env python3
"""Test-owned GTK editor: a binary visual counter identifies the response to each input."""
import json
import os
from pathlib import Path
import sys
import time

import gi
gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk, GLib, Gtk

log = Path(sys.argv[1]).open('a', buffering=1)
width, height = map(int, sys.argv[2:4])
animated = sys.argv[4] == 'animated'
window = Gtk.Window(title='PaneFerry End-to-End Latency Test')
scale = int(os.environ.get('GDK_SCALE', '1'))
window.set_default_size(width // scale, height // scale)
box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
area, entry = Gtk.DrawingArea(), Gtk.Entry()
area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK)
box.pack_start(area, True, True, 0)
box.pack_start(entry, False, False, 0)
window.add(box)
count = tick = 0
last_drawn = -1


def record(kind):
    log.write(json.dumps(dict(kind=kind, count=count, ns=time.monotonic_ns())) + '\n')


def respond(kind):
    global count
    count += 1
    record(kind)
    area.queue_draw()


def draw(widget, context):
    global last_drawn
    context.scale(1 / scale, 1 / scale)
    for y in range(0, widget.get_allocated_height() * scale, 64):
        for x in range(0, widget.get_allocated_width() * scale, 128):
            context.set_source_rgb((x % 251) / 251, (y % 239) / 239, .7)
            context.rectangle(x, y, 128, 64)
            context.fill()
            context.set_source_rgb(.1, .1, .1)
            context.move_to(x + 8, y + 30)
            context.show_text(f'Latency {x}:{y}')
    # Wide neutral blocks survive 4:2:0 and lossy H.264. Top 8 bits are the input ID.
    for bit in range(8):
        value = (count >> bit) & 1
        context.set_source_rgb(value, value, value)
        context.rectangle(bit * 32, 0, 32, 64)
        context.fill()
    context.set_source_rgb(0, (tick % 100) / 100, 0)
    context.rectangle(300, 0, 64, 64)
    context.fill()
    if count != last_drawn:
        record('draw')
        last_drawn = count


def key(_widget, event):
    if event.keyval == Gdk.KEY_Left:
        respond('key')
        return True
    return False


def click(_widget, event):
    if event.type != Gdk.EventType.BUTTON_PRESS or event.button != 1:
        return True
    respond('pointer')
    # Preserve the text entry as focus owner for later committed text.
    entry.grab_focus()
    return True


def animate():
    global tick
    tick += 1
    area.queue_draw()
    return True


area.connect('draw', draw)
area.connect('button-press-event', click)
entry.connect('key-press-event', key)
entry.connect('changed', lambda *_: respond('text'))
window.connect('destroy', Gtk.main_quit)
window.show_all()
entry.grab_focus()
if animated:
    GLib.timeout_add(33, animate)
Gtk.main()
