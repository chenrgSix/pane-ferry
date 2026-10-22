// Observe completion, without changing the input engine's result or events.
// EIS/Clutter round trips do not wait for IBus's asynchronous key filters.
export class KeyCompletion {
    constructor(schedule, cancel) {
        this.schedule = schedule;
        this.cancel = cancel;
        this.pending = 0;
        this.waiters = new Set();
        this.closed = false;
    }

    wrap(original) {
        const tracker = this;
        return function (...args) {
            if (tracker.closed) return original.apply(this, args);
            tracker.pending++;
            let completed = false;
            const done = () => {
                if (completed) return;
                completed = true;
                if (--tracker.pending === 0)
                    for (const waiter of [...tracker.waiters]) waiter.finish();
            };
            const callback = args.at(-1);
            if (typeof callback === 'function') {
                args[args.length - 1] = function (...reply) {
                    try { return callback.apply(this, reply); }
                    finally { done(); }
                };
            }
            try {
                const result = original.apply(this, args);
                if (typeof callback === 'function') return result;
                if (result && typeof result.finally === 'function') return result.finally(done);
                done();
                return result;
            } catch (error) {
                done();
                throw error;
            }
        };
    }

    wait() {
        if (this.closed) return Promise.reject(new Error('wayland_text_unavailable'));
        if (this.pending === 0) return Promise.resolve();
        return new Promise((resolve, reject) => {
            const waiter = {finish: error => {
                if (!this.waiters.delete(waiter)) return;
                this.cancel(timer);
                if (error) reject(error);
                else resolve();
            }};
            const timer = this.schedule(() => waiter.finish(new Error('wayland_text_unavailable')), 1000);
            this.waiters.add(waiter);
        });
    }

    close() {
        this.closed = true;
        for (const waiter of [...this.waiters]) waiter.finish(new Error('wayland_text_unavailable'));
    }
}

// An application may queue wl_keyboard events while applying text-input
// commits immediately. Observe its state acknowledgement, never its contents.
export class InputAcknowledgement {
    constructor(schedule, cancel) {
        this.schedule = schedule;
        this.cancel = cancel;
        this.serial = 0;
        this.keySerial = null;
        this.unconfirmed = null;
        this.waiters = new Set();
        this.closed = false;
    }

    key() { this.keySerial = this.serial; }

    acknowledge() {
        this.serial++;
        for (const waiter of [...this.waiters])
            if (this.serial > waiter.serial) waiter.finish();
    }

    after(serial) {
        if (this.closed) return Promise.reject(new Error('wayland_text_unavailable'));
        if (this.serial > serial) return Promise.resolve();
        return new Promise((resolve, reject) => {
            const waiter = {serial, finish: error => {
                if (!this.waiters.delete(waiter)) return;
                this.cancel(timer);
                if (error) reject(error);
                else resolve();
            }};
            const timer = this.schedule(() => waiter.finish(new Error('wayland_text_unavailable')), 1000);
            this.waiters.add(waiter);
        });
    }

    async before() {
        if (this.closed) throw new Error('wayland_text_unavailable');
        if (this.unconfirmed !== null && this.serial <= this.unconfirmed)
            throw new Error('wayland_text_outcome_unknown');
        if (this.keySerial !== null) await this.after(this.keySerial);
        if (this.keySerial !== null && this.serial <= this.keySerial)
            throw new Error('wayland_text_unavailable');
        this.keySerial = null;
    }

    async confirm(serial) {
        try { await this.after(serial); }
        catch {
            // The commit may already have been applied. Never replay it or
            // allow later text to overtake an unconfirmed application state.
            this.unconfirmed = serial;
            throw new Error('wayland_text_outcome_unknown');
        }
    }

    close() {
        this.closed = true;
        for (const waiter of [...this.waiters]) waiter.finish(new Error('wayland_text_unavailable'));
    }
}
