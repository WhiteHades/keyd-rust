// Native wire port of scripts/keyd-application-mapper, frozen f564288.
// © 2019 Raheman Vaiya (see also: LICENSE).

use crate::runtime::FdIo;
use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const SERVER_ID: u32 = 0xff00_0000;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Wlroots,
    Cosmic,
}

impl Kind {
    fn interface(self) -> &'static str {
        match self {
            Self::Wlroots => "zwlr_foreign_toplevel_manager_v1",
            Self::Cosmic => "zcosmic_toplevel_info_v1",
        }
    }
}

pub struct Monitor {
    stream: UnixStream,
    kind: Kind,
    managers: Vec<u32>,
}

struct Message {
    object: u32,
    opcode: u16,
    data: Vec<u8>,
}

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn receive(stream: &UnixStream) -> io::Result<Message> {
    let mut io = FdIo::new(stream.as_fd())?;
    let mut header = [0; 8];
    io.read_exact(&mut header)?;
    let object = u32::from_ne_bytes(header[..4].try_into().unwrap());
    let word = u32::from_ne_bytes(header[4..].try_into().unwrap());
    let size = (word >> 16) as usize;
    if object == 0 || size < 8 || !size.is_multiple_of(4) {
        return Err(invalid("invalid Wayland message header"));
    }
    // The wire's 16-bit size bounds allocation before reading the payload.
    let mut data = vec![0; size - 8];
    io.read_exact(&mut data)?;
    Ok(Message {
        object,
        opcode: word as u16,
        data,
    })
}

fn send(stream: &UnixStream, object: u32, opcode: u16, data: &[u8]) -> io::Result<()> {
    let size = data
        .len()
        .checked_add(8)
        .filter(|&size| size <= u16::MAX as usize);
    let Some(size) = size.filter(|&size| size % 4 == 0) else {
        return Err(invalid("invalid outgoing Wayland message size"));
    };
    let mut io = FdIo::new(stream.as_fd())?;
    io.write_all(&object.to_ne_bytes())?;
    io.write_all(&((size as u32) << 16 | u32::from(opcode)).to_ne_bytes())?;
    io.write_all(data)
}

struct Payload<'a>(&'a [u8]);

impl<'a> Payload<'a> {
    fn take(&mut self, size: usize) -> io::Result<&'a [u8]> {
        if size > self.0.len() {
            return Err(invalid("truncated Wayland event arguments"));
        }
        let (head, tail) = self.0.split_at(size);
        self.0 = tail;
        Ok(head)
    }

    fn word(&mut self) -> io::Result<u32> {
        Ok(u32::from_ne_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn string(&mut self) -> io::Result<&'a str> {
        let size = self.word()? as usize;
        if size == 0 || size > self.0.len() {
            return Err(invalid("invalid Wayland string length"));
        }
        let bytes = self.take(size)?;
        if bytes[size - 1] != 0 || bytes[..size - 1].contains(&0) {
            return Err(invalid("invalid Wayland string terminator"));
        }
        self.take((4 - size % 4) % 4)?;
        std::str::from_utf8(&bytes[..size - 1]).map_err(|_| invalid("invalid Wayland string UTF-8"))
    }

    fn states(&mut self) -> io::Result<&'a [u8]> {
        let size = self.word()? as usize;
        if !size.is_multiple_of(4) {
            return Err(invalid("invalid Wayland state array size"));
        }
        self.take(size)
    }

    fn object(&mut self, nullable: bool) -> io::Result<u32> {
        let object = self.word()?;
        if object == 0 && !nullable {
            return Err(invalid("null Wayland object argument"));
        }
        Ok(object)
    }

    fn finish(self) -> io::Result<()> {
        if self.0.is_empty() {
            Ok(())
        } else {
            Err(invalid("extra Wayland event arguments"))
        }
    }
}

fn registry(message: &Message) -> io::Result<Option<&str>> {
    let mut data = Payload(&message.data);
    match message.opcode {
        0 => {
            let _name = data.word()?;
            let interface = data.string()?;
            if interface.is_empty() || data.word()? == 0 {
                return Err(invalid("invalid Wayland global interface or version"));
            }
            data.finish()?;
            Ok(Some(interface))
        }
        1 => {
            let _name = data.word()?;
            data.finish()?;
            Ok(None)
        }
        _ => Err(invalid("invalid Wayland registry event")),
    }
}

fn display(message: &Message, next_id: u32) -> io::Result<()> {
    let mut data = Payload(&message.data);
    match message.opcode {
        0 => {
            let object = data.object(false)?;
            let code = data.word()?;
            let text = data.string()?;
            data.finish()?;
            Err(io::Error::other(format!(
                "Wayland protocol error on object {object}, code {code}: {text}"
            )))
        }
        1 => {
            let id = data.object(false)?;
            if id >= next_id {
                return Err(invalid("invalid Wayland deleted object ID"));
            }
            data.finish()
        }
        _ => Err(invalid("invalid Wayland display event")),
    }
}

#[derive(Default)]
struct Window {
    title: String,
    appid: String,
    monitored: bool,
}

fn equal_active(windows: &HashMap<u32, Window>, active: Option<u32>, object: u32) -> bool {
    // shortcut: Python compares dictionaries by value, use object identity in a later bug-fix phase.
    active
        .and_then(|id| windows.get(&id))
        .is_some_and(|active| {
            windows
                .get(&object)
                .is_some_and(|window| active.title == window.title && active.appid == window.appid)
        })
}

impl Monitor {
    pub fn connect(kind: Kind) -> io::Result<Self> {
        let display = std::env::var_os("WAYLAND_DISPLAY")
            .filter(|display| !display.is_empty())
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "WAYLAND_DISPLAY not set"))?;
        let mut path = PathBuf::from(display);
        if !path.is_absolute() {
            let runtime = std::env::var_os("XDG_RUNTIME_DIR")
                .filter(|runtime| !runtime.is_empty())
                .ok_or_else(|| {
                    io::Error::new(io::ErrorKind::NotFound, "XDG_RUNTIME_DIR not set")
                })?;
            path = PathBuf::from(runtime).join(path);
        }
        Self::from_stream(crate::runtime::connect_unix(&path)?, kind)
    }

    pub fn from_stream(stream: UnixStream, kind: Kind) -> io::Result<Self> {
        send(&stream, 1, 1, &2u32.to_ne_bytes())?;
        send(&stream, 1, 0, &3u32.to_ne_bytes())?;
        let mut managers = Vec::new();
        let mut next_id = 4u32;
        loop {
            let message = receive(&stream)?;
            match message.object {
                1 => display(&message, next_id)?,
                2 => {
                    if registry(&message)? == Some(kind.interface()) {
                        if next_id >= SERVER_ID {
                            return Err(invalid("Wayland client object IDs exhausted"));
                        }
                        // shortcut: preserve advertised COSMIC versions, add ext_foreign_toplevel for v2+ later.
                        let mut bind = message.data;
                        bind.extend_from_slice(&next_id.to_ne_bytes());
                        send(&stream, 2, 0, &bind)?;
                        managers.push(next_id);
                        next_id += 1;
                    }
                }
                3 => {
                    if message.opcode != 0 {
                        return Err(invalid("invalid Wayland sync callback event"));
                    }
                    let mut data = Payload(&message.data);
                    let _serial = data.word()?;
                    data.finish()?;
                    if managers.is_empty() {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("Could not find interface {}", kind.interface()),
                        ));
                    }
                    return Ok(Self {
                        stream,
                        kind,
                        managers,
                    });
                }
                _ => return Err(invalid("unknown Wayland object during registry sync")),
            }
        }
    }

    pub fn run(
        &mut self,
        mut on_change: impl FnMut(&str, &str) -> io::Result<()>,
        mut wait: impl FnMut(BorrowedFd<'_>) -> io::Result<bool>,
    ) -> io::Result<()> {
        let mut windows = HashMap::<u32, Window>::new();
        let mut active = None;
        loop {
            if wait(self.stream.as_fd())? {
                if self.kind == Kind::Wlroots
                    && let Some(window) = active.and_then(|id| windows.get(&id))
                {
                    on_change(&window.appid, &window.title)?;
                }
                continue;
            }
            let message = receive(&self.stream)?;
            let mut data = Payload(&message.data);
            if message.object == 1 {
                display(&message, 4 + self.managers.len() as u32)?;
            } else if message.object == 2 {
                registry(&message)?;
            } else if self.managers.contains(&message.object) {
                match message.opcode {
                    0 => {
                        let object = data.object(false)?;
                        data.finish()?;
                        if object < SERVER_ID || windows.contains_key(&object) {
                            return Err(invalid("invalid Wayland toplevel object ID"));
                        }
                        windows.insert(
                            object,
                            Window {
                                monitored: self.kind == Kind::Cosmic
                                    || Some(&message.object) == self.managers.last(),
                                ..Window::default()
                            },
                        );
                    }
                    1 => data.finish()?,
                    2 if self.kind == Kind::Cosmic => data.finish()?,
                    _ => (),
                }
            } else if windows.contains_key(&message.object) {
                let cosmic = self.kind == Kind::Cosmic;
                let monitored = windows[&message.object].monitored;
                match message.opcode {
                    opcode if opcode == if cosmic { 2 } else { 0 } => {
                        let title = data.string()?.to_owned();
                        data.finish()?;
                        windows.get_mut(&message.object).unwrap().title = title;
                    }
                    opcode if opcode == if cosmic { 3 } else { 1 } => {
                        let appid = data.string()?.to_owned();
                        data.finish()?;
                        windows.get_mut(&message.object).unwrap().appid = appid;
                    }
                    opcode if opcode == if cosmic { 8 } else { 4 } => {
                        let states = data.states()?;
                        data.finish()?;
                        if cosmic {
                            // shortcut: the baseline tests only length/first-state low bytes, scan all states in a later bug-fix phase.
                            if states.len() as u8 > 0 && states.first() == Some(&2) {
                                let window = &windows[&message.object];
                                on_change(&window.appid, &window.title)?;
                            }
                        } else if monitored {
                            if equal_active(&windows, active, message.object) {
                                active = None;
                            }
                            if states
                                .as_chunks::<4>()
                                .0
                                .iter()
                                .any(|state| u32::from_ne_bytes(*state) == 2)
                            {
                                active = Some(message.object);
                            }
                        }
                    }
                    opcode if opcode == if cosmic { 1 } else { 5 } => {
                        data.finish()?;
                        if !cosmic
                            && monitored
                            && equal_active(&windows, active, message.object)
                            && let Some(window) = active.and_then(|id| windows.get(&id))
                        {
                            on_change(&window.appid, &window.title)?;
                        }
                    }
                    opcode if opcode == if cosmic { 0 } else { 6 } => {
                        data.finish()?;
                        // shortcut: COSMIC retains closed entries and neither backend sends destroy, match baseline until lifecycle fixes.
                        if !cosmic {
                            if monitored && equal_active(&windows, active, message.object) {
                                active = None;
                            }
                            windows.remove(&message.object);
                        }
                    }
                    opcode if (if cosmic { 4..=7 } else { 2..=3 }).contains(&opcode) => {
                        data.object(false)?;
                        data.finish()?;
                    }
                    7 if !cosmic => {
                        let parent = data.object(true)?;
                        data.finish()?;
                        if parent != 0 && !windows.contains_key(&parent) {
                            return Err(invalid("unknown Wayland parent object ID"));
                        }
                    }
                    9 if cosmic => {
                        data.object(false)?;
                        for _ in 0..4 {
                            data.word()?;
                        }
                        data.finish()?;
                    }
                    10 | 11 if cosmic => {
                        data.object(false)?;
                        data.finish()?;
                    }
                    _ => (),
                }
            } else {
                return Err(invalid("unknown Wayland event object ID"));
            }
        }
    }
}
