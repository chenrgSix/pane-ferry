#!/usr/bin/env python3
"""Test-owned scrolling surface; only synthetic text and frame times are recorded."""
import json
from pathlib import Path
import sys
import time

import cairo
import gi
gi.require_version('Gtk', '3.0')
from gi.repository import Gtk

log = Path(sys.argv[1]).open('w', buffering=1)
width, height = map(int, sys.argv[2:4])
window = Gtk.Window(title='PaneFerry Scroll Benchmark')
window.set_decorated('--undecorated' not in sys.argv[4:])
window.set_default_size(*( (min(width, 1400), min(height, 700))
                          if '--small-start' in sys.argv[4:] else (width, height)))
area = Gtk.DrawingArea()
window.add(area)

# Paint the document once so text layout does not limit the redraw rate.
page = cairo.ImageSurface(cairo.FORMAT_RGB24, width, height + 256)
ink = cairo.Context(page)
ink.set_source_rgb(.94, .95, .96)
ink.paint()
ink.set_font_size(17)
for y in range(28, height + 256, 32):
    for x in range(12, width, 512):
        ink.set_source_rgb(.12, .18, .25)
        ink.move_to(x, y)
        ink.show_text(f'PaneFerry scrolling document   row {y // 32:04d}')
        ink.set_source_rgb(.25, .5, .7)
        ink.rectangle(x, y + 5, 420, 2)
        ink.fill()

sequence = 0


def draw(widget, context):
    global sequence
    sequence += 1
    context.set_source_surface(page, 0, -(sequence * 7 % 256))
    context.paint()
    # Binary counter lets a separate pixel probe recognize the actual redraw.
    for bit in range(8):
        value = (sequence >> bit) & 1
        context.set_source_rgb(value, value, value)
        context.rectangle(bit * 32, 0, 32, 32)
        context.fill()
    log.write(json.dumps(dict(sequence=sequence, ns=time.monotonic_ns(),
                             width=widget.get_allocated_width(),
                             height=widget.get_allocated_height())) + '\n')


def tick(widget, _clock):
    widget.queue_draw()
    return True


area.connect('draw', draw)
area.add_tick_callback(tick)
window.connect('destroy', Gtk.main_quit)
window.show_all()
Gtk.main()
