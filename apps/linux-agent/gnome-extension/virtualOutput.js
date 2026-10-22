import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

const BUS = 'org.gnome.Mutter.ScreenCast';
const PATH = '/org/gnome/Mutter/ScreenCast';
const SESSION = `${BUS}.Session`;
let provisioning = Promise.resolve();

function call(path, iface, method, parameters = null) {
    return new Promise((resolve, reject) => {
        Gio.DBus.session.call(BUS, path, iface, method, parameters, null,
            Gio.DBusCallFlags.NO_AUTO_START, 3000, null, (connection, result) => {
                try { resolve(connection.call_finish(result).deep_unpack()); }
                catch (error) { reject(error); }
            });
    });
}

export function delay(milliseconds) {
    return new Promise(resolve => GLib.timeout_add(GLib.PRIORITY_DEFAULT, milliseconds, () => {
        resolve(); return GLib.SOURCE_REMOVE;
    }));
}

// A private output changes the selected client's Wayland output scale, so the
// client redraws its own buffers. No screenshot resampling or application flags.
// RecordWindow still captures only the leased window, never this whole output.
export class VirtualOutput {
    constructor(root) {
        this.root = root;
        this.original = root.get_frame_rect();
        this.maximized = root.get_maximize_flags();
        this.fullscreen = root.is_fullscreen();
        this.manager = global.backend.get_monitor_manager();
        this.originalConnector = this.manager.get_monitors().find(m =>
            this.manager.get_monitor_for_connector(m.get_connector()) === root.get_monitor())?.get_connector();
        this.path = null;
        this.connector = null;
        this.cancelled = false;
    }

    open(scale = 2) {
        // Serialize output discovery: two controllers must not claim each
        // other's connector while their asynchronous D-Bus calls overlap.
        this.opening = provisioning.then(() => this.create(scale));
        provisioning = this.opening.catch(() => {});
        return this.opening;
    }

    async create(scale) {
        if (![1, 2].includes(scale)) throw new Error('invalid_dimensions');
        if (this.cancelled) throw new Error('window_unavailable');
        const before = new Set(this.manager.get_monitors().map(m => m.get_connector()));
        try {
            [this.path] = await call(PATH, BUS, 'CreateSession', new GLib.Variant('(a{sv})', [{}]));
            if (this.cancelled) throw new Error('window_unavailable');
            // Fixed envelope avoids output hotplug during every live resize.
            // It includes headroom for shadows around the 4096x2160 viewport.
            const mode = {size: new GLib.Variant('(uu)', [4352, 2432]),
                'refresh-rate': new GLib.Variant('d', 60),
                'is-preferred': new GLib.Variant('b', true),
                'preferred-scale': new GLib.Variant('d', scale)};
            await call(this.path, SESSION, 'RecordVirtual', new GLib.Variant('(a{sv})', [{
                'cursor-mode': new GLib.Variant('u', 0),
                'modes': new GLib.Variant('aa{sv}', [mode]),
            }]));
            await call(this.path, SESSION, 'Start');
            for (let attempt = 0; attempt < 40; attempt++) {
                if (this.cancelled) throw new Error('window_unavailable');
                const added = this.manager.get_monitors().filter(m => !before.has(m.get_connector()));
                if (added.length === 1) {
                    const connector = added[0].get_connector();
                    const index = this.manager.get_monitor_for_connector(connector);
                    if (index >= 0) {
                        this.connector = connector;
                        if (this.root.is_fullscreen()) this.root.unmake_fullscreen();
                        this.root.unmaximize();
                        this.root.move_to_monitor(index);
                        const area = this.root.get_work_area_for_monitor(index);
                        this.root.move_resize_frame(true, area.x + 48, area.y + 48,
                            Math.min(this.original.width, area.width - 96),
                            Math.min(this.original.height, area.height - 96));
                        for (let repaint = 0; repaint < 40; repaint++) {
                            if (this.cancelled) throw new Error('window_unavailable');
                            if (this.root.get_compositor_private()?.get_resource_scale() === scale) {
                                this.scale = scale;
                                return;
                            }
                            await delay(25);
                        }
                        throw new Error('wayland_capture_unavailable');
                    }
                }
                await delay(25);
            }
            throw new Error('wayland_capture_unavailable');
        } catch (error) {
            await this.destroy();
            throw error;
        }
    }

    async close() {
        this.cancelled = true;
        try { await this.opening; } catch { /* A failed acquisition already unwound. */ }
        if (!this.closing) {
            this.closing = provisioning.then(() => this.destroy());
            provisioning = this.closing.catch(() => {});
        }
        await this.closing;
    }

    contains(window) {
        return this.connector && window.get_monitor() === this.manager.get_monitor_for_connector(this.connector);
    }

    rehome(window) {
        const index = this.originalConnector ? this.manager.get_monitor_for_connector(this.originalConnector) : -1;
        if (index >= 0) window.move_to_monitor(index);
    }

    async destroy() {
        try {
            const index = this.originalConnector ? this.manager.get_monitor_for_connector(this.originalConnector) : -1;
            if (index >= 0 && this.connector) {
                this.root.move_to_monitor(index);
                const r = this.original;
                this.root.move_resize_frame(true, r.x, r.y, r.width, r.height);
                if (this.maximized) this.root.maximize();
                if (this.fullscreen) this.root.make_fullscreen();
            }
        } catch { /* A closed original window must not keep its output alive. */ }
        const connector = this.connector;
        this.connector = null;
        if (this.path) {
            const path = this.path;
            this.path = null;
            try { await call(path, SESSION, 'Stop'); } catch { /* Already stopped. */ }
            // Stop replies before monitor removal is necessarily reflected in
            // the layout. Wait before another output reuses the same connector,
            // otherwise discovery mistakes it for a pre-existing monitor.
            for (let attempt = 0; connector && attempt < 80; attempt++) {
                if (this.manager.get_monitor_for_connector(connector) < 0) break;
                await delay(25);
            }
        }
    }
}
