#!/usr/bin/env python3
"""Disposable GTK CSD window; deliberately keep the process alive after close.

Run only inside a test-owned private X11 session. The state file contains this
fixture's PID and window state, never user data.
"""
import json
import os
from pathlib import Path
import sys

import gi

gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
gi.require_version('GdkX11', '3.0')
from gi.repository import Gdk, GdkX11, GLib, Gtk

state = Path(sys.argv[1])
title = sys.argv[2]
window = Gtk.Window(title=title)
window.set_default_size(900, 550)
header = Gtk.HeaderBar(title=title, show_close_button=True)
header.set_decoration_layout(':minimize,maximize,close')
window.set_titlebar(header)
label = Gtk.Label(label='PaneFerry lifecycle fixture\nOnly this disposable window is controlled.')
window.add(label)
events = []
destroyed = False
iconified = False
xid = None


def write_state():
    record = dict(pid=os.getpid(), title=title, xid=xid, destroyed=destroyed,
                  iconified=iconified, events=events[-20:],
                  marker='private-window-lifecycle-fixture')
    temporary = state.with_suffix('.new')
    temporary.write_text(json.dumps(record))
    temporary.replace(state)
    return True


def on_state(_window, event):
    global iconified
    iconified = bool(event.new_window_state & Gdk.WindowState.ICONIFIED)
    events.append('minimized' if iconified else 'visible')
    write_state()
    return False


def on_destroy(_window):
    global destroyed
    destroyed = True
    events.append('destroyed-process-still-running')
    write_state()
    # Gtk.main deliberately remains active: the Agent must observe destruction
    # of the last real window, rather than wait for this process to quit.


def inspect():
    global xid
    if destroyed:
        return True
    surface = window.get_window()
    if surface is not None:
        xid = GdkX11.X11Window.get_xid(surface)
    write_state()
    return True


window.connect('window-state-event', on_state)
window.connect('destroy', on_destroy)
window.show_all()
GLib.timeout_add(100, inspect)
GLib.timeout_add(100, write_state)
Gtk.main()
