"""Native GTK window family, controlled only through the test's private file."""
import json
import gi
gi.require_version('Gtk', '3.0')
gi.require_version('Gdk', '3.0')
from gi.repository import Gtk, Gdk, GLib


def install(root, state):
    children = []
    command = state.with_suffix('.command')
    last_serial = -1

    def close():
        for window in reversed(children): window.destroy()
        children.clear()

    def popup(parent, hint, title, size, color):
        window = Gtk.Window(type=Gtk.WindowType.POPUP, title=title)
        window.set_transient_for(parent)
        window.set_type_hint(hint)
        window.set_default_size(*size)
        area = Gtk.DrawingArea()
        def draw(widget, cr):
            cr.set_source_rgb(*color); cr.paint()
        area.connect('draw', draw)
        window.add(area)
        x, y = parent.get_position()
        window.move(x + 80, y + 100)
        window.show_all(); children.append(window)
        return window

    def tick():
        nonlocal last_serial
        if not command.exists(): return True
        request = json.loads(command.read_text())
        if request['serial'] == last_serial: return True
        last_serial = request['serial']
        action = request['action']
        if action in ('close', 'open', 'pulse'): close()
        if action == 'open':
            dialog = Gtk.Window(title='Generic dialog')
            dialog.set_transient_for(root); dialog.set_type_hint(Gdk.WindowTypeHint.DIALOG)
            dialog.set_default_size(300, 180); dialog.add(Gtk.Label(label='Independent dialog'))
            dialog.show_all(); children.append(dialog)
            menu = popup(root, Gdk.WindowTypeHint.POPUP_MENU, 'Generic menu', (240, 100), (0, .8, .1))
            def nested():
                if menu in children:
                    popup(menu, Gdk.WindowTypeHint.POPUP_MENU, 'Generic nested popup', (120, 40), (.1, .2, .9))
                return False
            # xdg_popup's parent must have completed its initial configure.
            GLib.timeout_add(150, nested)
        if action == 'pulse':
            popup(root, Gdk.WindowTypeHint.POPUP_MENU, 'Short lived popup', (180, 60), (.9, .1, .2))
            GLib.timeout_add(15, lambda: (close(), False)[1])
        return True
    GLib.timeout_add(40, tick)

