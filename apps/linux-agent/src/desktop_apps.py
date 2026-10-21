"""Host-side desktop discovery. Invoked with fixed argv by the Agent, never a shell."""
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import sys

# A user service may not inherit the graphical login's export directories.
paths = os.environ.get('XDG_DATA_DIRS', '/usr/local/share:/usr/share').split(':')
for path in ([] if 'XDG_DATA_DIRS' in os.environ else [
             '/var/lib/snapd/desktop', '/var/lib/flatpak/exports/share',
             str(Path.home() / '.local/share/flatpak/exports/share')]):
    if path not in paths and Path(path).is_dir():
        paths.append(path)
os.environ['XDG_DATA_DIRS'] = ':'.join(paths)

import gi
gi.require_version('Gio', '2.0')
gi.require_version('Gtk', '3.0')
from gi.repository import Gio, GLib, Gtk, GdkPixbuf
try:
    gi.require_version('GioUnix', '2.0')
    from gi.repository import GioUnix
    DesktopAppInfo = GioUnix.DesktopAppInfo
except (ValueError, ImportError):
    DesktopAppInfo = Gio.DesktopAppInfo


def icon_theme():
    theme = Gtk.IconTheme.new()
    user_data = Path(os.environ.get('XDG_DATA_HOME', str(Path.home() / '.local/share')))
    directories = [str(Path.home() / '.icons'), str(user_data / 'icons')]
    directories += [str(Path(p) / 'icons') for p in paths if p]
    directories.append('/usr/share/pixmaps')
    theme.set_search_path(directories)
    name = ''
    source = Gio.SettingsSchemaSource.get_default()
    if source and source.lookup('org.gnome.desktop.interface', True):
        name = Gio.Settings.new('org.gnome.desktop.interface').get_string('icon-theme')
    theme.set_custom_theme(name or 'hicolor')
    return theme


def icon_png(app, theme):
    try:
        icon = app.get_icon()
        if not icon:
            return None
        if isinstance(icon, Gio.FileIcon):
            filename = icon.get_file().get_path()
        else:
            info = theme.lookup_by_gicon(icon, 96, Gtk.IconLookupFlags.FORCE_SIZE)
            filename = info.get_filename() if info else None
        if not filename or Path(filename).stat().st_size > 4 * 1024 * 1024:
            return None
        # Rasterize on Ubuntu; remote SVG/XML never reaches the webview.
        pixels = GdkPixbuf.Pixbuf.new_from_file_at_scale(filename, 96, 96, True)
        for size in (96, 64, 48):
            image = pixels if size == 96 else pixels.scale_simple(size, size, GdkPixbuf.InterpType.BILINEAR)
            ok, data = image.save_to_bufferv('png', ['compression'], ['9'])
            if ok and len(data) <= 12 * 1024:
                return base64.b64encode(data).decode('ascii')
    except (GLib.Error, OSError, ValueError):
        pass
    return None


def command(app):
    line = app.get_string('Exec') or ''
    if not line:
        raise ValueError('desktop_activation_unavailable')
    _, words = GLib.shell_parse_argv(line)
    result = []
    for word in words:
        if word in ('%f', '%F', '%u', '%U', '%d', '%D', '%n', '%N', '%v', '%m'):
            continue
        if word == '%i':
            icon = app.get_string('Icon')
            if icon:
                result += ['--icon', icon]
            continue
        value, i = '', 0
        while i < len(word):
            if word[i] != '%':
                value += word[i]
                i += 1
                continue
            code = word[i:i+2]
            if code not in ('%%', '%c', '%k'):
                raise ValueError('desktop_command_unsupported')
            value += {'%%': '%', '%c': app.get_display_name(), '%k': app.get_filename()}[code]
            i += 2
        result.append(value)
    if not result:
        raise ValueError('desktop_command_unsupported')
    executable = shutil.which(result[0])
    if not executable:
        raise ValueError('desktop_executable_unavailable')
    result[0] = executable
    if app.get_boolean('Terminal'):
        terminal = shutil.which('xterm')
        if not terminal:
            raise ValueError('desktop_terminal_unavailable')
        result = [terminal, '-e', *result]
    if len(result) > 65 or any('\0' in value or len(value.encode()) > 4096 for value in result):
        raise ValueError('desktop_command_unsupported')
    return result


def entry(app, theme=None):
    desktop_id = app.get_id()
    filename = app.get_filename()
    if not desktop_id or not filename or not app.should_show() or app.get_is_hidden():
        return None
    data = dict(id='desktop-' + hashlib.sha256(desktop_id.encode()).hexdigest()[:32],
                desktop_id=desktop_id, name=''.join(c if not c.isascii() or c.isprintable() else ' ' for c in app.get_display_name())[:256],
                description=(app.get_description() or '')[:1024],
                icon_png=icon_png(app, theme) if theme else None,
                startup_class=(app.get_startup_wm_class() or '')[:256],
                executable='', argv=[], cwd=app.get_string('Path') or None,
                snap=None, unavailable_reason=None)
    try:
        if data['cwd'] and (not Path(data['cwd']).is_absolute() or not Path(data['cwd']).is_dir()):
            data['cwd'] = None
            raise ValueError('desktop_working_directory_unavailable')
        argv = command(app)
        data.update(executable=argv[0], argv=argv[1:])
        # Snap confinement needs a readable per-session cookie and an app scope.
        if '/snapd/desktop/applications/' in filename:
            snap = Path(filename).name.split('_', 1)[0]
            if snap and len(snap) <= 64 and all(c in 'abcdefghijklmnopqrstuvwxyz0123456789-' for c in snap):
                data['snap'] = snap
    except (GLib.Error, ValueError) as error:
        data['unavailable_reason'] = str(error) if isinstance(error, ValueError) else 'desktop_command_unsupported'
    return data


def main():
    mode = sys.argv[1]
    if mode == 'resolve':
        app = DesktopAppInfo.new(sys.argv[2])
        result = entry(app) if app else None
    else:
        theme = icon_theme()
        result = [value for app in Gio.AppInfo.get_all() if isinstance(app, DesktopAppInfo)
                  for value in [entry(app, theme)] if value]
        result.sort(key=lambda app: (app['name'].casefold(), app['desktop_id']))
        result = result[:512]
    print(json.dumps(result, ensure_ascii=False, separators=(',', ':')))


if __name__ == '__main__':
    main()
