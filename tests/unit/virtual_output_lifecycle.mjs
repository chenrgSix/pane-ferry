// Exercise the production state machine with delayed D-Bus replies and native
// window lifetime signals. Native calls on a dying window are recorded even if
// the implementation swallows the fake exception (real Mutter can SIGSEGV).
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';

const source = readFileSync(new URL('../../apps/linux-agent/gnome-extension/virtualOutput.js', import.meta.url), 'utf8')
    .replace(/^import .*;\n/gm, '').replace(/^export /gm, '');

function fixture({hold, fail, scale = 1} = {}) {
    const requests = [], pending = [], timers = [], unsafe = [], moves = [], handlers = new Map();
    let dying = false, monitor = 0, virtual = false, nextHandler = 0;
    const check = method => {
        if (dying || monitor < 0) { unsafe.push(method); throw new Error('native window is unmanaged'); }
    };
    const root = {
        get_monitor: () => monitor,
        get_frame_rect: () => ({x: 12, y: 24, width: 700, height: 450}),
        get_maximize_flags: () => 0, is_fullscreen: () => false,
        connect: (signal, callback) => { const id = ++nextHandler; handlers.set(id, {signal, callback}); return id; },
        disconnect: id => handlers.delete(id),
        unmaximize: () => check('unmaximize'),
        move_to_monitor: index => { check('move_to_monitor'); moves.push(index); monitor = index; },
        get_work_area_for_monitor: () => ({x: 0, y: 0, width: 1600, height: 900}),
        move_resize_frame: () => check('move_resize_frame'),
        get_compositor_private: () => { check('get_compositor_private'); return {get_resource_scale: () => scale}; },
    };
    const manager = {
        get_monitors: () => ['DP-2', ...(virtual ? ['Meta-0'] : [])].map(name => ({get_connector: () => name})),
        get_monitor_for_connector: name => name === 'DP-2' ? 0 : virtual ? 1 : -1,
    };
    const Gio = {DBusCallFlags: {NO_AUTO_START: 0}, DBus: {session: {
        call(_bus, _path, _iface, method, _params, _reply, _flags, _timeout, _cancel, callback) {
            requests.push(method);
            const reply = () => {
                if (method === 'Start') virtual = true;
                if (method === 'Stop') virtual = false;
                callback({call_finish() {
                    if (method === fail) throw new Error('injected D-Bus failure');
                    return {deep_unpack: () => method === 'CreateSession' ? ['/session/1'] : []};
                }}, {});
            };
            if (method === hold) pending.push(reply); else queueMicrotask(reply);
        },
    }}};
    const GLib = {Variant: class {}, PRIORITY_DEFAULT: 0, SOURCE_REMOVE: false,
        timeout_add(_priority, _ms, callback) { timers.push(callback); }};
    const VirtualOutput = vm.runInNewContext(source + '\nVirtualOutput;',
        {Gio, GLib, global: {backend: {get_monitor_manager: () => manager}}});
    return {VirtualOutput, root, requests, pending, timers, unsafe, moves, handlers,
        dying(clearMonitor = true) {
            dying = true;
            for (const {signal, callback} of handlers.values()) if (signal === 'unmanaging') callback();
            if (clearMonitor) monitor = -1;
        },
        loseMonitor() { monitor = -1; },
        assertClean() { assert.deepEqual(unsafe, []); assert.equal(handlers.size, 0); assert.equal(virtual, false); },
    };
}
const tick = () => new Promise(resolve => setImmediate(resolve));
let passed = 0;
async function test(name, run) { await run(); passed++; console.log('PASS:', name); }

await test('normal release restores a living window and repeated close stops exactly once', async () => {
    const f = fixture(), output = new f.VirtualOutput(f.root);
    await output.open(1);
    await Promise.all([output.close(), output.close()]);
    assert.deepEqual(f.moves, [1, 0]);
    assert.equal(f.requests.filter(x => x === 'Stop').length, 1);
    f.assertClean();
});
for (const clearMonitor of [false, true]) await test(`unmanaging forbids native restore, monitor cleared=${clearMonitor}`, async () => {
    const f = fixture(), output = new f.VirtualOutput(f.root);
    await output.open(1);
    f.dying(clearMonitor);
    await output.close();
    assert.deepEqual(f.moves, [1]);
    assert.equal(f.requests.filter(x => x === 'Stop').length, 1);
    f.assertClean();
});
await test('monitor absence is also checked immediately before restore', async () => {
    const f = fixture(), output = new f.VirtualOutput(f.root);
    await output.open(1);
    f.loseMonitor();
    await output.close();
    assert.deepEqual(f.moves, [1]);
    f.assertClean();
});
await test('reject a stale root before registering signals or provisioning', async () => {
    const f = fixture(); f.loseMonitor();
    assert.throws(() => new f.VirtualOutput(f.root), /window_unavailable/);
    assert.deepEqual(f.requests, []); f.assertClean();
});
for (const hold of ['CreateSession', 'RecordVirtual', 'Start']) await test(`window closes while awaiting ${hold}`, async () => {
    const f = fixture({hold}), output = new f.VirtualOutput(f.root);
    const opening = assert.rejects(output.open(1), /window_unavailable/);
    await tick(); assert.equal(f.pending.length, 1);
    f.dying();
    const closing = output.close();
    f.pending.shift()();
    await opening; await closing;
    assert.deepEqual(f.moves, []);
    assert.equal(f.requests.filter(x => x === 'Stop').length, 1);
    f.assertClean();
});
await test('window closes during client repaint wait', async () => {
    const f = fixture(), output = new f.VirtualOutput(f.root);
    const opening = assert.rejects(output.open(2), /window_unavailable/);
    await tick(); assert.equal(f.timers.length, 1);
    f.dying(); const closing = output.close(); f.timers.shift()();
    await opening; await closing; f.assertClean();
});
await test('idle release retains its own lifetime observer while close is pending', async () => {
    const f = fixture(), output = new f.VirtualOutput(f.root);
    await output.open(1);
    const closing = output.close();
    f.dying(false);
    await closing; assert.deepEqual(f.moves, [1]); f.assertClean();
});
await test('failed acquisition still stops the cast session and disconnects observers', async () => {
    const f = fixture({fail: 'RecordVirtual'}), output = new f.VirtualOutput(f.root);
    await assert.rejects(output.open(1), /injected/);
    await output.close();
    assert.equal(f.requests.filter(x => x === 'Stop').length, 1); f.assertClean();
});
console.log(`PASS: ${passed} virtual output lifecycle cases`);
