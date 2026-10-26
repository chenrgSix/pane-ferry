#!/usr/bin/env python3
"""Test-owned native GTK window with a per-input visual counter, no user data."""
import json
from pathlib import Path
import sys
import time
import gi
gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gdk, GLib, Gtk

log = Path(sys.argv[1]).open('w', buffering=1)
width, height = map(int, sys.argv[2:4])
animated = sys.argv[4] == 'animated'
initial_scale = int(sys.argv[5]) if len(sys.argv) > 5 else 2
logical_content = '--logical-content' in sys.argv[6:]
output_markers = '--output-markers' in sys.argv[6:]
app = Gtk.Application(application_id='io.paneferry.LiveLatencyFixture')
count = tick = 0
last_drawn = -1
last_painted = -1


def record(kind, **extra):
    log.write(json.dumps(dict(kind=kind, count=count, ns=time.monotonic_ns(), **extra)) + '\n')


def activate(application):
    window = Gtk.ApplicationWindow(application=application, title='PaneFerry Live Latency Fixture')
    window.set_decorated(False)
    # Before mapping, GTK can still report scale 1 on a 2x GNOME display.
    # Use the known monitor scale to avoid an oversized initial capture.
    window.set_default_size(width // initial_scale, height // initial_scale)
    area = Gtk.DrawingArea()
    area.set_can_focus(True)
    area.add_events(Gdk.EventMask.BUTTON_PRESS_MASK | Gdk.EventMask.KEY_PRESS_MASK)
    window.add(area)

    def respond(kind):
        global count
        count += 1
        record(kind)
        area.queue_draw()

    def draw(widget, context):
        global last_drawn
        scale = window.get_scale_factor()
        context.save()
        units = 1 if logical_content else scale
        if not logical_content: context.scale(1 / scale, 1 / scale)
        for y in range(0, widget.get_allocated_height() * units, 64):
            for x in range(0, widget.get_allocated_width() * units, 128):
                context.set_source_rgb((x % 251) / 251, (y % 239) / 239, .7)
                context.rectangle(x, y, 128, 64); context.fill()
                context.set_source_rgb(.1, .1, .1)
                context.move_to(x + 8, y + 30); context.show_text(f'Latency {x}:{y}')
        context.restore()
        # Fixed physical marker coordinates keep probe recognition independent
        # of density; the page itself retains identical logical content.
        context.scale(1 / scale, 1 / scale)
        if output_markers:
            # Xwayland may keep a 2x backing buffer while the receiver gets 1x.
            # Keep the visual counter at fixed receiver coordinates in that case.
            context.scale(widget.get_allocated_width() * scale / width,
                          widget.get_allocated_height() * scale / height)
        for bit in range(8):
            value = (count >> bit) & 1
            context.set_source_rgb(value, value, value)
            context.rectangle(bit * 32, 0, 32, 64); context.fill()
        context.set_source_rgb(0, (tick % 100) / 100, 0)
        context.rectangle(300, 0, 64, 64); context.fill()
        signature = (count, widget.get_allocated_width(), widget.get_allocated_height(), scale)
        if signature != last_drawn:
            record('draw', width=widget.get_allocated_width() * scale,
                   height=widget.get_allocated_height() * scale, scale=scale,
                   logical_content=logical_content)
            last_drawn = signature

    def key(widget, event):
        if event.keyval == Gdk.KEY_Left:
            respond('key'); return True
        return False

    def click(widget, event):
        if event.type == Gdk.EventType.BUTTON_PRESS and event.button == 1:
            respond('pointer')
        return True

    def animate():
        global tick
        tick += 1
        area.queue_draw()
        return True

    def after_paint(clock):
        global last_painted
        if count != last_painted:
            record('after_paint', frame_counter=clock.get_frame_counter(),
                   frame_time_us=clock.get_frame_time())
            last_painted = count

    area.connect('draw', draw)
    area.connect('button-press-event', click)
    area.connect('key-press-event', key)
    window.show_all(); area.grab_focus()
    area.get_frame_clock().connect('after-paint', after_paint)
    if animated:
        GLib.timeout_add(33, animate)


app.connect('activate', activate)
app.run([])
