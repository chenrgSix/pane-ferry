// Compositor semantics, never application names, titles, PIDs or size heuristics.
export function windowKind(window, types) {
    const type = window.get_window_type();
    if (type === types.TOOLTIP) return 'tooltip';
    if (type === types.DND || type === types.NOTIFICATION) return 'passive';
    if ([types.MENU, types.DROPDOWN_MENU, types.POPUP_MENU, types.COMBO].includes(type)) return 'menu';
    if (window.is_override_redirect() || type === types.OVERRIDE_OTHER) return 'popup';
    if ([types.DIALOG, types.MODAL_DIALOG].includes(type)) return 'dialog';
    return 'window';
}

export function preservesPopupFocus(target, focused, windows, types) {
    // Keyboard grabs belong to the compositor. Activating the owner on every
    // key would dismiss its menu/combobox before that key can reach the grab.
    if (!focused || !windows.includes(focused)) return false;
    const seen = new Set();
    for (let at = focused; at && !seen.has(at); at = at.get_transient_for()) {
        if (at === target) return true;
        if (!['menu', 'popup'].includes(windowKind(at, types))) return false;
        seen.add(at);
    }
    return false;
}
