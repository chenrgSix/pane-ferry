import assert from 'node:assert/strict';
import {windowKind, preservesPopupFocus} from '../../apps/linux-agent/gnome-extension/windowPresentation.js';
const names = ['NORMAL', 'DIALOG', 'MODAL_DIALOG', 'MENU', 'DROPDOWN_MENU', 'POPUP_MENU', 'COMBO', 'TOOLTIP', 'DND', 'NOTIFICATION', 'OVERRIDE_OTHER', 'UTILITY'];
const types = Object.fromEntries(names.map((name, index) => [name, index]));
const win = (type, parent = null, override = false) => ({get_window_type: () => types[type], get_transient_for: () => parent, is_override_redirect: () => override});
for (const [type, kind] of Object.entries({NORMAL: 'window', DIALOG: 'dialog', MODAL_DIALOG: 'dialog', MENU: 'menu', DROPDOWN_MENU: 'menu', POPUP_MENU: 'menu', COMBO: 'menu', TOOLTIP: 'tooltip', DND: 'passive', NOTIFICATION: 'passive', OVERRIDE_OTHER: 'popup', UTILITY: 'window'})) {
    assert.equal(windowKind(win(type), types), kind);
}
assert.equal(windowKind(win('NORMAL', null, true), types), 'popup');
const owner = win('NORMAL'), menu = win('POPUP_MENU', owner), nested = win('COMBO', menu), other = win('NORMAL');
const scope = [owner, menu, nested, other];
assert.equal(preservesPopupFocus(owner, nested, scope, types), true);
assert.equal(preservesPopupFocus(owner, other, scope, types), false);
assert.equal(preservesPopupFocus(owner, win('COMBO', owner), scope, types), false);
const dialog = win('DIALOG', owner);
assert.equal(preservesPopupFocus(owner, dialog, [...scope, dialog], types), false);
menu.get_transient_for = () => nested;
assert.equal(preservesPopupFocus(owner, nested, scope, types), false);
console.log('PASS: compositor window kinds and scoped nested popup focus without application/size heuristics');
