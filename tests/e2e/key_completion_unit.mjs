// Run without GNOME. The live compositor test additionally delays real IBus
// callbacks, so a stage-only barrier deterministically fails before this fix.
import assert from 'node:assert/strict';
import {KeyCompletion, InputAcknowledgement} from '../../apps/linux-agent/gnome-extension/keyCompletion.js';

function tracker() {
    const timers = new Set();
    return {
        keys: new KeyCompletion(callback => { timers.add(callback); return callback; }, timer => timers.delete(timer)),
        expire: () => { for (const callback of [...timers]) callback(); },
        timers,
    };
}

{
    const {keys, timers} = tracker();
    const callbacks = [];
    const context = {};
    const calls = [];
    const method = keys.wrap(function (value, callback) {
        assert.equal(this, context);
        callbacks.push(callback);
        calls.push(value);
        return 42;
    });
    let forwarded = 0;
    assert.equal(method.call(context, 'first', () => forwarded++), 42);
    assert.equal(method.call(context, 'second', () => forwarded++), 42);
    const waiting = keys.wait().then(() => assert.equal(forwarded, 2));
    callbacks[1]();
    assert.equal(keys.pending, 1);
    callbacks[0]();
    await waiting;
    assert.deepEqual(calls, ['first', 'second']);
    assert.equal(timers.size, 0);
}
{
    const {keys, expire, timers} = tracker();
    let callback;
    let submitted = 0;
    keys.wrap(done => { callback = done; submitted++; })(() => {});
    const waiting = assert.rejects(keys.wait(), /wayland_text_unavailable/);
    expire();
    await waiting;
    assert.equal(submitted, 1, 'a timed-out key must never be replayed');
    assert.equal(timers.size, 0);
    callback();
    await keys.wait();
}
{
    const {keys} = tracker();
    const problem = new Error('filter failure');
    assert.throws(() => keys.wrap(() => { throw problem; })(() => {}), error => error === problem);
    const callbackError = keys.wrap(callback => callback());
    assert.throws(() => callbackError(() => { throw problem; }), error => error === problem);
    assert.equal(keys.pending, 0);
    assert.equal(await keys.wrap(async () => 17)(), 17);
    await assert.rejects(keys.wrap(async () => { throw problem; })(), error => error === problem);
    assert.equal(keys.pending, 0);
}
{
    const {keys, timers} = tracker();
    let callback;
    keys.wrap(done => { callback = done; })(() => {});
    const waiting = assert.rejects(keys.wait(), /wayland_text_unavailable/);
    keys.close();
    await waiting;
    assert.equal(timers.size, 0);
    await assert.rejects(keys.wait(), /wayland_text_unavailable/);
    callback();
    assert.equal(keys.pending, 0);
    assert.equal(keys.wrap(() => 3)(), 3, 'disabled observation leaves the original method intact');
}
console.log('PASS: key completion preserves callbacks/results/errors, waits for every prior filter, times out without replay, and releases waiters on disable');

{
    const {keys, expire, timers} = tracker();
    const state = new InputAcknowledgement(keys.schedule, keys.cancel);
    state.key();
    const waiting = state.before();
    state.acknowledge();
    await waiting;
    const serial = state.serial;
    const delivered = state.confirm(serial);
    state.acknowledge();
    await delivered;
    state.key();
    const unavailable = assert.rejects(state.before(), /wayland_text_unavailable/);
    expire(); await unavailable;
    // The failure before delivery is distinct from an unconfirmed commit.
    const unknown = assert.rejects(state.confirm(state.serial), /wayland_text_outcome_unknown/);
    expire(); await unknown;
    await assert.rejects(state.before(), /wayland_text_outcome_unknown/);
    state.acknowledge();
    await state.before();
    const pending = assert.rejects(state.after(state.serial), /wayland_text_unavailable/);
    state.close(); await pending;
    await assert.rejects(state.before(), /wayland_text_unavailable/);
    assert.equal(timers.size, 0);
}
console.log('PASS: application acknowledgements gate keys and following text; ambiguous commits block later text until acknowledged, without replay or retaining text contents');
