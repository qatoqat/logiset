//! Names for HID usages: keyboard page (0x07) and common consumer-page (0x0C) controls.

/// Keyboard/keypad page usages that a key action can send.
pub const KEYS: &[(u8, &str)] = &[
    (0x04, "A"), (0x05, "B"), (0x06, "C"), (0x07, "D"), (0x08, "E"), (0x09, "F"), (0x0A, "G"),
    (0x0B, "H"), (0x0C, "I"), (0x0D, "J"), (0x0E, "K"), (0x0F, "L"), (0x10, "M"), (0x11, "N"),
    (0x12, "O"), (0x13, "P"), (0x14, "Q"), (0x15, "R"), (0x16, "S"), (0x17, "T"), (0x18, "U"),
    (0x19, "V"), (0x1A, "W"), (0x1B, "X"), (0x1C, "Y"), (0x1D, "Z"),
    (0x1E, "1"), (0x1F, "2"), (0x20, "3"), (0x21, "4"), (0x22, "5"), (0x23, "6"), (0x24, "7"),
    (0x25, "8"), (0x26, "9"), (0x27, "0"),
    (0x28, "Enter"), (0x29, "Escape"), (0x2A, "Backspace"), (0x2B, "Tab"), (0x2C, "Space"),
    (0x2D, "- _"), (0x2E, "= +"), (0x2F, "[ {"), (0x30, "] }"), (0x31, "\\ |"), (0x32, "Non-US # ~"),
    (0x33, "; :"), (0x34, "' \""), (0x35, "` ~"), (0x36, ", <"), (0x37, ". >"), (0x38, "/ ?"),
    (0x39, "Caps Lock"),
    (0x3A, "F1"), (0x3B, "F2"), (0x3C, "F3"), (0x3D, "F4"), (0x3E, "F5"), (0x3F, "F6"),
    (0x40, "F7"), (0x41, "F8"), (0x42, "F9"), (0x43, "F10"), (0x44, "F11"), (0x45, "F12"),
    (0x46, "Print Screen"), (0x47, "Scroll Lock"), (0x48, "Pause"), (0x49, "Insert"), (0x4A, "Home"),
    (0x4B, "Page Up"), (0x4C, "Delete"), (0x4D, "End"), (0x4E, "Page Down"), (0x4F, "Right"),
    (0x50, "Left"), (0x51, "Down"), (0x52, "Up"), (0x53, "Num Lock"),
    (0x54, "Keypad /"), (0x55, "Keypad *"), (0x56, "Keypad -"), (0x57, "Keypad +"), (0x58, "Keypad Enter"),
    (0x59, "Keypad 1"), (0x5A, "Keypad 2"), (0x5B, "Keypad 3"), (0x5C, "Keypad 4"), (0x5D, "Keypad 5"),
    (0x5E, "Keypad 6"), (0x5F, "Keypad 7"), (0x60, "Keypad 8"), (0x61, "Keypad 9"), (0x62, "Keypad 0"),
    (0x63, "Keypad ."), (0x64, "Non-US \\ |"), (0x65, "Menu"),
    (0x68, "F13"), (0x69, "F14"), (0x6A, "F15"), (0x6B, "F16"), (0x6C, "F17"), (0x6D, "F18"),
    (0x6E, "F19"), (0x6F, "F20"), (0x70, "F21"), (0x71, "F22"), (0x72, "F23"), (0x73, "F24"),
    (0x7F, "Mute (key)"), (0x80, "Volume Up (key)"), (0x81, "Volume Down (key)"),
    (0xE0, "Left Ctrl"), (0xE1, "Left Shift"), (0xE2, "Left Alt"), (0xE3, "Left Super"),
    (0xE4, "Right Ctrl"), (0xE5, "Right Shift"), (0xE6, "Right Alt"), (0xE7, "Right Super"),
];

/// Consumer page usages commonly bound to mouse buttons.
pub const CONSUMER: &[(u16, &str)] = &[
    (0x006F, "Brightness Up"), (0x0070, "Brightness Down"),
    (0x00B5, "Next Track"), (0x00B6, "Previous Track"), (0x00B7, "Stop"), (0x00CD, "Play/Pause"),
    (0x00E2, "Mute"), (0x00E9, "Volume Up"), (0x00EA, "Volume Down"),
    (0x0183, "Media Player"), (0x018A, "Mail"), (0x0192, "Calculator"), (0x0194, "File Browser"),
    (0x0221, "Search"), (0x0223, "Browser Home"), (0x0224, "Browser Back"), (0x0225, "Browser Forward"),
    (0x0226, "Browser Stop"), (0x0227, "Browser Refresh"), (0x022A, "Bookmarks"),
    (0x022D, "Zoom In"), (0x022E, "Zoom Out"), (0x0238, "Horizontal Scroll (AC Pan)"),
];

/// Modifier bits as used by HID keyboard reports and onboard-profile key actions.
pub const MODIFIERS: &[(u8, &str)] = &[
    (0x01, "Ctrl"), (0x02, "Shift"), (0x04, "Alt"), (0x08, "Super"),
    (0x10, "Right Ctrl"), (0x20, "Right Shift"), (0x40, "Right Alt"), (0x80, "Right Super"),
];

pub fn key_name(usage: u8) -> String {
    KEYS.iter().find(|(u, _)| *u == usage).map(|(_, n)| (*n).to_owned()).unwrap_or_else(|| format!("Key 0x{usage:02X}"))
}

pub fn consumer_name(usage: u16) -> String {
    CONSUMER
        .iter()
        .find(|(u, _)| *u == usage)
        .map(|(_, n)| (*n).to_owned())
        .unwrap_or_else(|| format!("Consumer 0x{usage:04X}"))
}

/// `Ctrl+Shift` style text for a modifier bitmask (empty when none).
pub fn modifiers_text(mask: u8) -> String {
    MODIFIERS.iter().filter(|(bit, _)| mask & bit != 0).map(|(_, n)| *n).collect::<Vec<_>>().join("+")
}
