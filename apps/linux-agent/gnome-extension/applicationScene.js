import Clutter from 'gi://Clutter';
import Cogl from 'gi://Cogl';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

const BUS = 'org.gnome.Mutter.ScreenCast';
function call(path, iface, method, params = null) {
    return new Promise((resolve, reject) => Gio.DBus.session.call(BUS, path, iface, method,
        params, null, Gio.DBusCallFlags.NO_AUTO_START, 3000, null, (connection, result) => {
            try { resolve(connection.call_finish(result).deep_unpack()); }
            catch (error) { reject(error); }
        }));
}

// This is a compositor scene of explicitly leased window actors, not a desktop
// screenshot. The opaque cover is installed BEFORE creating a monitor stream,
// and remains until that stream AND its virtual output have been stopped.
// Clones retain native popup placement/stacking and share compositor textures.
export class ApplicationScene {
    constructor(output, scope) {
        this.output = output;
        this.connector = output.connector;
        this.scope = scope;
        this.clones = new Map();
        this.cover = new Clutter.Actor({reactive: false, clip_to_allocation: true,
            background_color: new Cogl.Color({red: 0, green: 0, blue: 0, alpha: 255})});
        global.stage.add_child(this.cover);
        this.raise = () => {
            if (global.stage.get_last_child() !== this.cover)
                global.stage.set_child_above_sibling(this.cover, null);
        };
        // Synchronous signal handling prevents a newly added Shell actor from
        // appearing above the cover for even one captured frame.
        this.childSignal = global.stage.connect('child-added', this.raise);
        this.paintSignal = global.stage.connect('before-update', () => this.update());
        this.update();
    }
    geometry() {
        const index = this.output.manager.get_monitor_for_connector(this.output.connector);
        if (index < 0) throw new Error('window_unavailable');
        return global.display.get_monitor_geometry(index);
    }
    update() {
        this.raise();
        try {
            const area = this.geometry();
            this.cover.set_position(area.x, area.y);
            this.cover.set_size(area.width, area.height);
            const allowed = this.retiring || Main.sessionMode.isLocked ? [] :
                global.display.sort_windows_by_stacking(this.scope()).filter(w => !w.minimized);
            if (allowed.length > 16) throw new Error('window_unavailable');
            const live = new Set(allowed);
            for (const [window, clone] of this.clones) {
                if (!live.has(window)) { clone.destroy(); this.clones.delete(window); }
            }
            for (const window of allowed) {
                const source = window.get_compositor_private();
                if (!source) continue;
                const rect = window.get_buffer_rect();
                if (rect.width <= 0 || rect.height <= 0 || source.width <= 0 || source.height <= 0) {
                    this.clones.get(window)?.hide();
                    continue;
                }
                let clone = this.clones.get(window);
                if (!clone) {
                    clone = new Clutter.Clone({source, reactive: false});
                    this.cover.add_child(clone);
                    this.clones.set(window, clone);
                }
                clone.set_position(rect.x - area.x, rect.y - area.y);
                // Leave natural size to the source. Forcing a size while a
                // popup unmaps (zero source allocation) produces NaN scales.
                clone.visible = source.visible;
            }
            const desired = allowed.map(w => this.clones.get(w)).filter(Boolean);
            const current = this.cover.get_children();
            if (current.length !== desired.length || current.some((actor, i) => actor !== desired[i]))
                for (const clone of desired) this.cover.set_child_above_sibling(clone, null);
            this.failed = false;
        } catch {
            this.failed = true;
            // A disappearing output/root must fail black, never uncover the
            // desktop while capture teardown is in flight.
            for (const clone of this.clones.values()) clone.hide();
        }
    }
    async start() {
        this.update();
        [this.session] = await call('/org/gnome/Mutter/ScreenCast', BUS, 'CreateSession',
            new GLib.Variant('(a{sv})', [{}]));
        const [stream] = await call(this.session, `${BUS}.Session`, 'RecordMonitor',
            new GLib.Variant('(sa{sv})', [this.output.connector, {'cursor-mode': new GLib.Variant('u', 0)}]));
        this.signal = Gio.DBus.session.signal_subscribe(BUS, `${BUS}.Stream`, 'PipeWireStreamAdded', stream,
            null, Gio.DBusSignalFlags.NONE, (_c, _s, _p, _i, _n, params) => { [this.node] = params.deep_unpack(); });
        await call(this.session, `${BUS}.Session`, 'Start');
    }
    info() {
        this.update();
        if (this.failed) throw new Error('window_unavailable');
        const area = this.geometry();
        return {x: area.x, y: area.y, width: area.width, height: area.height,
            scale: this.output.scale, node: this.node ?? null};
    }
    focus() {
        Main.overview.hide();
        const scope = this.scope();
        if (!scope.includes(global.display.focus_window)) Main.activateWindow(this.output.requireRoot());
        return global.display.focus_window;
    }
    point(x, y, width, height) {
        const area = this.geometry();
        if (![x, y, width, height].every(Number.isInteger) ||
            width !== Math.round(area.width * this.output.scale) || height !== Math.round(area.height * this.output.scale) ||
            x < 0 || y < 0 || x >= width || y >= height) throw new Error('window_unavailable');
        x = area.x + x / this.output.scale;
        y = area.y + y / this.output.scale;
        const allowed = this.scope();
        // Conservatively reject an overlapping foreign actor even when Mutter's
        // current popup grab/pick cache still resolves to an allowed window.
        // The output owner rehomes these arrivals; no hidden click is necessary.
        for (const candidate of global.get_window_actors()) {
            const window = candidate.meta_window;
            if (!window || allowed.includes(window) || window.minimized || !candidate.visible) continue;
            const rect = window.get_buffer_rect();
            if (x >= rect.x && y >= rect.y && x < rect.x + rect.width && y < rect.y + rect.height)
                throw new Error('window_unavailable');
        }
        // Don't send hidden input to unrelated windows/Shell UI underneath the
        // visual cover. All scene actors are nonreactive and excluded by pick.
        // Clutter.Clone can redirect a pick to its source actor even when the
        // clone itself is nonreactive. Exclude the cover for this synchronous
        // pick only. No await/main-loop iteration occurs before restoring it,
        // so a compositor paint can never observe the uncovered state.
        let actor;
        this.cover.hide();
        try { actor = global.stage.get_actor_at_pos(Clutter.PickMode.REACTIVE, x, y); }
        finally { this.cover.show(); }
        while (actor && !actor.meta_window) actor = actor.get_parent();
        if (!actor || !this.scope().includes(actor.meta_window)) throw new Error('window_unavailable');
        return {x: Math.round(x), y: Math.round(y)};
    }
    close() {
        if (this.closing) return this.closing;
        this.retiring = true;
        this.update();
        this.closing = (async () => {
            // start() may still be awaiting a D-Bus round trip on owner loss.
            try { await this.starting; } catch { /* Still stop the partial session. */ }
            if (this.session) {
                try { await call(this.session, `${BUS}.Session`, 'Stop'); } catch { /* Output teardown below ends capture too. */ }
            }
            if (this.signal) Gio.DBus.session.signal_unsubscribe(this.signal);
            await this.output.close();
            const dispose = () => {
                // Stop may time out without removing the virtual monitor. Keep
                // the opaque cover until compositor disappearance is confirmed.
                if (this.output.manager.get_monitor_for_connector(this.connector) >= 0)
                    return GLib.SOURCE_CONTINUE;
                global.stage.disconnect(this.paintSignal);
                global.stage.disconnect(this.childSignal);
                this.cover.destroy();
                this.clones.clear();
                return GLib.SOURCE_REMOVE;
            };
            if (dispose() === GLib.SOURCE_CONTINUE)
                GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, dispose);
        })();
        return this.closing;
    }
}
