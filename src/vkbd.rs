// keyd - A key remapping daemon.
// © 2019 Raheman Vaiya; © 2021 Giorgi Chavchanidze (see also: LICENSE).
// Ported from frozen f564288, including stdout and USB gadget backends.

use crate::keys::*;
use crate::linux::{self, InputEvent, UinputBit};
use crate::log;
use crate::runtime::{shutdown_signal, sleep_us_once};
use std::io::{self, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BackendKind {
    Uinput,
    Stdout,
    UsbGadget,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputTarget {
    Keyboard,
    Pointer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PointerAction {
    Move,
    MoveAbs,
    Scroll,
}

pub fn key_events(code: u8, state: i32) -> (OutputTarget, [InputEvent; 2]) {
    let (target, code) = match code {
        KEYD_LEFT_MOUSE => (OutputTarget::Pointer, 0x110),
        KEYD_MIDDLE_MOUSE => (OutputTarget::Pointer, 0x112),
        KEYD_RIGHT_MOUSE => (OutputTarget::Pointer, 0x111),
        KEYD_MOUSE_1 => (OutputTarget::Pointer, 0x113),
        KEYD_MOUSE_2 => (OutputTarget::Pointer, 0x114),
        KEYD_MOUSE_BACK => (OutputTarget::Pointer, 0x116),
        KEYD_MOUSE_FORWARD => (OutputTarget::Pointer, 0x115),
        KEYD_ZOOM => (OutputTarget::Keyboard, 0x174),
        KEYD_VOICECOMMAND => (OutputTarget::Keyboard, 0x246),
        _ => (OutputTarget::Keyboard, u16::from(code)),
    };
    (
        target,
        [
            InputEvent::new(linux::EV_KEY, code, state),
            InputEvent::new(linux::EV_SYN, 0, 0),
        ],
    )
}

pub fn pointer_events(action: PointerAction, x: i32, y: i32) -> Vec<InputEvent> {
    let mut events = Vec::with_capacity(3);
    match action {
        PointerAction::Move | PointerAction::MoveAbs => {
            let kind = if action == PointerAction::Move {
                linux::EV_REL
            } else {
                linux::EV_ABS
            };
            // C intentionally omits zero axes, including absolute coordinate zero.
            if x != 0 {
                events.push(InputEvent::new(kind, 0, x));
            }
            if y != 0 {
                events.push(InputEvent::new(kind, 1, y));
            }
        }
        PointerAction::Scroll => {
            events.push(InputEvent::new(linux::EV_REL, linux::REL_WHEEL, y));
            events.push(InputEvent::new(linux::EV_REL, linux::REL_HWHEEL, x));
        }
    }
    events.push(InputEvent::new(linux::EV_SYN, 0, 0));
    events
}

pub const UINPUT_USER_DEV_SIZE: usize = 1116;

/// Legacy uinput_user_dev encoding, used by C to support older Linux kernels.
pub fn uinput_descriptor(name: &[u8], pointer: bool) -> [u8; UINPUT_USER_DEV_SIZE] {
    let mut bytes = [0; UINPUT_USER_DEV_SIZE];
    let name_len = name
        .iter()
        .position(|&b| b == 0)
        .unwrap_or(name.len())
        .min(79);
    bytes[..name_len].copy_from_slice(&name[..name_len]);
    bytes[80..82].copy_from_slice(&3_u16.to_ne_bytes()); // BUS_USB
    bytes[82..84].copy_from_slice(&0x0fac_u16.to_ne_bytes());
    bytes[84..86].copy_from_slice(&(if pointer { 0x1ade_u16 } else { 0x0ade_u16 }).to_ne_bytes());
    if pointer {
        bytes[92..96].copy_from_slice(&1024_i32.to_ne_bytes());
        bytes[96..100].copy_from_slice(&1024_i32.to_ne_bytes());
    }
    bytes
}

fn create_uinput(name: &[u8], pointer: bool) -> io::Result<OwnedFd> {
    check_shutdown()?;
    let fd = linux::open(
        Path::new("/dev/uinput"),
        linux::O_WRONLY | linux::O_NONBLOCK,
    )
    .map_err(|error| output_error(if pointer { "open" } else { "open uinput" }, error))?;
    let events: &[u16] = if pointer {
        &[linux::EV_REL, linux::EV_ABS, linux::EV_KEY, linux::EV_SYN]
    } else {
        &[linux::EV_REP, linux::EV_KEY, linux::EV_LED, linux::EV_SYN]
    };
    for &event in events {
        check_shutdown()?;
        let result = linux::uinput_set_bit(fd.as_fd(), UinputBit::Event, event);
        if pointer {
            best_effort_output(result)?;
        } else {
            result.map_err(|error| output_error("ioctl set_evbit", error))?;
        }
    }
    if pointer {
        // C ignores pointer setup ioctl failures, but shutdown must still cancel.
        for code in [linux::ABS_X, linux::ABS_Y] {
            check_shutdown()?;
            best_effort_output(linux::uinput_set_bit(fd.as_fd(), UinputBit::Absolute, code))?;
        }
        for code in [
            linux::REL_X,
            linux::REL_WHEEL,
            linux::REL_HWHEEL,
            linux::REL_Y,
            linux::REL_Z,
        ] {
            check_shutdown()?;
            best_effort_output(linux::uinput_set_bit(fd.as_fd(), UinputBit::Relative, code))?;
        }
        for code in 0x110..=0x117 {
            check_shutdown()?;
            best_effort_output(linux::uinput_set_bit(fd.as_fd(), UinputBit::Key, code))?;
        }
    } else {
        for (code, entry) in KEYCODE_TABLE.iter().enumerate() {
            if entry.name.is_some() {
                check_shutdown()?;
                linux::uinput_set_bit(fd.as_fd(), UinputBit::Key, code as u16)
                    .map_err(|error| output_error("ioctl set_keybit", error))?;
            }
        }
        for code in 0..=8 {
            check_shutdown()?;
            linux::uinput_set_bit(fd.as_fd(), UinputBit::Led, code)
                .map_err(|error| output_error("ioctl set_ledbit", error))?;
        }
        check_shutdown()?;
        linux::uinput_set_bit(fd.as_fd(), UinputBit::Key, 0x174)
            .map_err(|error| output_error("ioctl set_keybit", error))?;
    }
    check_shutdown()?;
    // C attempts one descriptor write and accepts every nonnegative result.
    if linux::write(fd.as_fd(), &uinput_descriptor(name, pointer)).is_err() {
        check_shutdown()?;
        return Err(io::Error::other("failed to create uinput device"));
    }
    check_shutdown()?;
    let result = linux::uinput_create(fd.as_fd());
    if pointer {
        best_effort_output(result)?;
    } else {
        result.map_err(|error| output_error("ioctl dev_create", error))?;
    }
    check_shutdown()?;
    Ok(fd)
}

pub fn stdout_key(code: u8, state: i32) -> String {
    // glibc prints a NULL name as (null); unknown table entries remain unnamed.
    format!(
        "key: {}, state: {state}\n",
        KEYCODE_TABLE[code as usize].name.unwrap_or("(null)")
    )
}

pub fn stdout_pointer(action: PointerAction, x: i32, y: i32) -> String {
    let label = match action {
        PointerAction::Move => "mouse movement",
        PointerAction::MoveAbs => "absolute mouse movement",
        PointerAction::Scroll => "mouse scroll",
    };
    format!("{label}: x: {x}, y: {y}\n")
}

/// The C gadget uses a six-key boot-keyboard report, not NKRO.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HidState {
    modifiers: u8,
    keys: [u8; 6],
}

impl HidState {
    pub fn send_key(&mut self, code: u8, state: i32) -> [u8; 8] {
        let modifier = match code {
            KEYD_LEFTCTRL => 1,
            KEYD_LEFTSHIFT => 2,
            KEYD_LEFTALT => 4,
            KEYD_LEFTMETA => 8,
            KEYD_RIGHTCTRL => 16,
            KEYD_RIGHTSHIFT => 32,
            KEYD_RIGHTALT => 64,
            KEYD_RIGHTMETA => 128,
            _ => 0,
        };
        if modifier != 0 {
            if state != 0 {
                self.modifiers |= modifier;
            } else {
                self.modifiers &= !modifier;
            }
        } else {
            let hid_code = HID_TABLE[code as usize];
            let mut set = false;
            for key in &mut self.keys {
                if *key == hid_code {
                    set = true;
                    if state == 0 {
                        *key = 0;
                    }
                }
            }
            if state != 0
                && !set
                && let Some(slot) = self.keys.iter_mut().find(|key| **key == 0)
            {
                *slot = hid_code;
            }
        }
        let mut report = [0; 8];
        report[0] = self.modifiers;
        report[2..].copy_from_slice(&self.keys);
        // C leaves the reserved byte uninitialized; HID requires it to be zero.
        report
    }
}

pub const HID_TABLE: [u8; 256] = {
    let mut table = [0; 256];
    let entries: &[(u8, u8)] = &[
        (KEYD_ESC, 0x29),
        (KEYD_1, 0x1e),
        (KEYD_2, 0x1f),
        (KEYD_3, 0x20),
        (KEYD_4, 0x21),
        (KEYD_5, 0x22),
        (KEYD_6, 0x23),
        (KEYD_7, 0x24),
        (KEYD_8, 0x25),
        (KEYD_9, 0x26),
        (KEYD_0, 0x27),
        (KEYD_MINUS, 0x2d),
        (KEYD_EQUAL, 0x2e),
        (KEYD_BACKSPACE, 0x2a),
        (KEYD_TAB, 0x2b),
        (KEYD_Q, 0x14),
        (KEYD_W, 0x1a),
        (KEYD_E, 0x08),
        (KEYD_R, 0x15),
        (KEYD_T, 0x17),
        (KEYD_Y, 0x1c),
        (KEYD_U, 0x18),
        (KEYD_I, 0x0c),
        (KEYD_O, 0x12),
        (KEYD_P, 0x13),
        (KEYD_LEFTBRACE, 0x2f),
        (KEYD_RIGHTBRACE, 0x30),
        (KEYD_ENTER, 0x28),
        (KEYD_LEFTCTRL, 0xe0),
        (KEYD_A, 0x04),
        (KEYD_S, 0x16),
        (KEYD_D, 0x07),
        (KEYD_F, 0x09),
        (KEYD_G, 0x0a),
        (KEYD_H, 0x0b),
        (KEYD_J, 0x0d),
        (KEYD_K, 0x0e),
        (KEYD_L, 0x0f),
        (KEYD_SEMICOLON, 0x33),
        (KEYD_APOSTROPHE, 0x34),
        (KEYD_GRAVE, 0x35),
        (KEYD_LEFTSHIFT, 0xe1),
        (KEYD_BACKSLASH, 0x31),
        (KEYD_Z, 0x1d),
        (KEYD_X, 0x1b),
        (KEYD_C, 0x06),
        (KEYD_V, 0x19),
        (KEYD_B, 0x05),
        (KEYD_N, 0x11),
        (KEYD_M, 0x10),
        (KEYD_COMMA, 0x36),
        (KEYD_DOT, 0x37),
        (KEYD_SLASH, 0x38),
        (KEYD_RIGHTSHIFT, 0xe5),
        (KEYD_KPASTERISK, 0x55),
        (KEYD_LEFTALT, 0xe2),
        (KEYD_SPACE, 0x2c),
        (KEYD_CAPSLOCK, 0x39),
        (KEYD_F1, 0x3a),
        (KEYD_F2, 0x3b),
        (KEYD_F3, 0x3c),
        (KEYD_F4, 0x3d),
        (KEYD_F5, 0x3e),
        (KEYD_F6, 0x3f),
        (KEYD_F7, 0x40),
        (KEYD_F8, 0x41),
        (KEYD_F9, 0x42),
        (KEYD_F10, 0x43),
        (KEYD_NUMLOCK, 0x53),
        (KEYD_SCROLLLOCK, 0x47),
        (KEYD_KP7, 0x5f),
        (KEYD_KP8, 0x60),
        (KEYD_KP9, 0x61),
        (KEYD_KPMINUS, 0x56),
        (KEYD_KP4, 0x5c),
        (KEYD_KP5, 0x5d),
        (KEYD_KP6, 0x5e),
        (KEYD_KPPLUS, 0x57),
        (KEYD_KP1, 0x59),
        (KEYD_KP2, 0x5a),
        (KEYD_KP3, 0x5b),
        (KEYD_KP0, 0x62),
        (KEYD_KPDOT, 0x63),
        (KEYD_ZENKAKUHANKAKU, 0x94),
        (KEYD_102ND, 0x64),
        (KEYD_F11, 0x44),
        (KEYD_F12, 0x45),
        (KEYD_RO, 0x87),
        (KEYD_KATAKANA, 0x92),
        (KEYD_HIRAGANA, 0x93),
        (KEYD_HENKAN, 0x8a),
        (KEYD_KATAKANAHIRAGANA, 0x88),
        (KEYD_MUHENKAN, 0x8b),
        (KEYD_KPENTER, 0x58),
        (KEYD_RIGHTCTRL, 0xe4),
        (KEYD_KPSLASH, 0x54),
        (KEYD_SYSRQ, 0x46),
        (KEYD_RIGHTALT, 0xe6),
        (KEYD_HOME, 0x4a),
        (KEYD_UP, 0x52),
        (KEYD_PAGEUP, 0x4b),
        (KEYD_LEFT, 0x50),
        (KEYD_RIGHT, 0x4f),
        (KEYD_END, 0x4d),
        (KEYD_DOWN, 0x51),
        (KEYD_PAGEDOWN, 0x4e),
        (KEYD_INSERT, 0x49),
        (KEYD_DELETE, 0x4c),
        (KEYD_MUTE, 0x7f),
        (KEYD_VOLUMEDOWN, 0x81),
        (KEYD_VOLUMEUP, 0x80),
        (KEYD_POWER, 0x66),
        (KEYD_KPEQUAL, 0x67),
        (KEYD_KPPLUSMINUS, 0xd7),
        (KEYD_PAUSE, 0x48),
        (KEYD_KPCOMMA, 0x85),
        (KEYD_HANGEUL, 0x90),
        (KEYD_HANJA, 0x91),
        (KEYD_YEN, 0x89),
        (KEYD_LEFTMETA, 0xe3),
        (KEYD_RIGHTMETA, 0xe7),
        (KEYD_COMPOSE, 0x65),
        (KEYD_AGAIN, 0x79),
        (KEYD_UNDO, 0x7a),
        (KEYD_FRONT, 0x77),
        (KEYD_COPY, 0x7c),
        (KEYD_OPEN, 0x74),
        (KEYD_PASTE, 0x7d),
        (KEYD_FIND, 0x7e),
        (KEYD_CUT, 0x7b),
        (KEYD_HELP, 0x75),
        (KEYD_KPLEFTPAREN, 0xb6),
        (KEYD_KPRIGHTPAREN, 0xb7),
        (KEYD_F13, 0x68),
        (KEYD_F14, 0x69),
        (KEYD_F15, 0x6a),
        (KEYD_F16, 0x6b),
        (KEYD_F17, 0x6c),
        (KEYD_F18, 0x6d),
        (KEYD_F19, 0x6e),
        (KEYD_F20, 0x6f),
        (KEYD_F21, 0x70),
        (KEYD_F22, 0x71),
        (KEYD_F23, 0x72),
        (KEYD_F24, 0x73),
    ];
    let mut i = 0;
    while i < entries.len() {
        table[entries[i].0 as usize] = entries[i].1;
        i += 1;
    }
    table
};

#[derive(Debug)]
enum Backend {
    Uinput { keyboard: OwnedFd, pointer: OwnedFd },
    Stdout,
    UsbGadget { fd: OwnedFd, state: HidState },
}

#[derive(Debug)]
pub struct VirtualKeyboard {
    backend: Backend,
}

fn check_shutdown() -> io::Result<()> {
    if shutdown_signal() != 0 {
        return Err(io::ErrorKind::Interrupted.into());
    }
    Ok(())
}

fn output_error(label: &'static str, error: io::Error) -> io::Error {
    if shutdown_signal() != 0 {
        io::ErrorKind::Interrupted.into()
    } else {
        log::system_error(label, error)
    }
}

// C ignores pointer setup/report/stdio errors; shutdown still stops pending work.
fn best_effort_output(result: io::Result<()>) -> io::Result<()> {
    if result.is_err() && shutdown_signal() != 0 {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        Ok(())
    }
}

fn write_output(fd: BorrowedFd<'_>, mut bytes: &[u8], cleanup_release: bool) -> io::Result<()> {
    while !bytes.is_empty() {
        if !cleanup_release {
            check_shutdown()?;
        }
        match linux::write(fd, bytes) {
            Ok(0) => {
                check_shutdown()?;
                return Err(io::ErrorKind::WriteZero.into());
            }
            Ok(n) => bytes = &bytes[n..],
            Err(error) => return Err(output_error("write", error)),
        }
    }
    if !cleanup_release {
        check_shutdown()?;
    }
    Ok(())
}

impl VirtualKeyboard {
    pub fn init(name: &str, kind: BackendKind) -> io::Result<Self> {
        let backend = match kind {
            BackendKind::Uinput => Backend::Uinput {
                keyboard: create_uinput(name.as_bytes(), false)?,
                pointer: create_uinput(b"keyd virtual pointer", true)?,
            },
            BackendKind::Stdout => Backend::Stdout,
            BackendKind::UsbGadget => Backend::UsbGadget {
                fd: linux::open(Path::new("/dev/hidg0"), linux::O_WRONLY | linux::O_NONBLOCK)?,
                state: HidState::default(),
            },
        };
        Ok(Self { backend })
    }

    pub fn send_key(&mut self, code: u8, state: i32) -> io::Result<()> {
        // Key releases remain available during shutdown cleanup.
        let cleanup_release = state == 0;
        if !cleanup_release {
            check_shutdown()?;
        }
        match &mut self.backend {
            Backend::Uinput { keyboard, pointer } => {
                if log::level() >= 1 && shutdown_signal() == 0 {
                    best_effort_output(log::debug_stdout(
                        1,
                        file!(),
                        line!(),
                        "output %s %s",
                        &[
                            key_name(code).as_bytes(),
                            if state == 1 { b"down" } else { b"up" },
                        ],
                    ))?;
                }
                let (target, events) = key_events(code, state);
                let fd = match target {
                    OutputTarget::Keyboard => keyboard.as_fd(),
                    OutputTarget::Pointer => {
                        if shutdown_signal() == 0 {
                            let _ = sleep_us_once(1000);
                        }
                        pointer.as_fd()
                    }
                };
                for event in events {
                    write_output(fd, &event.encode(), cleanup_release)?;
                }
                Ok(())
            }
            Backend::Stdout => {
                let stdout = io::stdout();
                let _lock = stdout.lock();
                best_effort_output((|| {
                    let mut output = crate::runtime::FdIo::new(stdout.as_fd())?;
                    if cleanup_release && shutdown_signal() != 0 {
                        // Cleanup must not wait for an unread stdout pipe or socket.
                        return write_output(
                            stdout.as_fd(),
                            stdout_key(code, state).as_bytes(),
                            true,
                        );
                    }
                    output.write_all(stdout_key(code, state).as_bytes())
                })())
            }
            Backend::UsbGadget { fd, state: hid } => {
                let report = hid.send_key(code, state);
                if !cleanup_release {
                    check_shutdown()?;
                }
                // The gadget attempts exactly one write, even for a partial report.
                best_effort_output(linux::write(fd.as_fd(), &report).map(|_| ()))
            }
        }
    }

    pub fn mouse_move(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.pointer(PointerAction::Move, x, y)
    }
    pub fn mouse_move_abs(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.pointer(PointerAction::MoveAbs, x, y)
    }
    pub fn mouse_scroll(&mut self, x: i32, y: i32) -> io::Result<()> {
        self.pointer(PointerAction::Scroll, x, y)
    }

    fn pointer(&mut self, action: PointerAction, x: i32, y: i32) -> io::Result<()> {
        check_shutdown()?;
        match &self.backend {
            Backend::Uinput { pointer, .. } => {
                for event in pointer_events(action, x, y) {
                    write_output(pointer.as_fd(), &event.encode(), false)?;
                }
                Ok(())
            }
            Backend::Stdout => {
                let stdout = io::stdout();
                let _lock = stdout.lock();
                best_effort_output(crate::runtime::FdIo::new(stdout.as_fd()).and_then(
                    |mut output| output.write_all(stdout_pointer(action, x, y).as_bytes()),
                ))
            }
            Backend::UsbGadget { .. } => {
                // Match the baseline: mouse operations warn and emit no HID report.
                let _lock = io::stdout().lock();
                best_effort_output(crate::runtime::FdIo::new(io::stderr().as_fd()).and_then(
                    |mut output| {
                        output.write_all(b"usb-gadget: mouse support is not implemented\n")
                    },
                ))
            }
        }
    }
}
