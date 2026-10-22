import GLib from 'gi://GLib';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';

function drained() {
    // Let queued Clutter input events run before acknowledging the commit. The
    // following key/click must not overtake text on the client's Wayland socket.
    return new Promise(resolve => GLib.idle_add(GLib.PRIORITY_DEFAULT_IDLE, () => {
        resolve(); return GLib.SOURCE_REMOVE;
    }));
}

function stageDrained() {
    // EIS acknowledges backend processing before the stage's queued keys are
    // delivered during its next update. An idle alone can run before that frame.
    return new Promise((resolve, reject) => {
        let handler = 0;
        const timer = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, () => {
            global.stage.disconnect(handler);
            reject(new Error('wayland_text_unavailable'));
            return GLib.SOURCE_REMOVE;
        });
        handler = global.stage.connect('after-update', () => {
            global.stage.disconnect(handler);
            GLib.source_remove(timer);
            resolve();
        });
        global.stage.schedule_update();
    });
}

export class TextInput {
    constructor() {
        this.pending = Promise.resolve();
        this.closed = false;
    }

    commit(window, text, valid) {
        const job = this.pending.then(() => this.deliver(window, text, valid));
        this.pending = job.catch(() => {});
        return job;
    }

    focused(window, valid) {
        if (this.closed || !valid() || global.display.focus_window !== window || global.stage.get_key_focus())
            throw new Error('window_unavailable');
    }

    async deliver(window, text, valid) {
        await stageDrained();
        await drained();
        this.focused(window, valid);
        if (Main.inputMethod.currentFocus) {
            // The controller already performed composition. Deliver the final
            // UTF-8 text through the compositor's text-input protocol, without
            // re-composing it through the source machine's IM or clipboard.
            Main.inputMethod.commit(text);
            await drained();
            return 'text-input';
        }
        // A virtual key only works for characters already in the current XKB
        // layout; notify_keyval silently drops unsupported Unicode. Do not
        // claim success or fall back to racing per-character clipboard writes.
        // The caller can explicitly paste a whole string instead.
        throw new Error('wayland_text_unavailable');
    }

    destroy() {
        this.closed = true;
    }
}
