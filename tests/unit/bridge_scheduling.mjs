import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
import {windowKind, preservesPopupFocus} from '../../apps/linux-agent/gnome-extension/windowPresentation.js';

const source = readFileSync(new URL('../../apps/linux-agent/gnome-extension/extension.js', import.meta.url), 'utf8')
    .replace(/^import .*;\n/gm, '').replace('export default class', 'class');
const types = {NORMAL: 0, POPUP_MENU: 1, DESKTOP: 2, DOCK: 3};
let now = 1;
function fixture(native = false) {
    const root = {get_client_type: () => native ? 1 : 2, get_pid: () => 123,
        get_id: () => 7, get_stable_sequence: () => 7, get_window_type: () => types.NORMAL,
        connect: () => 1, disconnect: () => {},
        get_transient_for: () => null, is_override_redirect: () => false,
        get_frame_rect: () => ({x: 0, y: 0, width: 800, height: 600}),
        get_client_content_rect: () => ({x: 0, y: 0, width: 800, height: 600}),
        stage_to_protocol_rect: rect => rect,
        get_work_area_current_monitor: () => ({x: 0, y: 0, width: 1920, height: 1080})};
    const main = {sessionMode: {isLocked: false, currentMode: 'user'}, activateWindow(w) { activations.push(w); }};
    const activations = [], windows = [root];
    const display = {focus_window: null, sort_windows_by_stacking: value => value};
    const ShellBridge = vm.runInNewContext(source + '\nPaneFerry;', {
        Extension: class {}, Gio: {}, GLib: {get_monotonic_time: () => ++now, uuid_string_random: () => `token-${++now}`,
            Variant: class { constructor(_signature, value) { this.value = value; } }},
        Meta: {WindowClientType: {WAYLAND: 1, X11: 2}, WindowType: types},
        Main: main, global: {get_window_actors: () => windows.map(meta_window => ({meta_window})), display}, windowKind, preservesPopupFocus, console,
    });
    const bridge = new ShellBridge();
    bridge.describe = w => ({window_id: w.get_stable_sequence()});
    const lease = {owner: ':test', root, output: native ? {} : null, lastUse: 0};
    bridge.leases = new Map([['token', lease]]);
    function call(op, extra = {}, owner = ':test') {
        const replies = [];
        const promise = bridge.CallAsync([JSON.stringify({op, token: 'token', window_id: 7, ...extra})], {
            get_sender: () => owner, return_value: value => replies.push(JSON.parse(value.value[0])),
        });
        return {promise, replies};
    }
    return {bridge, lease, root, main, display, activations, windows, call};
}
for (const native of [false, true]) {
    const f = fixture(native);
    f.bridge.ensureOutput = () => { throw new Error('ready lease must not await output setup'); };
    for (const [op, extra] of [['scope', {}], ['focus', {}], ['locate_x11', {x: 12, y: 34, width: 800, height: 600}]]) {
        const c = f.call(op, extra);
        assert.equal(c.replies.length, 1, `${op}: ready bridge yielded a shell turn`);
        assert.ok(c.replies[0].result);
        await c.promise;
    }
    assert.ok(f.lease.lastUse > 0, 'fast requests must keep the output active');
    assert.equal(f.activations.length, 1);
    let c = f.call('focus', {}, ':foreign');
    assert.equal(c.replies[0].error, 'window_unavailable'); await c.promise;
    f.main.sessionMode.isLocked = true;
    c = f.call('focus'); assert.equal(c.replies[0].error, 'desktop_locked'); await c.promise;
    f.main.sessionMode.isLocked = false; f.lease.cancelled = true;
    c = f.call('focus'); assert.equal(c.replies[0].error, 'window_unavailable'); await c.promise;
}
for (const state of ['opening', 'suspending']) for (const cancelled of [false, true]) {
    const f = fixture(true);
    let resolve;
    f.lease[state] = new Promise(done => { resolve = done; });
    const c = f.call('focus');
    assert.equal(c.replies.length, 0, 'opening output must stay asynchronous');
    if (cancelled) { f.lease.cancelled = true; f.bridge.leases.delete('token'); }
    resolve(); await c.promise;
    assert.equal(c.replies.length, 1);
    assert.equal(c.replies[0].error, cancelled ? 'window_unavailable' : undefined);
    assert.equal(f.activations.length, cancelled ? 0 : 1);
}
{
    const f = fixture(true); f.lease.output = null;
    let resolve, called = 0;
    f.bridge.ensureOutput = () => { called++; return new Promise(done => { resolve = done; }); };
    const c = f.call('scope');
    assert.equal(called, 1); assert.equal(c.replies.length, 0);
    resolve(); await c.promise; assert.ok(c.replies[0].result);
}
const f = fixture();
const popup = {...f.root, get_window_type: () => types.POPUP_MENU, get_transient_for: () => f.root};
f.windows.push(popup); f.display.focus_window = popup;
const c = f.call('focus'); await c.promise;
assert.ok(c.replies[0].result); assert.equal(f.activations.length, 0);
console.log('PASS: ready X11/Wayland bridge replies without a Promise turn; opening/cancelled output, ownership, desktop lock and popup rules retained');

// Run the actual lease acquisition and access checks on mixed-protocol trees.
// X11 -> native dialog -> X11 child must belong to two disjoint capture scopes.
for (const nativeFirst of [false, true]) {
    const f = fixture();
    function child(id, native, parent) {
        const window = {...f.root, get_id: () => id, get_stable_sequence: () => id,
            get_client_type: () => native ? 1 : 2, get_transient_for: () => parent};
        f.windows.push(window); return window;
    }
    const menu = child(8, false, f.root);
    const dialog = child(9, true, f.root);
    const nestedX11 = child(10, false, dialog);
    const nestedNative = child(11, true, nestedX11);
    const unrelated = child(12, true, null);
    assert.deepEqual(Array.from(f.bridge.scope(f.root)), [f.root, menu]);
    assert.deepEqual(Array.from(f.bridge.scope(dialog)), [dialog, nestedX11, nestedNative]);
    f.bridge.leases.clear();
    f.bridge.ensureOutput = lease => { lease.output = {close() {}}; return null; };
    const tokens = new Map();
    for (const root of nativeFirst ? [dialog, f.root] : [f.root, dialog]) {
        const result = f.call('acquire', {id: String(root.get_id()), pid: 123});
        await result.promise;
        assert.ok(result.replies[0].result, JSON.stringify(result.replies));
        tokens.set(root, result.replies[0].result.token);
    }
    for (const root of [f.root, menu, dialog, nestedX11, nestedNative]) {
        const duplicate = f.call('acquire', {id: String(root.get_id()), pid: 123}, ':another');
        await duplicate.promise; assert.equal(duplicate.replies[0].error, 'session_controlled');
    }
    for (const [owner, target, allowed] of [[f.root, menu, true], [dialog, nestedX11, true],
        [f.root, dialog, false], [f.root, nestedX11, false], [dialog, f.root, false], [dialog, unrelated, false]]) {
        const access = f.call('focus', {token: tokens.get(owner), window_id: target.get_stable_sequence()});
        await access.promise;
        assert.equal(access.replies[0].error, allowed ? undefined : 'window_unavailable');
    }
    const foreign = f.call('scope', {token: tokens.get(dialog)}, ':another');
    await foreign.promise; assert.equal(foreign.replies[0].error, 'window_unavailable');
    const cycle = child(13, false, null); cycle.get_transient_for = () => cycle;
    assert.ok(!f.bridge.scope(f.root).includes(cycle));
}
console.log('PASS: mixed X11/Wayland leases follow capture capability in both acquisition orders; native subtrees, duplicate protection and input boundaries retained');
