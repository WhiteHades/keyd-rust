// Native D-Bus port of scripts/keyd-application-mapper, frozen f564288.
// © 2019 Raheman Vaiya (see also: LICENSE).

use crate::{linux, runtime};
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::linux::net::SocketAddrExt;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt;
use std::os::unix::net::{SocketAddr, UnixStream};
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

const BUS: &str = "org.freedesktop.DBus";
const BUS_PATH: &str = "/org/freedesktop/DBus";
const SERVICE: &str = "org.rvaiya.keyd";
const INTROSPECT: &str = "org.freedesktop.DBus.Introspectable";
const PEER: &str = "org.freedesktop.DBus.Peer";
// shortcut: mapper messages are capped at 1 MiB and headers at 64 KiB; raise only
// if a compositor needs larger captions or introspection documents.
const MAX_BODY: usize = 1024 * 1024;
const MAX_HEADER: usize = 64 * 1024;

const XML: &str = r#"<!DOCTYPE node PUBLIC "-//freedesktop//DTD D-BUS Object Introspection 1.0//EN"
"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd">
<node name="/listener">
  <interface name="org.rvaiya.keyd">
    <method name="updateWindow">
      <arg name="title" type="v" direction="in"/>
      <arg name="klass" type="v" direction="in"/>
      <arg name="id" type="v" direction="in"/>
    </method>
  </interface>
  <interface name="org.freedesktop.DBus.Introspectable">
    <method name="Introspect"><arg type="s" direction="out"/></method>
  </interface>
  <interface name="org.freedesktop.DBus.Peer">
    <method name="Ping"/>
    <method name="GetMachineId"><arg type="s" direction="out"/></method>
  </interface>
</node>"#;

fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn uid() -> u32 {
    #[cfg(target_arch = "x86_64")]
    const GETUID: usize = 102;
    #[cfg(target_arch = "aarch64")]
    const GETUID: usize = 174;
    #[cfg(target_arch = "x86")]
    const GETUID: usize = 199;
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { linux::syscall6(GETUID, [0; 6]) as u32 }
}

fn decode_address(value: &str) -> io::Result<Vec<u8>> {
    let mut bytes = value.bytes();
    let mut result = Vec::new();
    while let Some(byte) = bytes.next() {
        if byte == b'%' {
            let digit = |byte: Option<u8>| {
                byte.and_then(|byte| (byte as char).to_digit(16))
                    .map(|digit| digit as u8)
            };
            let high =
                digit(bytes.next()).ok_or_else(|| invalid("invalid D-Bus address escape"))?;
            let low = digit(bytes.next()).ok_or_else(|| invalid("invalid D-Bus address escape"))?;
            result.push(high * 16 + low);
        } else if byte.is_ascii_alphanumeric() || b"_-/.\\*".contains(&byte) {
            result.push(byte);
        } else {
            return Err(invalid("invalid D-Bus address character"));
        }
    }
    if result.is_empty() || result.contains(&0) {
        return Err(invalid("empty or NUL-containing D-Bus address"));
    }
    Ok(result)
}

fn bus_socket(address: &str) -> io::Result<(UnixStream, Option<String>)> {
    let mut last = io::Error::new(
        io::ErrorKind::Unsupported,
        "session bus requires a unix address",
    );
    for entry in address.split(';') {
        let Some(parameters) = entry.strip_prefix("unix:") else {
            continue;
        };
        let attempt = (|| {
            let mut endpoint = None;
            let mut guid = None;
            for parameter in parameters.split(',') {
                let (key, value) = parameter
                    .split_once('=')
                    .ok_or_else(|| invalid("invalid D-Bus address parameter"))?;
                match key {
                    "path" | "abstract" => {
                        if endpoint.is_some() {
                            return Err(invalid("duplicate D-Bus socket address"));
                        }
                        let bytes = decode_address(value)?;
                        endpoint = Some(if key == "abstract" {
                            SocketAddr::from_abstract_name(bytes)?
                        } else {
                            if bytes[0] != b'/' {
                                return Err(invalid("D-Bus socket path must be absolute"));
                            }
                            SocketAddr::from_pathname(std::ffi::OsStr::from_bytes(&bytes))?
                        });
                    }
                    "guid" => {
                        if guid.is_some()
                            || value.len() != 32
                            || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                        {
                            return Err(invalid("invalid D-Bus address GUID"));
                        }
                        guid = Some(value.to_ascii_lowercase());
                    }
                    _ => {
                        return Err(io::Error::new(
                            io::ErrorKind::Unsupported,
                            "unsupported D-Bus unix address parameter",
                        ));
                    }
                }
            }
            let endpoint = endpoint.ok_or_else(|| invalid("missing D-Bus socket address"))?;
            let stream = if let Some(path) = endpoint.as_pathname() {
                runtime::connect_unix(path)?
            } else if let Some(name) = endpoint.as_abstract_name() {
                runtime::connect_unix_abstract(name)?
            } else {
                return Err(invalid("unnamed D-Bus socket address"));
            };
            Ok((stream, guid))
        })();
        match attempt {
            Ok(socket) => return Ok(socket),
            Err(error) => last = error,
        }
    }
    Err(last)
}

fn authenticate(stream: &UnixStream, expected_guid: Option<&str>) -> io::Result<()> {
    let mut io = runtime::FdIo::new(stream.as_fd())?;
    let identity: String = uid()
        .to_string()
        .bytes()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    write!(io, "\0AUTH EXTERNAL {identity}\r\n")?;
    let mut line = Vec::new();
    loop {
        let mut byte = [0];
        io.read_exact(&mut byte)?;
        line.push(byte[0]);
        if line.ends_with(b"\r\n") {
            break;
        }
        if line.len() >= 4096 {
            return Err(invalid("oversized D-Bus authentication response"));
        }
    }
    let response =
        std::str::from_utf8(&line).map_err(|_| invalid("invalid D-Bus authentication response"))?;
    let guid = response
        .strip_prefix("OK ")
        .and_then(|text| text.strip_suffix("\r\n"))
        .filter(|text| text.len() == 32 && text.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::PermissionDenied,
                "D-Bus EXTERNAL authentication rejected",
            )
        })?;
    if expected_guid.is_some_and(|expected| !expected.eq_ignore_ascii_case(guid)) {
        return Err(invalid("D-Bus authentication GUID does not match address"));
    }
    // No NEGOTIATE_UNIX_FD: this connection never accepts passed descriptors.
    io.write_all(b"BEGIN\r\n")
}

#[derive(Debug)]
enum Type {
    Basic(u8),
    Variant,
    Array(Box<Type>),
    Struct(Vec<Type>),
    Dict(Box<Type>, Box<Type>),
}

impl Type {
    fn alignment(&self) -> usize {
        match self {
            Self::Basic(b'y' | b'g') | Self::Variant => 1,
            Self::Basic(b'n' | b'q') => 2,
            Self::Basic(b'x' | b't' | b'd') | Self::Struct(_) | Self::Dict(_, _) => 8,
            _ => 4,
        }
    }
}

fn parse_type(bytes: &[u8], position: &mut usize, depth: usize, dict: bool) -> io::Result<Type> {
    if depth > 32 {
        return Err(invalid("D-Bus signature nesting exceeds 32"));
    }
    let byte = *bytes
        .get(*position)
        .ok_or_else(|| invalid("truncated D-Bus signature"))?;
    *position += 1;
    match byte {
        b'y' | b'b' | b'n' | b'q' | b'i' | b'u' | b'x' | b't' | b'd' | b's' | b'o' | b'g'
        | b'h' => Ok(Type::Basic(byte)),
        b'v' => Ok(Type::Variant),
        b'a' => Ok(Type::Array(Box::new(parse_type(
            bytes,
            position,
            depth + 1,
            true,
        )?))),
        b'(' => {
            let mut fields = Vec::new();
            while bytes.get(*position) != Some(&b')') {
                fields.push(parse_type(bytes, position, depth + 1, false)?);
            }
            *position += 1;
            if fields.is_empty() {
                return Err(invalid("empty D-Bus struct signature"));
            }
            Ok(Type::Struct(fields))
        }
        b'{' if dict => {
            let key = parse_type(bytes, position, depth + 1, false)?;
            if !matches!(key, Type::Basic(_)) {
                return Err(invalid("invalid D-Bus dictionary key"));
            }
            let value = parse_type(bytes, position, depth + 1, false)?;
            if bytes.get(*position) != Some(&b'}') {
                return Err(invalid("invalid D-Bus dictionary signature"));
            }
            *position += 1;
            Ok(Type::Dict(Box::new(key), Box::new(value)))
        }
        _ => Err(invalid("invalid D-Bus signature type")),
    }
}

fn signature(text: &str) -> io::Result<Vec<Type>> {
    if text.len() > 255 {
        return Err(invalid("oversized D-Bus signature"));
    }
    let mut types = Vec::new();
    let mut position = 0;
    while position < text.len() {
        types.push(parse_type(text.as_bytes(), &mut position, 0, false)?);
    }
    Ok(types)
}

fn object_path(text: &str) -> bool {
    text == "/"
        || text.strip_prefix('/').is_some_and(|rest| {
            rest.split('/').all(|part| {
                !part.is_empty()
                    && part
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            })
        })
}

fn member(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 255
        && !text.as_bytes()[0].is_ascii_digit()
        && text
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

fn interface(text: &str) -> bool {
    text.len() <= 255 && text.contains('.') && text.split('.').all(member)
}

fn bus_name(text: &str) -> bool {
    let unique = text.starts_with(':');
    let name = if unique { &text[1..] } else { text };
    text.len() <= 255
        && name.contains('.')
        && name.split('.').all(|part| {
            !part.is_empty()
                && (unique || !part.as_bytes()[0].is_ascii_digit())
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_-".contains(&byte))
        })
}

struct Decoder<'a> {
    bytes: &'a [u8],
    position: usize,
    little: bool,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8], little: bool) -> Self {
        Self {
            bytes,
            position: 0,
            little,
        }
    }

    fn take(&mut self, length: usize) -> io::Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(length)
            .filter(|&end| end <= self.bytes.len())
            .ok_or_else(|| invalid("truncated D-Bus value"))?;
        let bytes = &self.bytes[self.position..end];
        self.position = end;
        Ok(bytes)
    }

    fn align(&mut self, alignment: usize) -> io::Result<()> {
        let length = (alignment - self.position % alignment) % alignment;
        if self.take(length)?.iter().any(|&byte| byte != 0) {
            return Err(invalid("nonzero D-Bus alignment padding"));
        }
        Ok(())
    }

    fn word(&mut self) -> io::Result<u32> {
        self.align(4)?;
        let bytes = self.take(4)?.try_into().unwrap();
        Ok(if self.little {
            u32::from_le_bytes(bytes)
        } else {
            u32::from_be_bytes(bytes)
        })
    }

    fn text(&mut self) -> io::Result<&'a str> {
        let length = self.word()? as usize;
        let bytes = self.take(length)?;
        if self.take(1)? != [0] || bytes.contains(&0) {
            return Err(invalid("invalid D-Bus string terminator"));
        }
        std::str::from_utf8(bytes).map_err(|_| invalid("invalid D-Bus string UTF-8"))
    }

    fn sig(&mut self) -> io::Result<&'a str> {
        let length = self.take(1)?[0] as usize;
        let bytes = self.take(length)?;
        if self.take(1)? != [0] {
            return Err(invalid("invalid D-Bus signature terminator"));
        }
        let text =
            std::str::from_utf8(bytes).map_err(|_| invalid("invalid D-Bus signature encoding"))?;
        signature(text)?;
        Ok(text)
    }

    fn skip(&mut self, ty: &Type, depth: usize) -> io::Result<()> {
        if depth > 32 {
            return Err(invalid("D-Bus value nesting exceeds 32"));
        }
        self.align(ty.alignment())?;
        match ty {
            Type::Basic(b'y') => {
                self.take(1)?;
            }
            Type::Basic(b'n' | b'q') => {
                self.take(2)?;
            }
            Type::Basic(b'i' | b'u') => {
                self.take(4)?;
            }
            Type::Basic(b'x' | b't' | b'd') => {
                self.take(8)?;
            }
            Type::Basic(b'b') => {
                if self.word()? > 1 {
                    return Err(invalid("invalid D-Bus boolean"));
                }
            }
            Type::Basic(b's') => {
                self.text()?;
            }
            Type::Basic(b'o') => {
                if !object_path(self.text()?) {
                    return Err(invalid("invalid D-Bus object path"));
                }
            }
            Type::Basic(b'g') => {
                self.sig()?;
            }
            Type::Basic(_) => return Err(invalid("D-Bus file descriptors are not supported")),
            Type::Variant => {
                let types = signature(self.sig()?)?;
                if types.len() != 1 {
                    return Err(invalid("D-Bus variant requires one type"));
                }
                self.skip(&types[0], depth + 1)?;
            }
            Type::Array(element) => {
                let length = self.word()? as usize;
                self.align(element.alignment())?;
                let end = self
                    .position
                    .checked_add(length)
                    .filter(|&end| end <= self.bytes.len())
                    .ok_or_else(|| invalid("truncated D-Bus array"))?;
                let outer = self.bytes;
                self.bytes = &outer[..end];
                while self.position < end {
                    self.skip(element, depth + 1)?;
                }
                self.bytes = outer;
            }
            Type::Struct(fields) => {
                for field in fields {
                    self.skip(field, depth + 1)?;
                }
            }
            Type::Dict(key, value) => {
                self.skip(key, depth + 1)?;
                self.skip(value, depth + 1)?;
            }
        }
        Ok(())
    }

    fn finish(&self) -> io::Result<()> {
        if self.position == self.bytes.len() {
            Ok(())
        } else {
            Err(invalid("extra D-Bus value bytes"))
        }
    }
}

#[derive(Default)]
struct Message {
    kind: u8,
    flags: u8,
    serial: u32,
    reply: u32,
    little: bool,
    path: String,
    interface: String,
    member: String,
    error: String,
    destination: String,
    sender: String,
    signature: String,
    body: Vec<u8>,
}

fn receive(stream: &UnixStream, deadline: Option<Instant>) -> io::Result<Message> {
    let mut io = match deadline {
        Some(deadline) => runtime::FdIo::with_deadline(stream.as_fd(), deadline)?,
        None => runtime::FdIo::new(stream.as_fd())?,
    };
    let mut fixed = [0; 16];
    io.read_exact(&mut fixed)?;
    let little = match fixed[0] {
        b'l' => true,
        b'B' => false,
        _ => return Err(invalid("invalid D-Bus byte order")),
    };
    if !(1..=4).contains(&fixed[1]) || fixed[3] != 1 {
        return Err(invalid("invalid D-Bus message type or version"));
    }
    let mut numbers = Decoder::new(&fixed[4..], little);
    let body_length = numbers.word()? as usize;
    let serial = numbers.word()?;
    let header_length = numbers.word()? as usize;
    if serial == 0 || body_length > MAX_BODY || header_length > MAX_HEADER {
        return Err(invalid("invalid D-Bus serial or message size"));
    }
    let padded = header_length.next_multiple_of(8);
    let mut header = vec![0; padded];
    io.read_exact(&mut header)?;
    if header[header_length..].iter().any(|&byte| byte != 0) {
        return Err(invalid("nonzero D-Bus header padding"));
    }
    let mut data = Decoder::new(&header[..header_length], little);
    let mut message = Message {
        kind: fixed[1],
        flags: fixed[2],
        serial,
        little,
        ..Message::default()
    };
    let mut fields = [false; 256];
    while data.position < header_length {
        data.align(8)?;
        let code = data.take(1)?[0] as usize;
        if code == 0 || fields[code] {
            return Err(invalid("invalid or duplicate D-Bus header field"));
        }
        fields[code] = true;
        let ty = data.sig()?;
        let expected = match code {
            1 => "o",
            2 | 3 | 4 | 6 | 7 => "s",
            5 | 9 => "u",
            8 => "g",
            _ => ty,
        };
        if ty != expected {
            return Err(invalid("invalid D-Bus header field signature"));
        }
        match code {
            1 => message.path = data.text()?.into(),
            2 => message.interface = data.text()?.into(),
            3 => message.member = data.text()?.into(),
            4 => message.error = data.text()?.into(),
            5 => {
                message.reply = data.word()?;
                if message.reply == 0 {
                    return Err(invalid("zero D-Bus reply serial"));
                }
            }
            6 => message.destination = data.text()?.into(),
            7 => message.sender = data.text()?.into(),
            8 => message.signature = data.sig()?.into(),
            9 => {
                if data.word()? != 0 {
                    return Err(invalid("D-Bus file descriptors are not supported"));
                }
            }
            _ => {
                let types = signature(ty)?;
                if types.len() != 1 {
                    return Err(invalid("invalid D-Bus header variant"));
                }
                data.skip(&types[0], 0)?;
            }
        }
    }
    data.finish()?;
    if (fields[1] && !object_path(&message.path))
        || (fields[2] && !interface(&message.interface))
        || (fields[3] && !member(&message.member))
        || (fields[4] && !interface(&message.error))
        || (fields[6] && !bus_name(&message.destination))
        || (fields[7] && !bus_name(&message.sender))
    {
        return Err(invalid("invalid D-Bus header name or path"));
    }
    let required = match message.kind {
        1 => fields[1] && fields[3],
        2 => fields[5],
        3 => fields[4] && fields[5],
        _ => fields[1] && fields[2] && fields[3],
    };
    if !required {
        return Err(invalid("missing required D-Bus header field"));
    }
    message.body = vec![0; body_length];
    io.read_exact(&mut message.body)?;
    let mut body = Decoder::new(&message.body, little);
    for ty in signature(&message.signature)? {
        body.skip(&ty, 0)?;
    }
    body.finish()?;
    Ok(message)
}

fn pad(bytes: &mut Vec<u8>, alignment: usize) {
    bytes.resize(bytes.len().next_multiple_of(alignment), 0);
}

fn put_text(bytes: &mut Vec<u8>, text: &str) {
    pad(bytes, 4);
    bytes.extend_from_slice(&(text.len() as u32).to_le_bytes());
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
}

fn put_sig(bytes: &mut Vec<u8>, text: &str) {
    bytes.push(text.len() as u8);
    bytes.extend_from_slice(text.as_bytes());
    bytes.push(0);
}

fn text_body(text: &str) -> Vec<u8> {
    let mut body = Vec::new();
    put_text(&mut body, text);
    body
}

fn send(stream: &UnixStream, message: &Message, deadline: Option<Instant>) -> io::Result<()> {
    let mut body = Decoder::new(&message.body, true);
    for ty in signature(&message.signature)? {
        body.skip(&ty, 0)?;
    }
    body.finish()?;
    let mut header = Vec::new();
    for (code, ty, value) in [
        (1, "o", message.path.as_str()),
        (2, "s", &message.interface),
        (3, "s", &message.member),
        (4, "s", &message.error),
        (6, "s", &message.destination),
        (8, "g", &message.signature),
    ] {
        if value.is_empty() {
            continue;
        }
        pad(&mut header, 8);
        header.push(code);
        put_sig(&mut header, ty);
        if ty == "g" {
            put_sig(&mut header, value);
        } else {
            put_text(&mut header, value);
        }
    }
    if message.reply != 0 {
        pad(&mut header, 8);
        header.push(5);
        put_sig(&mut header, "u");
        pad(&mut header, 4);
        header.extend_from_slice(&message.reply.to_le_bytes());
    }
    if message.body.len() > MAX_BODY || header.len() > MAX_HEADER {
        return Err(invalid("oversized outgoing D-Bus message"));
    }
    let mut bytes = vec![b'l', message.kind, message.flags, 1];
    bytes.extend_from_slice(&(message.body.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&message.serial.to_le_bytes());
    bytes.extend_from_slice(&(header.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&header);
    pad(&mut bytes, 8);
    bytes.extend_from_slice(&message.body);
    let mut io = match deadline {
        Some(deadline) => runtime::FdIo::with_deadline(stream.as_fd(), deadline)?,
        None => runtime::FdIo::new(stream.as_fd())?,
    };
    io.write_all(&bytes)
}

struct ScriptFile(PathBuf);

impl Drop for ScriptFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
    }
}

pub struct Monitor {
    stream: UnixStream,
    serial: u32,
    unique_name: String,
    kde6: bool,
}

impl Monitor {
    pub fn connect() -> io::Result<Self> {
        let version = std::env::var_os("KDE_SESSION_VERSION")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "KDE_SESSION_VERSION not set")
            })?;
        let address = std::env::var("DBUS_SESSION_BUS_ADDRESS").map_err(|_| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "DBUS_SESSION_BUS_ADDRESS not set or invalid",
            )
        })?;
        let (stream, guid) = bus_socket(&address)?;
        authenticate(&stream, guid.as_deref())?;
        let mut monitor = Self {
            stream,
            serial: 0,
            unique_name: String::new(),
            kde6: version == "6",
        };
        let reply = monitor.call(BUS, BUS_PATH, BUS, "Hello", "", Vec::new())?;
        monitor.unique_name = Self::reply_text(&reply)?.into();
        if !monitor.unique_name.starts_with(':') || !bus_name(&monitor.unique_name) {
            return Err(invalid("invalid D-Bus Hello unique name"));
        }
        if reply.destination != monitor.unique_name {
            return Err(invalid("D-Bus Hello reply has wrong destination"));
        }
        Ok(monitor)
    }

    fn serial(&mut self) -> u32 {
        self.serial = self.serial.wrapping_add(1).max(1);
        self.serial
    }

    fn call(
        &mut self,
        destination: &str,
        path: &str,
        interface: &str,
        member: &str,
        sig: &str,
        body: Vec<u8>,
    ) -> io::Result<Message> {
        let serial = self.serial();
        // dbus-python's default -1 selects libdbus's 25-second reply timeout.
        let deadline = Some(Instant::now() + std::time::Duration::from_secs(25));
        send(
            &self.stream,
            &Message {
                kind: 1,
                serial,
                destination: destination.into(),
                path: path.into(),
                interface: interface.into(),
                member: member.into(),
                signature: sig.into(),
                body,
                ..Message::default()
            },
            deadline,
        )?;
        loop {
            let reply = receive(&self.stream, deadline)?;
            if reply.kind == 4 {
                self.signal(&reply)?;
                continue;
            }
            if !matches!(reply.kind, 2 | 3) || reply.reply != serial {
                return Err(invalid("unexpected D-Bus method reply"));
            }
            if (!self.unique_name.is_empty() && reply.destination != self.unique_name)
                || (destination == BUS && reply.sender != BUS)
                || (destination != BUS
                    && !reply.sender.starts_with(':')
                    && !(reply.kind == 3 && reply.sender == BUS))
            {
                return Err(invalid(
                    "D-Bus method reply has wrong sender or destination",
                ));
            }
            if reply.kind == 3 {
                let text = if reply.signature == "s" {
                    Self::reply_text(&reply)?
                } else {
                    ""
                };
                return Err(io::Error::other(format!("D-Bus {}: {text}", reply.error)));
            }
            return Ok(reply);
        }
    }

    fn reply_text(reply: &Message) -> io::Result<&str> {
        if reply.signature != "s" {
            return Err(invalid("expected D-Bus string reply"));
        }
        Decoder::new(&reply.body, reply.little).text()
    }

    fn reply_word(reply: &Message, sig: &str) -> io::Result<u32> {
        if reply.signature != sig {
            return Err(invalid("unexpected D-Bus reply signature"));
        }
        Decoder::new(&reply.body, reply.little).word()
    }

    fn signal(&self, message: &Message) -> io::Result<()> {
        if message.sender == BUS
            && message.interface == BUS
            && message.member == "NameLost"
            && Self::reply_text(message)? == SERVICE
        {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "lost D-Bus mapper service name",
            ));
        }
        Ok(())
    }

    pub fn init(&mut self) -> io::Result<()> {
        let mut last = None;
        for _ in 0..5 {
            match self.call(
                "org.kde.KWin",
                "/Scripting",
                INTROSPECT,
                "Introspect",
                "",
                Vec::new(),
            ) {
                Ok(reply) => {
                    Self::reply_text(&reply)?;
                    last = None;
                    break;
                }
                Err(error)
                    if matches!(error.kind(), io::ErrorKind::Other | io::ErrorKind::TimedOut) =>
                {
                    last = Some(error)
                }
                Err(error) => return Err(error),
            }
            let start = Instant::now();
            while start.elapsed().as_millis() < 1000 {
                if runtime::shutdown_signal() != 0 {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let remaining = 1000 - start.elapsed().as_millis().min(1000) as i32;
                match runtime::poll_interruptible(&mut [], remaining) {
                    Err(error)
                        if error.kind() == io::ErrorKind::Interrupted
                            && runtime::shutdown_signal() == 0 =>
                    {
                        continue;
                    }
                    result => {
                        result?;
                    }
                }
            }
        }
        if let Some(error) = last {
            return Err(error);
        }

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let directory = std::env::temp_dir();
        let directory = if directory.is_absolute() {
            directory
        } else {
            std::env::current_dir()?.join(directory)
        };
        let path = directory.join(format!(
            "keyd-kwin-{}-{}-{timestamp}.js",
            uid(),
            std::process::id()
        ));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        let script = ScriptFile(path);
        let api = if self.kde6 {
            "windowActivated"
        } else {
            "clientActivated"
        };
        write!(
            file,
            "workspace.{api}.connect(client => {{\n    if (!client) return;\n    callDBus(\"org.rvaiya.keyd\", \"/listener\", \"\", \"updateWindow\", client.caption, client.resourceClass, client.resourceName);\n}});\n"
        )?;
        drop(file);
        // A stable plugin name preserves restart/unload semantics despite the
        // intentional safety difference from the source's fixed /tmp filename.
        let plugin = format!("/tmp/keyd-kwin-{}.js", uid());
        let reply = self.call(
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
            "unloadScript",
            "s",
            text_body(&plugin),
        )?;
        Self::reply_word(&reply, "b")?;
        let path = script
            .0
            .to_str()
            .ok_or_else(|| invalid("KWin script path is not UTF-8"))?;
        let mut body = text_body(path);
        put_text(&mut body, &plugin);
        let reply = self.call(
            "org.kde.KWin",
            "/Scripting",
            "org.kde.kwin.Scripting",
            "loadScript",
            "ss",
            body,
        )?;
        let number = Self::reply_word(&reply, "i")? as i32;
        if number < 0 {
            return Err(io::Error::other("KWin refused to load mapper script"));
        }
        let object = if self.kde6 {
            format!("/Scripting/Script{number}")
        } else {
            format!("/{number}")
        };
        let reply = self.call(
            "org.kde.KWin",
            &object,
            "org.kde.kwin.Script",
            "run",
            "",
            Vec::new(),
        )?;
        if !reply.signature.is_empty() {
            return Err(invalid("unexpected KWin run reply signature"));
        }
        // KWin's delayed run reply follows its asynchronous file read.
        drop(script);
        Ok(())
    }

    fn respond(
        &mut self,
        request: &Message,
        error: &str,
        sig: &str,
        body: Vec<u8>,
    ) -> io::Result<()> {
        if request.flags & 1 != 0 {
            return Ok(());
        }
        let serial = self.serial();
        send(
            &self.stream,
            &Message {
                kind: if error.is_empty() { 2 } else { 3 },
                serial,
                reply: request.serial,
                destination: request.sender.clone(),
                error: error.into(),
                signature: sig.into(),
                body,
                ..Message::default()
            },
            None,
        )
    }

    fn error(&mut self, request: &Message, error: &str, text: &str) -> io::Result<()> {
        self.respond(request, error, "s", text_body(text))
    }

    pub fn run(
        &mut self,
        mut on_change: impl FnMut(&str, &str) -> io::Result<()>,
    ) -> io::Result<()> {
        let mut body = text_body(SERVICE);
        pad(&mut body, 4);
        body.extend_from_slice(&4u32.to_le_bytes()); // DO_NOT_QUEUE, never replace another owner.
        let reply = self.call(BUS, BUS_PATH, BUS, "RequestName", "su", body)?;
        if !matches!(Self::reply_word(&reply, "u")?, 1 | 4) {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                "D-Bus mapper service name is already owned",
            ));
        }
        loop {
            let request = receive(&self.stream, None)?;
            if request.kind == 4 {
                self.signal(&request)?;
                continue;
            }
            if request.kind != 1 {
                continue;
            }
            if !request.sender.starts_with(':') {
                return Err(invalid("D-Bus method call lacks a unique sender"));
            }
            if request.destination != SERVICE && request.destination != self.unique_name {
                return Err(invalid("D-Bus method call has wrong destination"));
            }
            if request.path != "/listener" {
                self.error(
                    &request,
                    "org.freedesktop.DBus.Error.UnknownObject",
                    "unknown object path",
                )?;
                continue;
            }
            let iface = request.interface.as_str();
            let mut data = Decoder::new(&request.body, request.little);
            match request.member.as_str() {
                "updateWindow" if iface.is_empty() || iface == SERVICE => {
                    let args = signature(&request.signature)?;
                    if args.len() != 3
                        || args[..2]
                            .iter()
                            .any(|ty| !matches!(ty, Type::Basic(b's') | Type::Variant))
                    {
                        self.error(
                            &request,
                            "org.freedesktop.DBus.Error.InvalidArgs",
                            "updateWindow requires title, class, and resource name",
                        )?;
                        continue;
                    }
                    let mut values = Vec::new();
                    for ty in &args[..2] {
                        let mut wrapped = matches!(ty, Type::Variant);
                        let mut string = true;
                        while wrapped {
                            match data.sig()? {
                                "v" => {}
                                "s" => wrapped = false,
                                _ => {
                                    string = false;
                                    break;
                                }
                            }
                        }
                        if !string {
                            break;
                        }
                        values.push(data.text()?);
                    }
                    if values.len() != 2 {
                        self.error(
                            &request,
                            "org.freedesktop.DBus.Error.InvalidArgs",
                            "updateWindow variants must contain strings",
                        )?;
                        continue;
                    }
                    // The source accepts an unconstrained third argument and ignores it.
                    data.skip(&args[2], 0)?;
                    data.finish()?;
                    match on_change(values[1], values[0]) {
                        Ok(()) => self.respond(&request, "", "", Vec::new())?,
                        Err(error) => {
                            self.error(
                                &request,
                                "org.freedesktop.DBus.Error.Failed",
                                &error.to_string(),
                            )?;
                            return Err(error);
                        }
                    }
                }
                "Introspect" if iface.is_empty() || iface == INTROSPECT => {
                    if request.signature.is_empty() {
                        self.respond(&request, "", "s", text_body(XML))?;
                    } else {
                        self.error(
                            &request,
                            "org.freedesktop.DBus.Error.InvalidArgs",
                            "Introspect takes no arguments",
                        )?;
                    }
                }
                "Ping" | "GetMachineId" if iface.is_empty() || iface == PEER => {
                    if !request.signature.is_empty() {
                        self.error(
                            &request,
                            "org.freedesktop.DBus.Error.InvalidArgs",
                            "Peer methods take no arguments",
                        )?;
                    } else if request.member == "Ping" {
                        self.respond(&request, "", "", Vec::new())?;
                    } else {
                        let machine_id = ["/var/lib/dbus/machine-id", "/etc/machine-id"]
                            .into_iter()
                            .filter_map(|path| {
                                let mut text = String::new();
                                fs::File::open(path)
                                    .ok()?
                                    .take(128)
                                    .read_to_string(&mut text)
                                    .ok()?;
                                Some(text)
                            })
                            .map(|text| text.trim().to_string())
                            .find(|text| {
                                text.len() == 32
                                    && text.bytes().all(|byte| byte.is_ascii_hexdigit())
                            });
                        if let Some(id) = machine_id {
                            self.respond(&request, "", "s", text_body(&id))?;
                        } else {
                            self.error(
                                &request,
                                "org.freedesktop.DBus.Error.Failed",
                                "machine ID not available",
                            )?;
                        }
                    }
                }
                _ => self.error(
                    &request,
                    "org.freedesktop.DBus.Error.UnknownMethod",
                    "unknown interface or method",
                )?,
            }
        }
    }
}
