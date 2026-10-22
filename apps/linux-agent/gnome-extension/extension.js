import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Meta from 'gi://Meta';
import Mtk from 'gi://Mtk';
import Shell from 'gi://Shell';
import St from 'gi://St';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

// No Eval, shell commands, desktop scaling or whole-monitor capture. Leases
// belong to the unique D-Bus sender, and die when that helper disconnects.
const XML = `<node><interface name="io.paneferry.Windows1"><method name="Call"><arg type="s" direction="in"/><arg type="s" direction="out"/></method></interface></node>`;
export default class PaneFerry extends Extension {
    enable() {
        this.leases = new Map();
        this.object = Gio.DBusExportedObject.wrapJSObject(XML, this);
        this.object.export(Gio.DBus.session, '/io/paneferry/Windows');
        this.name = Gio.bus_own_name_on_connection(Gio.DBus.session, 'io.paneferry.Windows', Gio.BusNameOwnerFlags.NONE, null, null);
        this.ownerSignal = Gio.DBus.session.signal_subscribe('org.freedesktop.DBus', 'org.freedesktop.DBus', 'NameOwnerChanged', '/org/freedesktop/DBus', null, Gio.DBusSignalFlags.NONE,
            (_c, _s, _p, _i, _n, params) => {
                const [name, , owner] = params.deep_unpack();
                if (!owner) for (const [token, lease] of this.leases) if (lease.owner === name) this.leases.delete(token);
            });
    }
    disable() {
        if (this.ownerSignal) Gio.DBus.session.signal_unsubscribe(this.ownerSignal);
        if (this.name) Gio.bus_unown_name(this.name);
        this.object?.unexport(); this.leases?.clear();
        this.object = null; this.ownerSignal = 0; this.name = 0;
    }
    windows() {
        return global.get_window_actors().map(actor => actor.meta_window)
            .filter(w => w && w.get_pid() > 0 && ![Meta.WindowType.DESKTOP, Meta.WindowType.DOCK].includes(w.get_window_type()));
    }
    describe(w) {
        const frame = w.get_frame_rect();
        const buffer = w.get_buffer_rect();
        const monitor = global.display.get_monitor_geometry(w.get_monitor());
        const parent = w.get_transient_for();
        const app = Shell.WindowTracker.get_default().get_window_app(w);
        return {id: String(w.get_id()), window_id: w.get_stable_sequence(), pid: w.get_pid(),
            app_id: app?.get_id() ?? '', name: app?.get_name() ?? w.get_wm_class() ?? '',
            title: Array.from(w.get_title() ?? '').slice(0, 256).join(''), native: w.get_client_type() === Meta.WindowClientType.WAYLAND,
            xwindow: w.get_client_type() === Meta.WindowClientType.X11 ? w.xwindow : null,
            x: frame.x, y: frame.y, width: frame.width, height: frame.height,
            buffer_x: buffer.x, buffer_y: buffer.y, buffer_width: buffer.width, buffer_height: buffer.height,
            max_width: monitor.width - (buffer.width - frame.width), max_height: monitor.height - (buffer.height - frame.height),
            parent_id: parent?.get_stable_sequence() ?? null, minimized: w.minimized,
            scale: w.get_compositor_private()?.get_resource_scale() ?? 1};
    }
    scope(root) {
        return this.windows().filter(w => {
            const seen = new Set();
            for (let at = w; at && !seen.has(at); at = at.get_transient_for()) {
                if (at === root) return true;
                seen.add(at);
            }
            return false;
        });
    }
    async CallAsync([encoded], invocation) {
        let result;
        try {
            if (encoded.length > 65536) throw new Error('invalid_request');
            const r = JSON.parse(encoded);
            const owner = invocation.get_sender();
            if (Main.sessionMode.isLocked || (Main.sessionMode.currentMode !== 'user' && Main.sessionMode.parentMode !== 'user')) throw new Error('desktop_locked');
            if (r.op === 'list') result = this.windows().slice(0, 128).map(w => this.describe(w));
            else if (r.op === 'acquire') {
                const root = this.windows().find(w => String(w.get_id()) === r.id && w.get_pid() === r.pid);
                if (!root) throw new Error('window_unavailable');
                const selected = this.scope(root);
                if ([...this.leases.values()].some(l => this.scope(l.root).some(w => selected.includes(w)))) throw new Error('session_controlled');
                if (this.leases.size >= 16) throw new Error('session_limit');
                const token = GLib.uuid_string_random();
                this.leases.set(token, {owner, root}); result = {token, window: this.describe(root)};
            } else {
                const lease = this.leases.get(r.token);
                if (!lease || lease.owner !== owner) throw new Error('window_unavailable');
                if (r.op === 'release') { this.leases.delete(r.token); result = {}; }
                else {
                    if (!this.windows().includes(lease.root)) throw new Error('window_unavailable');
                    const scope = this.scope(lease.root);
                    const w = r.xwindow ? scope.find(window => window.get_client_type() === Meta.WindowClientType.X11 && window.xwindow === r.xwindow)
                        : scope.find(window => window.get_stable_sequence() === r.window_id);
                    if (r.op === 'scope') result = scope.slice(0, 16).map(window => this.describe(window));
                    else {
                        if (!w) throw new Error('window_unavailable');
                        if (r.op === 'resize' || r.op === 'resize_x11') {
                            if (!Number.isInteger(r.width) || !Number.isInteger(r.height) || r.width < 1 || r.height < 1 || r.width > 4096 || r.height > 4096 || r.width * r.height > 8000000) throw new Error('invalid_dimensions');
                            const rect = w.get_frame_rect();
                            const limits = this.describe(w);
                            if (w.get_client_type() === Meta.WindowClientType.WAYLAND && (r.width > limits.max_width || r.height > limits.max_height)) throw new Error('resolution_limited');
                            if (w.is_fullscreen()) w.unmake_fullscreen();
                            w.unmaximize();
                            // This is an explicit controller resize. Mutter otherwise
                            // clamps programmatic requests to the physical work area.
                            let width = r.width, height = r.height;
                            if (r.op === 'resize_x11') {
                                const client = w.get_client_content_rect();
                                const wanted = w.protocol_to_stage_rect(new Mtk.Rectangle({x: 0, y: 0, width, height}));
                                width = wanted.width + rect.width - client.width;
                                height = wanted.height + rect.height - client.height;
                            }
                            w.move_resize_frame(true, rect.x, rect.y, width, height);
                            result = this.describe(w);
                        } else if (r.op === 'locate_x11') {
                            if (![r.x, r.y, r.width, r.height].every(Number.isInteger) || r.x < 0 || r.y < 0 || r.x >= r.width || r.y >= r.height) throw new Error('invalid_dimensions');
                            const frame = w.get_frame_rect();
                            let client = w.get_client_content_rect();
                            const pixels = w.stage_to_protocol_rect(client);
                            if (pixels.width !== r.width || pixels.height !== r.height) throw new Error('window_unavailable');
                            const area = w.get_work_area_current_monitor();
                            let x = client.x + r.x * client.width / r.width;
                            let y = client.y + r.y * client.height / r.height;
                            const dx = Math.max(area.x + 8, Math.min(x, area.x + area.width - 9)) - x;
                            const dy = Math.max(area.y + 8, Math.min(y, area.y + area.height - 9)) - y;
                            if (dx || dy) {
                                // Pan only the selected oversized window, so EIS
                                // can reach its off-screen controls without clipping.
                                w.move_frame(true, Math.round(frame.x + dx), Math.round(frame.y + dy));
                                client = w.get_client_content_rect();
                                x = client.x + r.x * client.width / r.width;
                                y = client.y + r.y * client.height / r.height;
                            }
                            if (x < area.x || y < area.y || x >= area.x + area.width || y >= area.y + area.height) throw new Error('window_unavailable');
                            result = {x: Math.round(x), y: Math.round(y)};
                        } else if (r.op === 'focus') { Main.activateWindow(w); result = {}; }
                        else if (r.op === 'close') { w.delete(global.get_current_time()); result = {}; }
                        else if (r.op === 'set_text') {
                            if (typeof r.text !== 'string' || new TextEncoder().encode(r.text).length > 8192 || r.text.includes('\0')) throw new Error('invalid_text');
                            St.Clipboard.get_default().set_text(St.ClipboardType.CLIPBOARD, r.text); result = {};
                        } else if (r.op === 'get_text') {
                            result = await new Promise((resolve, reject) => {
                                let timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1500, () => {
                                    timer = 0; reject(new Error('clipboard_copy_failed')); return GLib.SOURCE_REMOVE;
                                });
                                St.Clipboard.get_default().get_text(St.ClipboardType.CLIPBOARD, (_c, text) => {
                                    if (!timer) return;
                                    GLib.source_remove(timer); timer = 0;
                                    if (new TextEncoder().encode(text ?? '').length > 8192 || text?.includes('\0')) reject(new Error('invalid_text'));
                                    else resolve({text: text ?? ''});
                                });
                            });
                        } else throw new Error('invalid_request');
                    }
                }
            }
            invocation.return_value(new GLib.Variant('(s)', [JSON.stringify({result})]));
        } catch (error) {
            const allowed = ['desktop_locked', 'window_unavailable', 'session_controlled', 'session_limit', 'invalid_request', 'invalid_dimensions', 'invalid_text', 'clipboard_copy_failed', 'resolution_limited'];
            if (!allowed.includes(error.message)) console.error(`PaneFerry compositor API: ${error.message}
${error.stack}`);
            invocation.return_value(new GLib.Variant('(s)', [JSON.stringify({error: allowed.includes(error.message) ? error.message : 'wayland_bridge_failed'})]));
        }
    }
}
