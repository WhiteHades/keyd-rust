// keyd - A key remapping daemon.
// © 2019 Raheman Vaiya (see also: LICENSE). Ported from frozen f564288.

use crate::keys::*;
use crate::linux::{self, InputEvent, InputId, KEY_MASK_SIZE, KEY_MAX};
use crate::runtime::{shutdown_signal, sleep_us_once};
use crate::{dbg, log};
use std::ffi::OsString;
use std::io;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

pub const CAP_MOUSE: u8 = 0x1;
pub const CAP_MOUSE_ABS: u8 = 0x2;
pub const CAP_KEYBOARD: u8 = 0x4;
pub const CAP_KEY: u8 = 0x8;
pub const MAX_DEVICES: usize = 64;

pub fn resolve_device_capabilities(
    keymask: &[u8; KEY_MASK_SIZE],
    relmask: u8,
    absmask: u8,
) -> (u8, u32) {
    let has_key = |code: usize| (keymask[code / 8] >> (code % 8)) & 1 != 0;
    let num_keys = keymask.iter().map(|b| b.count_ones()).sum();
    let mut capabilities = 0;
    if relmask != 0 || absmask != 0 {
        capabilities |= CAP_MOUSE;
    }
    if absmask != 0 {
        capabilities |= CAP_MOUSE_ABS;
    }
    if num_keys != 0 {
        capabilities |= CAP_KEY;
    }
    let media_keys = [225, 115, 0x212, 0x214, 248];
    let keyboard_keys = [2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21];
    if keyboard_keys.into_iter().all(has_key) || media_keys.into_iter().any(has_key) {
        capabilities |= CAP_KEYBOARD;
    }
    (capabilities, num_keys)
}

pub fn generate_uid(num_keys: u32, absmask: u8, relmask: u8, name: &[u8]) -> u32 {
    let mut hash = 5183_u32;
    for byte in num_keys.to_be_bytes().into_iter().chain([absmask, relmask]) {
        hash = hash.wrapping_mul(33).wrapping_add(u32::from(byte));
    }
    for &byte in name.iter().take_while(|&&b| b != 0) {
        // C plain char is signed on x86_64, unsigned on Linux aarch64.
        let value = byte as std::ffi::c_char as u32;
        hash = hash.wrapping_mul(33).wrapping_add(value);
    }
    hash
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceMetadata {
    pub id: String,
    pub name: OsString,
    pub path: PathBuf,
    pub capabilities: u8,
    pub is_virtual: bool,
    /// C stores these as unsigned values: (minimum, maximum) for X and Y.
    pub abs_bounds: [(u32, u32); 2],
}

impl DeviceMetadata {
    pub fn new(
        path: &Path,
        name: &[u8],
        info: InputId,
        keymask: &[u8; KEY_MASK_SIZE],
        relmask: u8,
        absmask: u8,
        abs_bounds: [(u32, u32); 2],
    ) -> Self {
        let name_end = name
            .iter()
            .position(|&b| b == 0)
            .unwrap_or(name.len())
            .min(63);
        let name = &name[..name_end];
        let (capabilities, num_keys) = resolve_device_capabilities(keymask, relmask, absmask);
        let uid = generate_uid(num_keys, absmask, relmask, name);
        let path_bytes = path.as_os_str().as_bytes();
        Self {
            id: format!("{:04x}:{:04x}:{:08x}", info.vendor, info.product, uid),
            name: OsString::from_vec(name.to_vec()),
            path: PathBuf::from(OsString::from_vec(
                path_bytes[..path_bytes.len().min(255)].to_vec(),
            )),
            capabilities,
            is_virtual: info.vendor == 0x0fac,
            abs_bounds,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceEvent {
    Key { code: u8, pressed: u8 },
    Led { code: u8, pressed: u8 },
    MouseMove { x: i32, y: i32 },
    MouseMoveAbs { x: i32, y: i32 },
    MouseScroll { x: i32, y: i32 },
    Removed,
}

#[derive(Clone, Debug, Default)]
pub struct EventTranslator {
    abs_bounds: [(u32, u32); 2],
    pending_rel: [u32; 2],
}

impl EventTranslator {
    pub fn new(abs_bounds: [(u32, u32); 2]) -> Self {
        Self {
            abs_bounds,
            pending_rel: [0; 2],
        }
    }

    pub fn translate(&mut self, ev: InputEvent) -> io::Result<Option<DeviceEvent>> {
        use DeviceEvent::*;
        use linux::*;
        let event = match ev.kind {
            EV_REL => match ev.code {
                REL_WHEEL => MouseScroll { x: 0, y: ev.value },
                REL_HWHEEL => MouseScroll { x: ev.value, y: 0 },
                REL_X | REL_Y => {
                    let axis = ev.code as usize;
                    self.pending_rel[axis] = self.pending_rel[axis].wrapping_add(ev.value as u32);
                    return Ok(None);
                }
                _ => {
                    if log::level() >= 1 {
                        log::debug_stdout(
                            1,
                            file!(),
                            line!(),
                            "Unrecognized EV_REL code: %s\n",
                            &[ev.code.to_string().as_bytes()],
                        )?;
                    }
                    return Ok(None);
                }
            },
            EV_SYN => {
                if self.pending_rel == [0; 2] {
                    return Ok(None);
                }
                let event = MouseMove {
                    x: self.pending_rel[0] as i32,
                    y: self.pending_rel[1] as i32,
                };
                self.pending_rel = [0; 2];
                event
            }
            EV_ABS => {
                if ev.code != ABS_X && ev.code != ABS_Y {
                    if log::level() >= 1 {
                        log::debug_stdout(
                            1,
                            file!(),
                            line!(),
                            "Unrecognized EV_ABS code: %s",
                            &[format!("{:x}", ev.code).as_bytes()],
                        )?;
                    }
                    return Ok(None);
                }
                let (minimum, maximum) = self.abs_bounds[ev.code as usize];
                let span = maximum.wrapping_sub(minimum);
                if span == 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "zero absolute axis range",
                    ));
                }
                // Preserve C's unsigned division and its lack of minimum subtraction.
                let value = (ev.value.wrapping_mul(1024) as u32 / span) as i32;
                if ev.code == ABS_X {
                    MouseMoveAbs { x: value, y: 0 }
                } else {
                    MouseMoveAbs { x: 0, y: value }
                }
            }
            EV_KEY => {
                if ev.value == 2 {
                    return Ok(None);
                }
                let Some(code) = translate_keycode(ev.code) else {
                    if log::level() >= 1 {
                        log::debug_stdout(
                            1,
                            file!(),
                            line!(),
                            "unsupported evdev code: 0x%s\n",
                            &[format!("{:x}", ev.code).as_bytes()],
                        )?;
                    }
                    return Ok(None);
                };
                if log::level() >= 2 {
                    log::debug_stdout(
                        2,
                        file!(),
                        line!(),
                        "key %s %s",
                        &[
                            key_name(code).as_bytes(),
                            if ev.value as u8 != 0 { b"down" } else { b"up" },
                        ],
                    )?;
                }
                Key {
                    code,
                    pressed: ev.value as u8,
                }
            }
            EV_LED => Led {
                code: ev.code as u8,
                pressed: ev.value as u8,
            },
            _ => {
                if ev.kind != 0 && log::level() >= 2 {
                    log::debug_stdout(
                        2,
                        file!(),
                        line!(),
                        "unrecognized evdev event type: %s %s %s",
                        &[
                            ev.kind.to_string().as_bytes(),
                            ev.code.to_string().as_bytes(),
                            ev.value.to_string().as_bytes(),
                        ],
                    )?;
                }
                return Ok(None);
            }
        };
        Ok(Some(event))
    }
}

pub fn translate_keycode(code: u16) -> Option<u8> {
    if code < 256 {
        return Some(code as u8);
    }
    Some(match code {
        0x1d2..=0x1dd => KEYD_F13 + (code - 0x1d2) as u8,
        // PROG1..4 are below 256 and never reach the C switch.
        0x212 | 0x1bc => KEYD_F21,
        0x16c => KEYD_BOOKMARKS,
        0x1bd => KEYD_F22,
        0x1be | 0x1bf | 0x24e => KEYD_F23,
        0x1e5 => KEYD_F13,
        0x176 => KEYD_F14,
        0x232 => KEYD_F15,
        0x27a => KEYD_F16,
        0x214 => KEYD_F17,
        0x213 => KEYD_F18,
        0x168 => KEYD_F19,
        0x2b8..=0x2bc => KEYD_F20 + (code - 0x2b8) as u8,
        0x1a6..=0x1ae => KEYD_F13 + (code - 0x1a6) as u8,
        0x1d0 => KEYD_FN,
        0x174 => KEYD_ZOOM,
        0x246 => KEYD_VOICECOMMAND,
        0x110 => KEYD_LEFT_MOUSE,
        0x112 => KEYD_MIDDLE_MOUSE,
        0x111 => KEYD_RIGHT_MOUSE,
        0x113 => KEYD_MOUSE_1,
        0x114 => KEYD_MOUSE_2,
        0x116 => KEYD_MOUSE_BACK,
        0x115 => KEYD_MOUSE_FORWARD,
        0x117 => KEYD_F18,
        0x100..=0x109 => KEYD_F13 + (code - 0x100) as u8,
        _ => return None,
    })
}

#[derive(Debug)]
pub struct Device {
    fd: Option<OwnedFd>,
    pub metadata: DeviceMetadata,
    grabbed: bool,
    translator: EventTranslator,
}

impl Device {
    pub fn open(path: &Path) -> io::Result<Self> {
        let fd = match linux::open(path, linux::O_RDWR | linux::O_NONBLOCK) {
            Ok(fd) => fd,
            Err(error) => {
                log::stdout(0, "failed to open %s\n", &[path.as_os_str().as_bytes()])?;
                return Err(error);
            }
        };
        if log::level() >= 2 {
            // Keep the complete multiline record together during parallel scans.
            let stdout = io::stdout();
            let _lock = stdout.lock();
            dbg::print_evdev_details(
                path,
                log::level(),
                &mut crate::runtime::FdIo::new(stdout.as_fd())?,
            )?;
        }
        let mut keymask = [0; KEY_MASK_SIZE];
        let mut relmask = [0];
        let mut absmask = [0];
        let capabilities = match (|| {
            linux::evdev_bits(fd.as_fd(), linux::EV_KEY, &mut keymask)?;
            linux::evdev_bits(fd.as_fd(), linux::EV_ABS, &mut absmask)?;
            linux::evdev_bits(fd.as_fd(), linux::EV_REL, &mut relmask)?;
            Ok::<_, io::Error>(resolve_device_capabilities(&keymask, relmask[0], absmask[0]).0)
        })() {
            Ok(capabilities) => capabilities,
            Err(error) => {
                log::perror("ioctl", error)?;
                0
            }
        };
        let name = match linux::evdev_name(fd.as_fd()) {
            Ok(name) => name,
            Err(error) => {
                // C reports its still-empty dev->path at this point.
                log::stdout(0, "ERROR: could not fetch device name of \n", &[])?;
                return Err(error);
            }
        };
        let mut bounds = [(0, 0); 2];
        if capabilities & CAP_MOUSE_ABS != 0 {
            bounds[0] = linux::evdev_abs_bounds(fd.as_fd(), linux::ABS_X)
                .map_err(|error| ioctl_failure("ioctl", error))?;
            bounds[1] = linux::evdev_abs_bounds(fd.as_fd(), linux::ABS_Y)
                .map_err(|error| ioctl_failure("ioctl", error))?;
        }
        if log::level() >= 1 {
            let end = name
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(name.len());
            log::debug_stdout(
                1,
                file!(),
                line!(),
                "capabilities of %s (%s): %s",
                &[
                    path.as_os_str().as_bytes(),
                    &name[..end],
                    format!("{capabilities:x}").as_bytes(),
                ],
            )?;
        }
        if capabilities == 0 {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "device has no supported capabilities",
            ));
        }
        let info =
            linux::evdev_id(fd.as_fd()).map_err(|error| ioctl_failure("ioctl EVIOCGID", error))?;
        let metadata =
            DeviceMetadata::new(path, &name, info, &keymask, relmask[0], absmask[0], bounds);
        Ok(Self::from_fd(fd, metadata))
    }

    /// Takes ownership of an already open, nonblocking evdev descriptor.
    pub fn from_fd(fd: OwnedFd, metadata: DeviceMetadata) -> Self {
        let translator = EventTranslator::new(metadata.abs_bounds);
        Self {
            fd: Some(fd),
            metadata,
            grabbed: false,
            translator,
        }
    }

    pub fn fd(&self) -> Option<BorrowedFd<'_>> {
        self.fd.as_ref().map(AsFd::as_fd)
    }
    pub fn grabbed(&self) -> bool {
        self.grabbed
    }

    pub fn grab(&mut self) -> io::Result<()> {
        if self.grabbed {
            return Ok(());
        }
        let fd = self.fd().ok_or(io::ErrorKind::NotConnected)?;
        let mut pending_release = false;
        loop {
            if shutdown_signal() != 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let mut state = [0; KEY_MASK_SIZE];
            linux::evdev_key_state(fd, &mut state)
                .map_err(|error| ioctl_failure("ioctl EVIOCGKEY", error))?;
            if !(0..KEY_MAX).any(|i| (state[i / 8] >> (i % 8)) & 1 != 0) {
                break;
            }
            pending_release = true;
        }
        if shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if pending_release {
            let _ = sleep_us_once(100);
        }
        if shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        linux::evdev_grab(fd, true).map_err(|error| ioctl_failure("EVIOCGRAB", error))?;
        let mut bytes = [0; InputEvent::SIZE];
        while shutdown_signal() == 0 && matches!(linux::read(fd, &mut bytes), Ok(n) if n > 0) {}
        self.grabbed = true;
        if shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        Ok(())
    }

    pub fn ungrab(&mut self) -> io::Result<()> {
        if !self.grabbed {
            return Ok(());
        }
        linux::evdev_grab(self.fd().ok_or(io::ErrorKind::NotConnected)?, false)?;
        self.grabbed = false;
        Ok(())
    }

    pub fn read_event(&mut self) -> io::Result<Option<DeviceEvent>> {
        let Some(fd) = self.fd() else {
            return Ok(None);
        };
        let mut bytes = [0; InputEvent::SIZE];
        match linux::read(fd, &mut bytes) {
            Ok(n) if n == bytes.len() => self.translator.translate(InputEvent::decode(&bytes)?),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            // C emits DEV_REMOVED on every non-EAGAIN error; EOF/short records
            // are also terminal here rather than exposing uninitialized data.
            _ => {
                self.fd.take();
                self.grabbed = false;
                Ok(Some(DeviceEvent::Removed))
            }
        }
    }

    pub fn set_led(&self, led: u16, state: i32) -> io::Result<()> {
        let fd = self.fd().ok_or(io::ErrorKind::NotConnected)?;
        linux::write_all(fd, &InputEvent::new(linux::EV_LED, led, state).encode())
            .map_err(|error| log::system_error("write", error))
    }
}

pub fn device_scan() -> io::Result<Vec<Device>> {
    device_scan_at(Path::new("/dev/input"))
}

pub fn device_scan_at(directory: &Path) -> io::Result<Vec<Device>> {
    let mut paths = Vec::new();
    for entry in
        std::fs::read_dir(directory).map_err(|error| log::system_error("opendir", error))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() && entry.file_name().as_bytes().starts_with(b"event") {
            if paths.len() == MAX_DEVICES {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "maximum devices exceeded",
                ));
            }
            paths.push(entry.path());
        }
    }
    // Retain the C scan's parallel initialization and directory-order results.
    std::thread::scope(|scope| {
        let mut workers = Vec::new();
        for path in paths {
            workers.push(
                std::thread::Builder::new()
                    .spawn_scoped(scope, move || initialize_device(&path))?,
            );
        }
        let mut devices = Vec::new();
        for worker in workers {
            if let Some(device) = worker
                .join()
                .map_err(|_| io::Error::other("device scan worker panicked"))??
            {
                devices.push(device);
            }
        }
        Ok(devices)
    })
}

/// Parses one complete inotify record. Only IN_CREATE event* names qualify.
pub fn parse_monitor_record(bytes: &[u8]) -> io::Result<(usize, Option<OsString>)> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidData, "invalid inotify record");
    if bytes.len() < 16 {
        return Err(invalid());
    }
    let mask = u32::from_ne_bytes(bytes[4..8].try_into().unwrap());
    let len = u32::from_ne_bytes(bytes[12..16].try_into().unwrap()) as usize;
    let size = 16_usize.checked_add(len).ok_or_else(invalid)?;
    let name = bytes.get(16..size).ok_or_else(invalid)?;
    if mask & linux::IN_CREATE == 0 || name.is_empty() {
        return Ok((size, None));
    }
    let end = name.iter().position(|&b| b == 0).ok_or_else(invalid)?;
    let name = &name[..end];
    if name.contains(&b'/') {
        return Err(invalid());
    }
    let event = name
        .starts_with(b"event")
        .then(|| OsString::from_vec(name.to_vec()));
    Ok((size, event))
}

#[derive(Debug)]
pub struct DeviceMonitor {
    fd: OwnedFd,
    directory: PathBuf,
    buffer: [u8; 4096],
    position: usize,
    length: usize,
}

impl DeviceMonitor {
    pub fn create() -> io::Result<Self> {
        Self::watch(Path::new("/dev/input"))
    }

    pub fn watch(directory: &Path) -> io::Result<Self> {
        let fd = linux::inotify_create().map_err(|error| log::system_error("inotify", error))?;
        linux::inotify_watch(fd.as_fd(), &linux::path_cstring(directory)?)
            .map_err(|error| log::system_error("inotify", error))?;
        Ok(Self {
            fd,
            directory: directory.to_path_buf(),
            buffer: [0; 4096],
            position: 0,
            length: 0,
        })
    }

    pub fn fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }

    /// Returns the next matching newly initialized device, without blocking.
    pub fn read_device(&mut self) -> io::Result<Option<Device>> {
        while let Some(path) = self.read_path()? {
            if let Some(device) = initialize_device(&path)? {
                return Ok(Some(device));
            }
        }
        Ok(None)
    }

    /// Returns the next event* creation path, retaining unread inotify records.
    pub fn read_path(&mut self) -> io::Result<Option<PathBuf>> {
        loop {
            if self.position == self.length {
                self.position = 0;
                self.length = 0;
                self.length = match linux::read(self.fd.as_fd(), &mut self.buffer) {
                    Ok(n) => n,
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(None),
                    Err(e) => return Err(e),
                };
                if self.length == 0 {
                    return Ok(None);
                }
            }
            let (size, name) = parse_monitor_record(&self.buffer[self.position..self.length])?;
            self.position += size;
            if let Some(name) = name {
                return Ok(Some(self.directory.join(name)));
            }
        }
    }
}

fn initialize_device(path: &Path) -> io::Result<Option<Device>> {
    match Device::open(path) {
        Ok(device) => Ok(Some(device)),
        Err(error) if dbg::is_fatal_ioctl(&error) || shutdown_signal() != 0 => Err(error),
        Err(_) => Ok(None),
    }
}

fn ioctl_failure(label: &'static str, error: io::Error) -> io::Error {
    let diagnostic = error.raw_os_error().map_or_else(
        || io::Error::new(error.kind(), error.to_string()),
        io::Error::from_raw_os_error,
    );
    log::perror(label, diagnostic).err().unwrap_or(error)
}
