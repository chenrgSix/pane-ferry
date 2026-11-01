// A compositor scene may follow explicit transient relationships across window
// protocols. Ordinary XComposite capture must retain its original boundary.
import assert from 'node:assert/strict';
import {readFileSync} from 'node:fs';
import vm from 'node:vm';
const code = readFileSync(new URL('../../apps/linux-agent/gnome-extension/extension.js', import.meta.url), 'utf8')
  .replace(/^import .*;\n/gm, '').replace('export default class', 'class');
const Meta = {WindowClientType: {X11: 0, WAYLAND: 1}};
const PaneFerry = vm.runInNewContext(code + '\nPaneFerry', {
  Extension: class {}, Meta, GLib: {get_monotonic_time: () => 1},
});
function window(type, parent = null) {
  return {get_client_type: () => type, get_transient_for: () => parent};
}
const root = window(0), menu = window(0, root), portal = window(1, root);
const nested = window(1, portal), x11Child = window(0, portal), stranger = window(0);
const bridge = new PaneFerry();
bridge.windows = () => [root, menu, portal, nested, x11Child, stranger];
assert.deepEqual([...bridge.scope(root)], [root, menu]);
assert.deepEqual([...bridge.scope(root, true)], [root, menu, portal, nested, x11Child]);
assert.equal(bridge.scope(stranger, true).length, 1);
const cyclic = window(1);cyclic.get_transient_for = () => cyclic;
bridge.windows = () => [root, cyclic];
assert.deepEqual([...bridge.scope(root, true)], [root]);
let opened = 0;
bridge.ensureOutput = () => { opened++; return 'opening'; };
bridge.prepareOutput({root});assert.equal(opened, 0);
assert.equal(bridge.prepareOutput({root, canvas: {width: 1600, height: 900}}), 'opening');
assert.equal(opened, 1);
console.log('PASS: explicit cross-protocol transient scope, foreign roots and cycle rejection');
