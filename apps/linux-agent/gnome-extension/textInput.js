import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import IBus from 'gi://IBus';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import {InjectionManager} from 'resource:///org/gnome/shell/extensions/extension.js';
import {KeyCompletion, InputAcknowledgement} from './keyCompletion.js';

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
        const schedule = (callback, milliseconds) =>
            GLib.timeout_add(GLib.PRIORITY_DEFAULT, milliseconds, () => {
                callback(); return GLib.SOURCE_REMOVE;
            });
        const cancel = timer => GLib.source_remove(timer);
        this.keys = new KeyCompletion(schedule, cancel);
        this.acknowledgements = new InputAcknowledgement(schedule, cancel);
        this.inputMethod = Main.inputMethod;
        this.stateSignal = this.inputMethod.connect('surrounding-text-set', () =>
            this.acknowledgements.acknowledge());
        this.injections = new InjectionManager();
        this.injections.overrideMethod(IBus.InputContext.prototype, 'process_key_event_async',
            original => this.keys.wrap(original));
        const tracker = this;
        this.injections.overrideMethod(Object.getPrototypeOf(this.inputMethod), 'vfunc_filter_key_event',
            original => function (event) {
                tracker.observeKey(event);
                return original.call(this, event);
            });
        this.injections.overrideMethod(this.inputMethod, 'notify_key_event',
            original => function (event, filtered) {
                if (!filtered) tracker.observeKey(event);
                return original.call(this, event, filtered);
            });
    }

    observeKey(event) {
        if (event.type() !== Clutter.EventType.KEY_PRESS) return;
        const key = event.get_key_symbol();
        const modifier = key >= 0xffe1 && key <= 0xffee ||
            key >= 0xfe01 && key <= 0xfe13 || key === 0xff7e || key === 0xff7f;
        if (!modifier) this.acknowledgements.key();
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
        // The stage can finish while IBus still filters Ctrl+A/Ctrl+L. Wait
        // until their original callbacks have forwarded the keys, then drain
        // the reinjected events before enqueuing the independent text commit.
        await this.keys.wait();
        await stageDrained();
        await drained();
        this.focused(window, valid);
        if (this.keys.pending) throw new Error('wayland_text_unavailable');
        if (Main.inputMethod.currentFocus) {
            await this.acknowledgements.before();
            this.focused(window, valid);
            if (!Main.inputMethod.currentFocus) throw new Error('wayland_text_unavailable');
            // The controller already performed composition. Deliver the final
            // UTF-8 text through the compositor's text-input protocol, without
            // re-composing it through the source machine's IM or clipboard.
            const serial = this.acknowledgements.serial;
            Main.inputMethod.commit(text);
            await this.acknowledgements.confirm(serial);
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
        this.keys.close();
        this.acknowledgements.close();
        this.inputMethod.disconnect(this.stateSignal);
        this.injections.clear();
    }
}
