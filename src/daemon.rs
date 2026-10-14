// Rust port of keyd's daemon.c. © 2019 Raheman Vaiya. See LICENSE.

use crate::config::{self, Config, LayerType};
use crate::device::{self, DeviceEvent};
use crate::evloop::{DeviceSlot, Event, EventKind, EventLoop};
use crate::keyboard::{KeyEvent, Keyboard, LayerState, Output};
use crate::keys::*;
use crate::{cli, ipc, ipc_server, linux, log, macros, runtime, string, unicode, vkbd};
use std::cell::RefCell;
use std::ffi::OsStr;
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

struct LedTarget {
    owner: usize,
    device_fd: RawFd,
    fd: OwnedFd,
}

struct OutputState {
    keyboard: vkbd::VirtualKeyboard,
    keys: [u8; 256],
    listeners: Vec<UnixStream>,
    leds: Vec<LedTarget>,
    error: Option<io::Error>,
}

impl OutputState {
    fn input_key(&mut self, code: u8, state: i32) -> Result<(), Vec<u8>> {
        self.keyboard.send_key(code, state).map_err(|error| {
            let message = error.to_string();
            self.record(Err(error));
            message.into_bytes()
        })
    }

    fn send_key(&mut self, code: u8, state: u8) -> io::Result<()> {
        self.keys[code as usize] = state;
        match code {
            KEYD_SCROLL_DOWN | KEYD_SCROLL_UP | KEYD_SCROLL_RIGHT | KEYD_SCROLL_LEFT => {
                if state == 0 {
                    return Ok(());
                }
                let (x, y) = match code {
                    KEYD_SCROLL_DOWN => (0, -1),
                    KEYD_SCROLL_UP => (0, 1),
                    KEYD_SCROLL_RIGHT => (1, 0),
                    _ => (-1, 0),
                };
                self.keyboard.mouse_scroll(x, y)
            }
            _ => self.keyboard.send_key(code, i32::from(state)),
        }
    }

    fn clear_keys(&mut self) -> io::Result<()> {
        for (code, state) in self.keys.iter_mut().enumerate() {
            if *state != 0 {
                self.keyboard.send_key(code as u8, 0)?;
                *state = 0;
            }
        }
        Ok(())
    }

    fn record(&mut self, result: io::Result<()>) {
        if self.error.is_none() {
            self.error = result.err();
        }
    }
}

struct DaemonOutput {
    shared: Rc<RefCell<OutputState>>,
    owner: usize,
}

impl Output for DaemonOutput {
    fn failed(&self) -> bool {
        self.shared.borrow().error.is_some()
    }

    fn send_key(&mut self, code: u8, state: u8) {
        let mut output = self.shared.borrow_mut();
        if output.error.is_some() || runtime::shutdown_signal() != 0 {
            return;
        }
        let result = output.send_key(code, state);
        output.record(result);
    }

    fn on_layer_change(
        &mut self,
        config: &Config,
        states: &[LayerState],
        index: usize,
        active: bool,
    ) {
        let mut output = self.shared.borrow_mut();
        if output.error.is_some() || runtime::shutdown_signal() != 0 {
            return;
        }
        if config.layer_indicator != 0 {
            let active_layers = config
                .layers
                .iter()
                .zip(states)
                .skip(1)
                .any(|(layer, state)| layer.kind != LayerType::Layout && state.active != 0);
            for target in &output.leds {
                if target.owner == self.owner {
                    let event = linux::InputEvent::new(linux::EV_LED, 1, i32::from(active_layers));
                    if let Err(error) = linux::write_all(target.fd.as_fd(), &event.encode()) {
                        output.record(Err(log::system_error("write", error)));
                        return;
                    }
                }
            }
        }
        let layer = &config.layers[index];
        let prefix = if layer.kind == LayerType::Layout {
            b'/'
        } else if active {
            b'+'
        } else {
            b'-'
        };
        let mut message = vec![prefix];
        message.extend_from_slice(layer.name.as_bytes());
        // C's fixed notification buffer stores NUL instead of the final newline.
        message.push(
            if layer.name.as_bytes().len() == config::MAX_LAYER_NAME_LEN {
                0
            } else {
                b'\n'
            },
        );
        // One write matches C's back-pressure/drop contract, rather than retrying a partial notification.
        output.listeners.retain(|listener| matches!(linux::write(listener.as_fd(), &message), Ok(count) if count == message.len()));
    }

    fn execute_command(&mut self, command: &OsStr) {
        if self.failed() || runtime::shutdown_signal() != 0 {
            return;
        }
        let result = runtime::execute_command(command);
        self.shared.borrow_mut().record(result);
    }
}

struct Daemon {
    configs: Vec<Keyboard<DaemonOutput>>,
    output: Rc<RefCell<OutputState>>,
    directory: PathBuf,
    active: Option<usize>,
    last_time: i32,
    timeout: i32,
}

fn disconnected(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::NotConnected
    )
}

impl Daemon {
    fn new(keyboard: vkbd::VirtualKeyboard, directory: &Path) -> Self {
        Self {
            configs: Vec::new(),
            output: Rc::new(RefCell::new(OutputState {
                keyboard,
                keys: [0; 256],
                listeners: Vec::new(),
                leds: Vec::new(),
                error: None,
            })),
            directory: directory.into(),
            active: None,
            last_time: 0,
            timeout: 0,
        }
    }

    fn check_output(&mut self) -> io::Result<()> {
        if runtime::shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        match self.output.borrow_mut().error.take() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn load_configs(&mut self) -> io::Result<()> {
        let stdout = io::stdout();
        let _lock = stdout.lock();
        let mut stdout = runtime::FdIo::new(stdout.as_fd())?;
        for entry in std::fs::read_dir(&self.directory)
            .map_err(|error| log::system_error("opendir", error))?
        {
            let entry = entry?;
            if entry.file_type()?.is_dir() || !entry.file_name().as_bytes().ends_with(b".conf") {
                continue;
            }
            let path = entry.path();
            log::write(
                &mut stdout,
                0,
                "CONFIG: parsing b{%s}\n",
                &[path.as_os_str().as_bytes()],
            )?;
            match config::config_parse_with_warnings(&path, |warning| {
                cli::warning(warning, &mut stdout)
            }) {
                Ok(config) => {
                    let output = DaemonOutput {
                        shared: self.output.clone(),
                        owner: 0,
                    };
                    self.configs.push(Keyboard::new(config, output));
                }
                Err(config::LoadError::Parse(error)) if error.is_fatal() => {
                    return Err(io::Error::other(error));
                }
                Err(config::LoadError::Parse(_)) => log::write(
                    &mut stdout,
                    0,
                    "DEVICE: y{WARNING} failed to parse %s\n",
                    &[path.as_os_str().as_bytes()],
                )?,
                Err(config::LoadError::Output(error)) => return Err(error),
            }
        }
        // C prepends each directory entry; equal-ranked matches use the last parsed file.
        self.configs.reverse();
        for (owner, keyboard) in self.configs.iter_mut().enumerate() {
            keyboard.output.owner = owner;
        }
        Ok(())
    }

    fn lookup(&self, id: &str, flags: u8) -> Option<usize> {
        let mut rank = 0;
        let mut matched = None;
        for (index, keyboard) in self.configs.iter().enumerate() {
            let score = config::config_check_match(&keyboard.config, id, flags);
            if score > rank {
                rank = score;
                matched = Some(index);
            }
        }
        if rank == 1 && (flags & config::ID_KEYBOARD == 0 || flags & config::ID_TRACKPAD != 0) {
            None
        } else {
            if flags & config::ID_TRACKPAD != 0 {
                let _ = log::stdout(
                    0,
                    "y{WARNING}: %s appears to be a trackpad, which is current unsupported. Mouse movement is likely to break(YMMV)\n",
                    &[id.as_bytes()],
                );
            }
            matched
        }
    }

    fn manage_device(&mut self, slot: &mut DeviceSlot) -> io::Result<()> {
        let metadata = &slot.device.metadata;
        if metadata.is_virtual {
            return Ok(());
        }
        let capabilities = metadata.capabilities;
        let flags = if capabilities & device::CAP_KEY != 0 {
            config::ID_KEY
        } else {
            0
        } | if capabilities & device::CAP_KEYBOARD != 0 {
            config::ID_KEYBOARD
        } else {
            0
        } | if capabilities & device::CAP_MOUSE_ABS != 0 {
            config::ID_TRACKPAD
        } else {
            0
        } | if capabilities & device::CAP_MOUSE != 0 {
            config::ID_MOUSE
        } else {
            0
        };
        let owner = self.lookup(&metadata.id, flags);
        let stdout = io::stdout();
        let _lock = stdout.lock();
        let mut stdout = runtime::FdIo::new(stdout.as_fd())?;
        if let Some(fd) = slot.device.fd() {
            self.output
                .borrow_mut()
                .leds
                .retain(|target| target.device_fd != fd.as_raw_fd());
        }
        if let Some(owner) = owner {
            if slot.device.grab().is_err() {
                if runtime::shutdown_signal() != 0 {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                log::write(
                    &mut stdout,
                    0,
                    "DEVICE: y{WARNING} Failed to grab %s\n",
                    &[slot.device.metadata.path.as_os_str().as_bytes()],
                )?;
                slot.owner = None;
                return Ok(());
            }
            let metadata = &slot.device.metadata;
            log::write(
                &mut stdout,
                0,
                "DEVICE: g{match}    %s  %s\t(%s)\n",
                &[
                    metadata.id.as_bytes(),
                    self.configs[owner].config.path.as_os_str().as_bytes(),
                    metadata.name.as_bytes(),
                ],
            )?;
            slot.owner = Some(owner);
            let fd = slot.device.fd().ok_or(io::ErrorKind::NotConnected)?;
            // The duplicate owns the callback's LED access without borrowing the event loop's device table.
            self.output.borrow_mut().leds.push(LedTarget {
                owner,
                device_fd: fd.as_raw_fd(),
                fd: fd.try_clone_to_owned()?,
            });
        } else {
            slot.owner = None;
            let _ = slot.device.ungrab();
            let metadata = &slot.device.metadata;
            log::write(
                &mut stdout,
                0,
                "DEVICE: r{ignoring} %s  (%s)\n",
                &[metadata.id.as_bytes(), metadata.name.as_bytes()],
            )?;
        }
        Ok(())
    }

    fn reload(&mut self, devices: &mut [DeviceSlot]) -> io::Result<()> {
        // Indices cannot outlive the configurations they identify, including after a failed reload.
        self.active = None;
        self.output.borrow_mut().leds.clear();
        self.configs.clear();
        for slot in devices.iter_mut() {
            slot.owner = None;
        }
        self.load_configs()?;
        for slot in devices {
            self.manage_device(slot)?;
        }
        self.output.borrow_mut().clear_keys()
    }

    fn add_listener(&mut self, stream: UnixStream) -> io::Result<()> {
        if self.output.borrow().listeners.len() == 32 {
            return match runtime::FdIo::new(stream.as_fd())?
                .write_all(b"Max listeners exceeded\n\0")
            {
                Err(error) if disconnected(&error) => Ok(()),
                result => result.map_err(|error| log::system_error("write", error)),
            };
        }
        stream.set_nonblocking(false)?;
        stream.set_write_timeout(Some(Duration::from_millis(50)))?;
        let mut listener = &stream;
        if let Some(active) = self.active {
            let keyboard = &self.configs[active];
            let layout = keyboard
                .config
                .layers
                .iter()
                .zip(&keyboard.layer_state)
                .skip(1)
                .find(|(layer, state)| layer.kind == LayerType::Layout && state.active != 0)
                .map_or(&keyboard.config.layers[0], |(layer, _)| layer);
            if listener
                .write_all(&[b"/".as_slice(), layout.name.as_bytes(), b"\n"].concat())
                .is_err()
            {
                return Ok(());
            }
            for (layer, state) in keyboard
                .config
                .layers
                .iter()
                .zip(&keyboard.layer_state)
                .skip(1)
            {
                if layer.kind != LayerType::Layout
                    && state.active != 0
                    && listener
                        .write_all(&[b"+".as_slice(), layer.name.as_bytes(), b"\n"].concat())
                        .is_err()
                {
                    return Ok(());
                }
            }
        }
        self.output.borrow_mut().listeners.push(stream);
        Ok(())
    }

    fn input(&mut self, mut bytes: &[u8], timeout: u32) -> Result<(), Vec<u8>> {
        while let Some((codepoint, len)) =
            string::utf8_read_char(bytes).map_err(String::into_bytes)?
        {
            if runtime::shutdown_signal() != 0 {
                return Err(b"interrupted".to_vec());
            }
            let mut output = self.output.borrow_mut();
            let key = if len == 1 {
                let character = [codepoint as u8];
                parse_key_sequence(std::str::from_utf8(&character).unwrap_or("")).or(
                    match codepoint as u8 {
                        b' ' => Some((KEYD_SPACE, 0)),
                        b'\n' => Some((KEYD_ENTER, 0)),
                        b'\t' => Some((KEYD_TAB, 0)),
                        _ => None,
                    },
                )
            } else {
                None
            };
            if let Some((code, mods)) = key {
                if mods & MOD_SHIFT != 0 {
                    output.input_key(KEYD_LEFTSHIFT, 1)?;
                }
                output.input_key(code, 1)?;
                output.input_key(code, 0)?;
                if mods & MOD_SHIFT != 0 {
                    output.input_key(KEYD_LEFTSHIFT, 0)?;
                }
            } else {
                let index = unicode::unicode_lookup_index(codepoint);
                if index < 0 {
                    let mut error = b"ERROR: could not find code for \"".to_vec();
                    error.extend_from_slice(&bytes[..len]);
                    error.push(b'"');
                    return Err(error);
                }
                for code in unicode::unicode_get_sequence(index as u16) {
                    output.input_key(code, 1)?;
                    output.input_key(code, 0)?;
                }
            }
            bytes = &bytes[len..];
            if timeout != 0 {
                let _ = runtime::sleep_us_once(u64::from(timeout));
            }
        }
        Ok(())
    }

    fn handle_client(&mut self, stream: UnixStream, devices: &mut [DeviceSlot]) -> io::Result<()> {
        let mut connection = runtime::FdIo::new(stream.as_fd())?;
        let message = match ipc::Message::read(&mut connection) {
            Err(error) if disconnected(&error) => return Ok(()),
            result => result.map_err(|error| log::system_error("read", error))?,
        };
        let mut parser_error = false;
        let result = if let Err(error) = message.validate_request() {
            Err(error.into())
        } else {
            let payload = message.payload()?;
            let payload = &payload[..payload
                .iter()
                .position(|&byte| byte == 0)
                .unwrap_or(payload.len())];
            match message.kind {
                ipc::MACRO => {
                    // C trims the complete frame before its parser stops at NUL.
                    let mut bytes = message.payload()?;
                    while bytes.last() == Some(&b'\n') {
                        bytes = &bytes[..bytes.len() - 1];
                    }
                    let parsed = macros::macro_parse_bytes(bytes);
                    parser_error = parsed.is_err();
                    parsed.map_err(|_| String::new()).map(|macro_| {
                        let output = self.output.clone();
                        macros::macro_execute_while(
                            &macro_,
                            u64::from(message.timeout),
                            |code, state| {
                                let mut output = output.borrow_mut();
                                if runtime::shutdown_signal() == 0 && output.error.is_none() {
                                    let result = output.send_key(code, state);
                                    output.record(result);
                                }
                                output.error.is_none()
                            },
                        );
                    })
                }
                ipc::INPUT => self.input(payload, message.timeout).map_err(|error| {
                    log::set_error_bytes(&error);
                    parser_error = true;
                    String::new()
                }),
                ipc::RELOAD => {
                    self.reload(devices)?;
                    Ok(())
                }
                ipc::LAYER_LISTEN => {
                    drop(connection);
                    return self.add_listener(stream);
                }
                ipc::BIND => {
                    let mut success = false;
                    for keyboard in &mut self.configs {
                        let warning_start = keyboard.config.warnings.len();
                        let result = keyboard.eval(OsStr::from_bytes(payload));
                        if keyboard.config.warnings.len() != warning_start {
                            let warnings = keyboard.config.warnings.drain(warning_start..);
                            let stdout = io::stdout();
                            let _lock = stdout.lock();
                            let mut stdout = runtime::FdIo::new(stdout.as_fd())?;
                            for warning in warnings {
                                cli::warning(&warning, &mut stdout)?;
                            }
                        }
                        match result {
                            Ok(()) => success = true,
                            Err(error) if error.is_fatal() => {
                                return Err(io::Error::other(error));
                            }
                            Err(_) => (),
                        }
                    }
                    parser_error = !success;
                    if success { Ok(()) } else { Err(String::new()) }
                }
                _ => Err("Unknown command".into()),
            }
        };
        self.check_output()?;
        let reply = match result {
            Ok(()) => ipc::Message::new(ipc::SUCCESS, b"", 0)?,
            Err(error) => ipc::Message::new(
                ipc::FAIL,
                &if parser_error {
                    log::error()
                } else {
                    error.as_bytes()[..error.len().min(ipc::MAX_MESSAGE_SIZE - 1)].to_vec()
                },
                0,
            )?,
        };
        match reply.write(&mut connection) {
            Err(error) if disconnected(&error) => Ok(()),
            result => result.map_err(|error| log::system_error("write", error)),
        }
    }

    fn keypress(&mut self, owner: usize, code: u8, timestamp: i32) -> i32 {
        let keyboard = &mut self.configs[owner];
        keyboard.process_events(&[KeyEvent {
            code,
            pressed: 1,
            timestamp,
        }]);
        keyboard.process_events(&[KeyEvent {
            code,
            pressed: 0,
            timestamp,
        }]) as i32
    }

    fn handle(&mut self, event: Event, devices: &mut [DeviceSlot]) -> io::Result<i32> {
        self.check_output()?;
        self.timeout = self
            .timeout
            .wrapping_sub(event.timestamp.wrapping_sub(self.last_time))
            .max(0);
        self.last_time = event.timestamp;
        match event.kind {
            EventKind::Timeout => {
                if let Some(active) = self.active {
                    self.timeout = self.configs[active].process_events(&[KeyEvent {
                        timestamp: event.timestamp,
                        ..KeyEvent::default()
                    }]) as i32;
                } else {
                    return Ok(0);
                }
            }
            EventKind::DeviceAdd(index) => self.manage_device(&mut devices[index])?,
            EventKind::DeviceRemove(index) => {
                let slot = &devices[index];
                // Removed descriptors are already closed; retain duplicates only for live devices.
                self.output.borrow_mut().leds.retain(|target| {
                    devices.iter().any(|slot| {
                        slot.device
                            .fd()
                            .is_some_and(|fd| fd.as_raw_fd() == target.device_fd)
                    })
                });
                log::stdout(
                    0,
                    "DEVICE: r{removed}\t%s %s\n",
                    &[
                        slot.device.metadata.id.as_bytes(),
                        slot.device.metadata.name.as_bytes(),
                    ],
                )?;
            }
            EventKind::DeviceInput(index, input) => {
                let slot = &devices[index];
                if let Some(owner) = slot.owner {
                    self.active = Some(owner);
                    match input {
                        DeviceEvent::Key { code, pressed } => {
                            log::debug_stdout(
                                1,
                                "src/daemon.rs",
                                line!(),
                                "input %s %s",
                                &[
                                    KEYCODE_TABLE[code as usize]
                                        .name
                                        .unwrap_or("(null)")
                                        .as_bytes(),
                                    if pressed != 0 { b"down" } else { b"up" },
                                ],
                            )?;
                            self.timeout = self.configs[owner].process_events(&[KeyEvent {
                                code,
                                pressed,
                                timestamp: event.timestamp,
                            }]) as i32;
                        }
                        DeviceEvent::MouseMove { x, y } => {
                            let scroll = &mut self.configs[owner].scroll;
                            let mut output = self.output.borrow_mut();
                            if scroll.active {
                                if scroll.sensitivity != 0 {
                                    scroll.x = scroll.x.wrapping_add(x);
                                    scroll.y = scroll.y.wrapping_add(y);
                                    let xticks = scroll.x.wrapping_div(scroll.sensitivity);
                                    let yticks = scroll.y.wrapping_div(scroll.sensitivity);
                                    scroll.x = scroll.x.wrapping_rem(scroll.sensitivity);
                                    scroll.y = scroll.y.wrapping_rem(scroll.sensitivity);
                                    output.keyboard.mouse_scroll(0, yticks.wrapping_neg())?;
                                    // Preserve the baseline's horizontal-as-vertical output until the bug-fix phase.
                                    output.keyboard.mouse_scroll(0, xticks)?;
                                }
                            } else {
                                output.keyboard.mouse_move(x, y)?;
                            }
                        }
                        DeviceEvent::MouseMoveAbs { x, y } => {
                            self.output.borrow_mut().keyboard.mouse_move_abs(x, y)?
                        }
                        DeviceEvent::MouseScroll { x, y } => {
                            for (amount, positive, negative) in [
                                (x, KEYD_SCROLL_RIGHT, KEYD_SCROLL_LEFT),
                                (y, KEYD_SCROLL_UP, KEYD_SCROLL_DOWN),
                            ] {
                                for _ in 0..amount.unsigned_abs() {
                                    self.timeout = self.keypress(
                                        owner,
                                        if amount > 0 { positive } else { negative },
                                        event.timestamp,
                                    );
                                }
                            }
                        }
                        _ => {}
                    }
                } else if !slot.device.metadata.is_virtual
                    && slot.device.metadata.capabilities & device::CAP_MOUSE != 0
                {
                    if let Some(active) = self.active
                        && matches!(
                            input,
                            DeviceEvent::Key { .. } | DeviceEvent::MouseScroll { .. }
                        )
                    {
                        self.timeout =
                            self.keypress(active, KEYD_EXTERNAL_MOUSE_BUTTON, event.timestamp);
                    }
                } else if slot.device.metadata.is_virtual
                    && let DeviceEvent::Led { code, pressed } = input
                {
                    for slot in devices {
                        if slot.owner.is_some() {
                            slot.device.set_led(u16::from(code), i32::from(pressed))?;
                        }
                    }
                }
            }
            EventKind::FdActivity(_) | EventKind::FdError(_) => {}
        }
        self.check_output()?;
        Ok(self.timeout)
    }
}

pub fn run() -> io::Result<i32> {
    let server = ipc_server::bind_with_setup(crate::SOCKET_PATH, || {
        runtime::set_keyd_group().map_err(|error| log::system_error("setgid", error))
    })
    .map_err(|error| {
        if error.kind() == io::ErrorKind::WouldBlock && error.get_ref().is_none() {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "failed to create {} (another instance already running?)",
                    crate::SOCKET_PATH
                ),
            )
        } else {
            error
        }
    })?;
    let keyboard = vkbd::VirtualKeyboard::init("keyd virtual keyboard", crate::BACKEND)?;
    let mut daemon = Daemon::new(keyboard, Path::new(crate::CONFIG_DIR));
    runtime::set_realtime().map_err(|error| log::system_error("pthread_setschedparam", error))?;
    runtime::lock_memory().map_err(|error| log::system_error("mlockall", error))?;
    daemon.reload(&mut [])?;
    log::stdout(
        0,
        "Starting keyd-rust %s\n",
        &[crate::BUILD_VERSION.as_bytes()],
    )?;
    let mut event_loop = EventLoop::new()?;
    event_loop.add_fd(server.listener.as_fd())?;
    event_loop.announce_devices(|event, devices| daemon.handle(event, devices))?;
    loop {
        let result = event_loop.step(|event, devices| {
            if matches!(event.kind, EventKind::FdActivity(fd) if fd == server.listener.as_raw_fd())
            {
                let (stream, _) = server
                    .listener
                    .accept()
                    .map_err(|error| log::system_error("accept", error))?;
                daemon.handle_client(stream, devices)?;
            }
            daemon.handle(event, devices)
        });
        if runtime::shutdown_signal() != 0 {
            return Ok(runtime::shutdown_signal());
        }
        match result {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
            Ok(()) => {}
        }
    }
}
