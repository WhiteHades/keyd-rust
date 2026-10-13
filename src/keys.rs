// keyd - A key remapping daemon.
// © 2019 Raheman Vaiya (see also: LICENSE). Ported from frozen f564288.

pub const MOD_ALT_GR: u8 = 0x10;
pub const MOD_CTRL: u8 = 0x8;
pub const MOD_SHIFT: u8 = 0x4;
pub const MOD_SUPER: u8 = 0x2;
pub const MOD_ALT: u8 = 0x1;
pub const MAX_MOD: usize = 5;
pub const KEYD_ESC: u8 = 1;
pub const KEYD_1: u8 = 2;
pub const KEYD_2: u8 = 3;
pub const KEYD_3: u8 = 4;
pub const KEYD_4: u8 = 5;
pub const KEYD_5: u8 = 6;
pub const KEYD_6: u8 = 7;
pub const KEYD_7: u8 = 8;
pub const KEYD_8: u8 = 9;
pub const KEYD_9: u8 = 10;
pub const KEYD_0: u8 = 11;
pub const KEYD_MINUS: u8 = 12;
pub const KEYD_EQUAL: u8 = 13;
pub const KEYD_BACKSPACE: u8 = 14;
pub const KEYD_TAB: u8 = 15;
pub const KEYD_Q: u8 = 16;
pub const KEYD_W: u8 = 17;
pub const KEYD_E: u8 = 18;
pub const KEYD_R: u8 = 19;
pub const KEYD_T: u8 = 20;
pub const KEYD_Y: u8 = 21;
pub const KEYD_U: u8 = 22;
pub const KEYD_I: u8 = 23;
pub const KEYD_O: u8 = 24;
pub const KEYD_P: u8 = 25;
pub const KEYD_LEFTBRACE: u8 = 26;
pub const KEYD_RIGHTBRACE: u8 = 27;
pub const KEYD_ENTER: u8 = 28;
pub const KEYD_LEFTCTRL: u8 = 29;
pub const KEYD_A: u8 = 30;
pub const KEYD_S: u8 = 31;
pub const KEYD_D: u8 = 32;
pub const KEYD_F: u8 = 33;
pub const KEYD_G: u8 = 34;
pub const KEYD_H: u8 = 35;
pub const KEYD_J: u8 = 36;
pub const KEYD_K: u8 = 37;
pub const KEYD_L: u8 = 38;
pub const KEYD_SEMICOLON: u8 = 39;
pub const KEYD_APOSTROPHE: u8 = 40;
pub const KEYD_GRAVE: u8 = 41;
pub const KEYD_LEFTSHIFT: u8 = 42;
pub const KEYD_BACKSLASH: u8 = 43;
pub const KEYD_Z: u8 = 44;
pub const KEYD_X: u8 = 45;
pub const KEYD_C: u8 = 46;
pub const KEYD_V: u8 = 47;
pub const KEYD_B: u8 = 48;
pub const KEYD_N: u8 = 49;
pub const KEYD_M: u8 = 50;
pub const KEYD_COMMA: u8 = 51;
pub const KEYD_DOT: u8 = 52;
pub const KEYD_SLASH: u8 = 53;
pub const KEYD_RIGHTSHIFT: u8 = 54;
pub const KEYD_KPASTERISK: u8 = 55;
pub const KEYD_LEFTALT: u8 = 56;
pub const KEYD_SPACE: u8 = 57;
pub const KEYD_CAPSLOCK: u8 = 58;
pub const KEYD_F1: u8 = 59;
pub const KEYD_F2: u8 = 60;
pub const KEYD_F3: u8 = 61;
pub const KEYD_F4: u8 = 62;
pub const KEYD_F5: u8 = 63;
pub const KEYD_F6: u8 = 64;
pub const KEYD_F7: u8 = 65;
pub const KEYD_F8: u8 = 66;
pub const KEYD_F9: u8 = 67;
pub const KEYD_F10: u8 = 68;
pub const KEYD_NUMLOCK: u8 = 69;
pub const KEYD_SCROLLLOCK: u8 = 70;
pub const KEYD_KP7: u8 = 71;
pub const KEYD_KP8: u8 = 72;
pub const KEYD_KP9: u8 = 73;
pub const KEYD_KPMINUS: u8 = 74;
pub const KEYD_KP4: u8 = 75;
pub const KEYD_KP5: u8 = 76;
pub const KEYD_KP6: u8 = 77;
pub const KEYD_KPPLUS: u8 = 78;
pub const KEYD_KP1: u8 = 79;
pub const KEYD_KP2: u8 = 80;
pub const KEYD_KP3: u8 = 81;
pub const KEYD_KP0: u8 = 82;
pub const KEYD_KPDOT: u8 = 83;
pub const KEYD_IS_LEVEL3_SHIFT: u8 = 84;
pub const KEYD_ZENKAKUHANKAKU: u8 = 85;
pub const KEYD_102ND: u8 = 86;
pub const KEYD_F11: u8 = 87;
pub const KEYD_F12: u8 = 88;
pub const KEYD_RO: u8 = 89;
pub const KEYD_KATAKANA: u8 = 90;
pub const KEYD_HIRAGANA: u8 = 91;
pub const KEYD_HENKAN: u8 = 92;
pub const KEYD_KATAKANAHIRAGANA: u8 = 93;
pub const KEYD_MUHENKAN: u8 = 94;
pub const KEYD_KPJPCOMMA: u8 = 95;
pub const KEYD_KPENTER: u8 = 96;
pub const KEYD_RIGHTCTRL: u8 = 97;
pub const KEYD_KPSLASH: u8 = 98;
pub const KEYD_SYSRQ: u8 = 99;
pub const KEYD_RIGHTALT: u8 = 100;
pub const KEYD_LINEFEED: u8 = 101;
pub const KEYD_HOME: u8 = 102;
pub const KEYD_UP: u8 = 103;
pub const KEYD_PAGEUP: u8 = 104;
pub const KEYD_LEFT: u8 = 105;
pub const KEYD_RIGHT: u8 = 106;
pub const KEYD_END: u8 = 107;
pub const KEYD_DOWN: u8 = 108;
pub const KEYD_PAGEDOWN: u8 = 109;
pub const KEYD_INSERT: u8 = 110;
pub const KEYD_DELETE: u8 = 111;
pub const KEYD_MACRO: u8 = 112;
pub const KEYD_MUTE: u8 = 113;
pub const KEYD_VOLUMEDOWN: u8 = 114;
pub const KEYD_VOLUMEUP: u8 = 115;
pub const KEYD_POWER: u8 = 116;
pub const KEYD_KPEQUAL: u8 = 117;
pub const KEYD_KPPLUSMINUS: u8 = 118;
pub const KEYD_PAUSE: u8 = 119;
pub const KEYD_SCALE: u8 = 120;
pub const KEYD_KPCOMMA: u8 = 121;
pub const KEYD_HANGEUL: u8 = 122;
pub const KEYD_HANJA: u8 = 123;
pub const KEYD_YEN: u8 = 124;
pub const KEYD_LEFTMETA: u8 = 125;
pub const KEYD_RIGHTMETA: u8 = 126;
pub const KEYD_COMPOSE: u8 = 127;
pub const KEYD_STOP: u8 = 128;
pub const KEYD_AGAIN: u8 = 129;
pub const KEYD_PROPS: u8 = 130;
pub const KEYD_UNDO: u8 = 131;
pub const KEYD_FRONT: u8 = 132;
pub const KEYD_COPY: u8 = 133;
pub const KEYD_OPEN: u8 = 134;
pub const KEYD_PASTE: u8 = 135;
pub const KEYD_FIND: u8 = 136;
pub const KEYD_CUT: u8 = 137;
pub const KEYD_HELP: u8 = 138;
pub const KEYD_MENU: u8 = 139;
pub const KEYD_CALC: u8 = 140;
pub const KEYD_SETUP: u8 = 141;
pub const KEYD_SLEEP: u8 = 142;
pub const KEYD_WAKEUP: u8 = 143;
pub const KEYD_FILE: u8 = 144;
pub const KEYD_SENDFILE: u8 = 145;
pub const KEYD_DELETEFILE: u8 = 146;
pub const KEYD_XFER: u8 = 147;
pub const KEYD_SCROLL_DOWN: u8 = 148;
pub const KEYD_SCROLL_UP: u8 = 149;
pub const KEYD_WWW: u8 = 150;
pub const KEYD_MSDOS: u8 = 151;
pub const KEYD_COFFEE: u8 = 152;
pub const KEYD_ROTATE_DISPLAY: u8 = 153;
pub const KEYD_CYCLEWINDOWS: u8 = 154;
pub const KEYD_MAIL: u8 = 155;
pub const KEYD_BOOKMARKS: u8 = 156;
pub const KEYD_COMPUTER: u8 = 157;
pub const KEYD_BACK: u8 = 158;
pub const KEYD_FORWARD: u8 = 159;
pub const KEYD_CLOSECD: u8 = 160;
pub const KEYD_EJECTCD: u8 = 161;
pub const KEYD_EJECTCLOSECD: u8 = 162;
pub const KEYD_NEXTSONG: u8 = 163;
pub const KEYD_PLAYPAUSE: u8 = 164;
pub const KEYD_PREVIOUSSONG: u8 = 165;
pub const KEYD_STOPCD: u8 = 166;
pub const KEYD_RECORD: u8 = 167;
pub const KEYD_REWIND: u8 = 168;
pub const KEYD_PHONE: u8 = 169;
pub const KEYD_ISO: u8 = 170;
pub const KEYD_CONFIG: u8 = 171;
pub const KEYD_HOMEPAGE: u8 = 172;
pub const KEYD_REFRESH: u8 = 173;
pub const KEYD_EXIT: u8 = 174;
pub const KEYD_MOVE: u8 = 175;
pub const KEYD_EDIT: u8 = 176;
pub const KEYD_ZOOM: u8 = 177;
pub const KEYD_KPLEFTPAREN: u8 = 179;
pub const KEYD_KPRIGHTPAREN: u8 = 180;
pub const KEYD_NEW: u8 = 181;
pub const KEYD_REDO: u8 = 182;
pub const KEYD_F13: u8 = 183;
pub const KEYD_F14: u8 = 184;
pub const KEYD_F15: u8 = 185;
pub const KEYD_F16: u8 = 186;
pub const KEYD_F17: u8 = 187;
pub const KEYD_F18: u8 = 188;
pub const KEYD_F19: u8 = 189;
pub const KEYD_F20: u8 = 190;
pub const KEYD_F21: u8 = 191;
pub const KEYD_F22: u8 = 192;
pub const KEYD_F23: u8 = 193;
pub const KEYD_F24: u8 = 194;
pub const KEYD_PLAYCD: u8 = 200;
pub const KEYD_PAUSECD: u8 = 201;
pub const KEYD_SCROLL_LEFT: u8 = 202;
pub const KEYD_SCROLL_RIGHT: u8 = 203;
pub const KEYD_DASHBOARD: u8 = 204;
pub const KEYD_SUSPEND: u8 = 205;
pub const KEYD_CLOSE: u8 = 206;
pub const KEYD_PLAY: u8 = 207;
pub const KEYD_FASTFORWARD: u8 = 208;
pub const KEYD_BASSBOOST: u8 = 209;
pub const KEYD_PRINT: u8 = 210;
pub const KEYD_HP: u8 = 211;
pub const KEYD_CAMERA: u8 = 212;
pub const KEYD_SOUND: u8 = 213;
pub const KEYD_QUESTION: u8 = 214;
pub const KEYD_EMAIL: u8 = 215;
pub const KEYD_CHAT: u8 = 216;
pub const KEYD_SEARCH: u8 = 217;
pub const KEYD_CONNECT: u8 = 218;
pub const KEYD_FINANCE: u8 = 219;
pub const KEYD_SPORT: u8 = 220;
pub const KEYD_SHOP: u8 = 221;
pub const KEYD_VOICECOMMAND: u8 = 222;
pub const KEYD_CANCEL: u8 = 223;
pub const KEYD_BRIGHTNESSDOWN: u8 = 224;
pub const KEYD_BRIGHTNESSUP: u8 = 225;
pub const KEYD_MEDIA: u8 = 226;
pub const KEYD_SWITCHVIDEOMODE: u8 = 227;
pub const KEYD_KBDILLUMTOGGLE: u8 = 228;
pub const KEYD_KBDILLUMDOWN: u8 = 229;
pub const KEYD_KBDILLUMUP: u8 = 230;
pub const KEYD_SEND: u8 = 231;
pub const KEYD_REPLY: u8 = 232;
pub const KEYD_FORWARDMAIL: u8 = 233;
pub const KEYD_SAVE: u8 = 234;
pub const KEYD_DOCUMENTS: u8 = 235;
pub const KEYD_BATTERY: u8 = 236;
pub const KEYD_BLUETOOTH: u8 = 237;
pub const KEYD_WLAN: u8 = 238;
pub const KEYD_UWB: u8 = 239;
pub const KEYD_UNKNOWN: u8 = 240;
pub const KEYD_VIDEO_NEXT: u8 = 241;
pub const KEYD_VIDEO_PREV: u8 = 242;
pub const KEYD_BRIGHTNESS_CYCLE: u8 = 243;
pub const KEYD_BRIGHTNESS_AUTO: u8 = 244;
pub const KEYD_DISPLAY_OFF: u8 = 245;
pub const KEYD_WWAN: u8 = 246;
pub const KEYD_RFKILL: u8 = 247;
pub const KEYD_MICMUTE: u8 = 248;
pub const KEYD_NOOP: u8 = 195;
pub const KEYD_EXTERNAL_MOUSE_BUTTON: u8 = 196;
pub const KEYD_CHORD_1: u8 = 197;
pub const KEYD_CHORD_2: u8 = 198;
pub const KEYD_CHORD_MAX: u8 = 199;
pub const KEYD_LEFT_MOUSE: u8 = 249;
pub const KEYD_MIDDLE_MOUSE: u8 = 250;
pub const KEYD_RIGHT_MOUSE: u8 = 251;
pub const KEYD_MOUSE_1: u8 = 252;
pub const KEYD_MOUSE_2: u8 = 253;
pub const KEYD_MOUSE_BACK: u8 = 178;
pub const KEYD_FN: u8 = 254;
pub const KEYD_MOUSE_FORWARD: u8 = 255;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyName {
    pub name: Option<&'static str>,
    pub alt_name: Option<&'static str>,
    pub shifted_name: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifier {
    pub mask: u8,
    pub key: u8,
}

pub const MODIFIERS: [Modifier; MAX_MOD] = [
    Modifier {
        mask: MOD_ALT,
        key: KEYD_LEFTALT,
    },
    Modifier {
        mask: MOD_ALT_GR,
        key: KEYD_RIGHTALT,
    },
    Modifier {
        mask: MOD_SHIFT,
        key: KEYD_LEFTSHIFT,
    },
    Modifier {
        mask: MOD_SUPER,
        key: KEYD_LEFTMETA,
    },
    Modifier {
        mask: MOD_CTRL,
        key: KEYD_LEFTCTRL,
    },
];

pub const KEYCODE_TABLE: [KeyName; 256] = {
    let names = [
        "",
        "esc",
        "1",
        "2",
        "3",
        "4",
        "5",
        "6",
        "7",
        "8",
        "9",
        "0",
        "-",
        "=",
        "backspace",
        "tab",
        "q",
        "w",
        "e",
        "r",
        "t",
        "y",
        "u",
        "i",
        "o",
        "p",
        "[",
        "]",
        "enter",
        "leftcontrol",
        "a",
        "s",
        "d",
        "f",
        "g",
        "h",
        "j",
        "k",
        "l",
        ";",
        "'",
        "`",
        "leftshift",
        "\\",
        "z",
        "x",
        "c",
        "v",
        "b",
        "n",
        "m",
        ",",
        ".",
        "/",
        "rightshift",
        "kpasterisk",
        "leftalt",
        "space",
        "capslock",
        "f1",
        "f2",
        "f3",
        "f4",
        "f5",
        "f6",
        "f7",
        "f8",
        "f9",
        "f10",
        "numlock",
        "scrolllock",
        "kp7",
        "kp8",
        "kp9",
        "kpminus",
        "kp4",
        "kp5",
        "kp6",
        "kpplus",
        "kp1",
        "kp2",
        "kp3",
        "kp0",
        "kpdot",
        "iso-level3-shift",
        "zenkakuhankaku",
        "102nd",
        "f11",
        "f12",
        "ro",
        "katakana",
        "hiragana",
        "henkan",
        "katakanahiragana",
        "muhenkan",
        "kpjpcomma",
        "kpenter",
        "rightcontrol",
        "kpslash",
        "sysrq",
        "rightalt",
        "linefeed",
        "home",
        "up",
        "pageup",
        "left",
        "right",
        "end",
        "down",
        "pagedown",
        "insert",
        "delete",
        "macro",
        "mute",
        "volumedown",
        "volumeup",
        "power",
        "kpequal",
        "kpplusminus",
        "pause",
        "scale",
        "kpcomma",
        "hangeul",
        "hanja",
        "yen",
        "leftmeta",
        "rightmeta",
        "compose",
        "stop",
        "again",
        "props",
        "undo",
        "front",
        "copy",
        "open",
        "paste",
        "find",
        "cut",
        "help",
        "menu",
        "calc",
        "setup",
        "sleep",
        "wakeup",
        "file",
        "sendfile",
        "deletefile",
        "xfer",
        "scrolldown",
        "scrollup",
        "www",
        "msdos",
        "coffee",
        "display",
        "cyclewindows",
        "mail",
        "favorites",
        "computer",
        "back",
        "forward",
        "closecd",
        "ejectcd",
        "ejectclosecd",
        "nextsong",
        "playpause",
        "previoussong",
        "stopcd",
        "record",
        "rewind",
        "phone",
        "iso",
        "config",
        "homepage",
        "refresh",
        "exit",
        "move",
        "edit",
        "zoom",
        "mouseback",
        "kpleftparen",
        "kprightparen",
        "new",
        "redo",
        "f13",
        "f14",
        "f15",
        "f16",
        "f17",
        "f18",
        "f19",
        "f20",
        "f21",
        "f22",
        "f23",
        "f24",
        "noop",
        "",
        "",
        "",
        "",
        "playcd",
        "pausecd",
        "scrollleft",
        "scrollright",
        "dashboard",
        "suspend",
        "close",
        "play",
        "fastforward",
        "bassboost",
        "print",
        "hp",
        "camera",
        "sound",
        "question",
        "email",
        "chat",
        "search",
        "connect",
        "finance",
        "sport",
        "shop",
        "voicecommand",
        "cancel",
        "brightnessdown",
        "brightnessup",
        "media",
        "switchvideomode",
        "kbdillumtoggle",
        "kbdillumdown",
        "kbdillumup",
        "send",
        "reply",
        "forwardmail",
        "save",
        "documents",
        "battery",
        "bluetooth",
        "wlan",
        "uwb",
        "unknown",
        "next",
        "prev",
        "cycle",
        "auto",
        "off",
        "wwan",
        "rfkill",
        "micmute",
        "leftmouse",
        "middlemouse",
        "rightmouse",
        "mouse1",
        "mouse2",
        "fn",
        "mouseforward",
    ];
    let mut table = [KeyName {
        name: None,
        alt_name: None,
        shifted_name: None,
    }; 256];
    let mut i = 0;
    while i < 256 {
        if !names[i].is_empty() {
            table[i].name = Some(names[i]);
        }
        i += 1;
    }
    let aliases = [
        (KEYD_ESC, "escape"),
        (KEYD_MINUS, "minus"),
        (KEYD_EQUAL, "equal"),
        (KEYD_LEFTBRACE, "leftbrace"),
        (KEYD_RIGHTBRACE, "rightbrace"),
        (KEYD_LEFTCTRL, ""),
        (KEYD_SEMICOLON, "semicolon"),
        (KEYD_APOSTROPHE, "apostrophe"),
        (KEYD_GRAVE, "grave"),
        (KEYD_LEFTSHIFT, ""),
        (KEYD_BACKSLASH, "backslash"),
        (KEYD_COMMA, "comma"),
        (KEYD_DOT, "dot"),
        (KEYD_SLASH, "slash"),
        (KEYD_LEFTALT, ""),
        (KEYD_LEFTMETA, ""),
        (KEYD_BOOKMARKS, "bookmarks"),
        (KEYD_F21, "prog1"),
        (KEYD_F22, "prog2"),
        (KEYD_F23, "prog3"),
        (KEYD_F24, "prog4"),
    ];
    i = 0;
    while i < aliases.len() {
        table[aliases[i].0 as usize].alt_name = Some(aliases[i].1);
        i += 1;
    }
    let shifted = [
        (KEYD_1, "!"),
        (KEYD_2, "@"),
        (KEYD_3, "#"),
        (KEYD_4, "$"),
        (KEYD_5, "%"),
        (KEYD_6, "^"),
        (KEYD_7, "&"),
        (KEYD_8, "*"),
        (KEYD_9, "("),
        (KEYD_0, ")"),
        (KEYD_MINUS, "_"),
        (KEYD_EQUAL, "+"),
        (KEYD_Q, "Q"),
        (KEYD_W, "W"),
        (KEYD_E, "E"),
        (KEYD_R, "R"),
        (KEYD_T, "T"),
        (KEYD_Y, "Y"),
        (KEYD_U, "U"),
        (KEYD_I, "I"),
        (KEYD_O, "O"),
        (KEYD_P, "P"),
        (KEYD_LEFTBRACE, "{"),
        (KEYD_RIGHTBRACE, "}"),
        (KEYD_A, "A"),
        (KEYD_S, "S"),
        (KEYD_D, "D"),
        (KEYD_F, "F"),
        (KEYD_G, "G"),
        (KEYD_H, "H"),
        (KEYD_J, "J"),
        (KEYD_K, "K"),
        (KEYD_L, "L"),
        (KEYD_SEMICOLON, ":"),
        (KEYD_APOSTROPHE, "\""),
        (KEYD_GRAVE, "~"),
        (KEYD_BACKSLASH, "|"),
        (KEYD_Z, "Z"),
        (KEYD_X, "X"),
        (KEYD_C, "C"),
        (KEYD_V, "V"),
        (KEYD_B, "B"),
        (KEYD_N, "N"),
        (KEYD_M, "M"),
        (KEYD_COMMA, "<"),
        (KEYD_DOT, ">"),
        (KEYD_SLASH, "?"),
    ];
    i = 0;
    while i < shifted.len() {
        table[shifted[i].0 as usize].shifted_name = Some(shifted[i].1);
        i += 1;
    }
    table
};

pub fn key_name(code: u8) -> &'static str {
    KEYCODE_TABLE[code as usize].name.unwrap_or("UNKNOWN")
}

/// The C helper returns a mutable static buffer. Rust returns owned text.
pub fn modstring(mods: u8) -> String {
    let mut out = String::new();
    for (mask, name) in [
        (MOD_CTRL, 'C'),
        (MOD_SUPER, 'M'),
        (MOD_ALT_GR, 'G'),
        (MOD_SHIFT, 'S'),
        (MOD_ALT, 'A'),
    ] {
        if mods & mask != 0 {
            if !out.is_empty() {
                out.push('-');
            }
            out.push(name);
        }
    }
    out
}

pub fn parse_modset(s: &str) -> Option<u8> {
    let s = s.split('\0').next().unwrap();
    let mut bytes = s.as_bytes();
    let mut mods = 0;
    while let Some(&first) = bytes.first() {
        mods |= match first {
            b'C' => MOD_CTRL,
            b'M' => MOD_SUPER,
            b'A' => MOD_ALT,
            b'S' => MOD_SHIFT,
            b'G' => MOD_ALT_GR,
            _ => return None,
        };
        if bytes.len() == 1 {
            return Some(mods);
        }
        if bytes[1] != b'-' {
            return None;
        }
        bytes = &bytes[2..];
    }
    Some(mods)
}

pub fn parse_key_sequence(s: &str) -> Option<(u8, u8)> {
    let mut s = s.split('\0').next().unwrap();
    if s.is_empty() {
        return None;
    }
    let mut mods = 0;
    while s.as_bytes().get(1) == Some(&b'-') {
        mods |= parse_modset(&s[..1])?;
        s = &s[2..];
    }
    for (code, ent) in KEYCODE_TABLE.iter().enumerate() {
        if ent.name.is_some() {
            if ent.shifted_name == Some(s) {
                return Some((code as u8, mods | MOD_SHIFT));
            }
            if ent.name == Some(s) || ent.alt_name == Some(s) {
                return Some((code as u8, mods));
            }
        }
    }
    None
}
