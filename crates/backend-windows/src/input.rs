//! Both existing receivers are accepted; textual composition stays at the receiver.
pub fn keysym(value: u32) -> Option<u16> {
    match value {
        0x61..=0x7a => Some((value - 32) as u16),
        0x41..=0x5a | 0x30..=0x39 => Some(value as u16),
        0xffbe..=0xffd5 => Some((0x70 + value - 0xffbe) as u16),
        0xff08 => Some(0x08),
        0xff09 | 0xfe20 => Some(0x09),
        0xff0d => Some(0x0d),
        0xff1b => Some(0x1b),
        0x20 => Some(0x20),
        0xff50 => Some(0x24),
        0xff51 => Some(0x25),
        0xff52 => Some(0x26),
        0xff53 => Some(0x27),
        0xff54 => Some(0x28),
        0xff55 => Some(0x21),
        0xff56 => Some(0x22),
        0xff57 => Some(0x23),
        0xff63 => Some(0x2d),
        0xffff => Some(0x2e),
        0xffe1 => Some(0xa0),
        0xffe2 => Some(0xa1),
        0xffe3 => Some(0xa2),
        0xffe4 => Some(0xa3),
        0xffe9 => Some(0xa4),
        0xffea => Some(0xa5),
        0xffeb => Some(0x5b),
        0xffec => Some(0x5c),
        // OEM punctuation on the initial supported US keyboard layout.
        0x3b => Some(0xba),
        0x3d => Some(0xbb),
        0x2c => Some(0xbc),
        0x2d => Some(0xbd),
        0x2e => Some(0xbe),
        0x2f => Some(0xbf),
        0x60 => Some(0xc0),
        0x5b => Some(0xdb),
        0x5c => Some(0xdc),
        0x5d => Some(0xdd),
        0x27 => Some(0xde),
        _ => None,
    }
}

pub fn logical(key: &str) -> Option<u16> {
    if key.len() == 1 {
        return keysym(key.as_bytes()[0] as u32);
    }
    if let Some(n) = key.strip_prefix('F').and_then(|n| n.parse::<u32>().ok()) {
        if (1..=24).contains(&n) {
            return keysym(0xffbe + n - 1);
        }
    }
    let symbol = match key {
        "BackSpace" => 0xff08,
        "Tab" | "ISO_Left_Tab" => 0xff09,
        "Return" | "KP_Enter" => 0xff0d,
        "Escape" => 0xff1b,
        "space" => 0x20,
        "Home" => 0xff50,
        "Left" => 0xff51,
        "Up" => 0xff52,
        "Right" => 0xff53,
        "Down" => 0xff54,
        "Page_Up" => 0xff55,
        "Page_Down" => 0xff56,
        "End" => 0xff57,
        "Insert" => 0xff63,
        "Delete" => 0xffff,
        "Shift_L" => 0xffe1,
        "Shift_R" => 0xffe2,
        "Control_L" => 0xffe3,
        "Control_R" => 0xffe4,
        "Alt_L" => 0xffe9,
        "Alt_R" => 0xffea,
        "Super_L" => 0xffeb,
        "Super_R" => 0xffec,
        "semicolon" => 0x3b,
        "equal" => 0x3d,
        "comma" => 0x2c,
        "minus" => 0x2d,
        "period" => 0x2e,
        "slash" => 0x2f,
        "grave" => 0x60,
        "bracketleft" => 0x5b,
        "backslash" => 0x5c,
        "bracketright" => 0x5d,
        "apostrophe" => 0x27,
        _ => return None,
    };
    keysym(symbol)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn both_receivers_map_control_to_windows_control() {
        assert_eq!(logical("Control_L"), keysym(0xffe3));
        assert_eq!(logical("Control_L"), Some(0xa2));
        assert_eq!(logical("a"), keysym(65));
        assert_eq!(logical("F12"), Some(0x7b));
        assert_eq!(keysym(0x01004e2d), None); // Chinese is input.text.commit.
        assert_eq!(logical("unsupported"), None);
    }
}
