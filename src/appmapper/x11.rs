// Raw X11 consumer for the frozen application mapper. See LICENSE.
use crate::runtime::FdIo;
use std::collections::HashSet;
use std::io::{self, Read, Write};
use std::net::{IpAddr, TcpStream};
use std::os::fd::{AsFd, BorrowedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;

const MAX_PROPERTY: usize = 1024 * 1024;
const MAX_WINDOWS: usize = 65536;
const EVENT_MASK: u32 = (1 << 19) | (1 << 22);
const COOKIE: &[u8] = b"MIT-MAGIC-COOKIE-1";

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(bytes[at..at + 2].try_into().unwrap())
}

fn u32_at(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
}

fn put32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn padded(length: usize) -> usize {
    (length + 3) & !3
}

enum Stream {
    Unix(UnixStream),
    Tcp(TcpStream),
}

impl AsFd for Stream {
    fn as_fd(&self) -> BorrowedFd<'_> {
        match self {
            Self::Unix(stream) => stream.as_fd(),
            Self::Tcp(stream) => stream.as_fd(),
        }
    }
}

struct Display {
    host: String,
    number: u16,
    screen: usize,
    local: bool,
    tcp_fallback: bool,
}

impl Display {
    fn parse(value: &str) -> io::Result<Self> {
        let (protocol, value) = match value.split_once('/') {
            Some((protocol @ ("unix" | "tcp"), value)) => (Some(protocol), value),
            Some(_) => return Err(invalid("unsupported X11 DISPLAY protocol")),
            None => (None, value),
        };
        let (host, suffix) = value
            .rsplit_once(':')
            .ok_or_else(|| invalid("invalid X11 DISPLAY"))?;
        let (number, screen) = suffix.split_once('.').unwrap_or((suffix, "0"));
        if number.is_empty()
            || screen.is_empty()
            || !number.bytes().all(|b| b.is_ascii_digit())
            || !screen.bytes().all(|b| b.is_ascii_digit())
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-:._[]".contains(&b))
            || protocol == Some("tcp") && host.is_empty()
        {
            return Err(invalid("invalid X11 DISPLAY"));
        }
        let number = number
            .parse()
            .map_err(|_| invalid("invalid X11 display number"))?;
        let screen = screen
            .parse()
            .map_err(|_| invalid("invalid X11 screen number"))?;
        Ok(Self {
            host: host
                .trim_start_matches('[')
                .trim_end_matches(']')
                .to_owned(),
            number,
            screen,
            local: protocol == Some("unix") || host.is_empty() || host == "unix",
            tcp_fallback: protocol.is_none() && host.is_empty(),
        })
    }

    fn tcp(&self) -> io::Result<TcpStream> {
        if crate::runtime::shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let port = 6000u16
            .checked_add(self.number)
            .ok_or_else(|| invalid("X11 TCP port is out of range"))?;
        let host = if self.host.is_empty() {
            "127.0.0.1"
        } else {
            &self.host
        };
        TcpStream::connect((host, port))
    }

    fn connect(&self) -> io::Result<Stream> {
        if !self.local {
            return self.tcp().map(Stream::Tcp);
        }
        let path = format!("/tmp/.X11-unix/X{}", self.number);
        let stream = crate::runtime::connect_unix(std::path::Path::new(&path))
            .or_else(|_| crate::runtime::connect_unix_abstract(path.as_bytes()));
        match stream {
            Ok(stream) => Ok(Stream::Unix(stream)),
            Err(_) if self.tcp_fallback => self.tcp().map(Stream::Tcp),
            Err(error) => Err(error),
        }
    }
}

fn auth_field<'a>(bytes: &mut &'a [u8]) -> io::Result<&'a [u8]> {
    if bytes.len() < 2 {
        return Err(invalid("truncated Xauthority record"));
    }
    let length = usize::from(u16::from_be_bytes(bytes[..2].try_into().unwrap()));
    *bytes = &bytes[2..];
    if length > bytes.len() {
        return Err(invalid("truncated Xauthority field"));
    }
    let (field, rest) = bytes.split_at(length);
    *bytes = rest;
    Ok(field)
}

fn find_cookie(mut bytes: &[u8], family: u16, address: &[u8], display: u16) -> io::Result<Vec<u8>> {
    let number = display.to_string();
    let mut cookie = Vec::new();
    let mut exact = false;
    while !bytes.is_empty() {
        if bytes.len() < 2 {
            return Err(invalid("truncated Xauthority family"));
        }
        let entry_family = u16::from_be_bytes(bytes[..2].try_into().unwrap());
        bytes = &bytes[2..];
        let entry_address = auth_field(&mut bytes)?;
        let entry_number = auth_field(&mut bytes)?;
        let name = auth_field(&mut bytes)?;
        let data = auth_field(&mut bytes)?;
        let matching_address = entry_family == family && entry_address == address;
        if (matching_address || entry_family == 65535)
            && (entry_number == number.as_bytes() || entry_number.is_empty() && cookie.is_empty())
            && name == COOKIE
            && (!exact || matching_address)
        {
            if data.len() != 16 {
                return Err(invalid("invalid MIT-MAGIC-COOKIE-1 length"));
            }
            cookie = data.to_vec();
            exact = matching_address;
        }
    }
    Ok(cookie)
}

fn authorization(stream: &Stream, number: u16) -> io::Result<Vec<u8>> {
    let path = std::env::var_os("XAUTHORITY")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".Xauthority")));
    let Some(path) = path else {
        return Ok(Vec::new());
    };
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(Vec::new()), // X servers may permit unauthenticated connections.
    };
    let mut bytes = Vec::new();
    file.take((MAX_PROPERTY + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_PROPERTY {
        return Err(invalid("Xauthority file exceeds 1 MiB"));
    }
    if let Stream::Tcp(stream) = stream {
        let ip = stream.peer_addr()?.ip();
        let (family, address) = match ip {
            IpAddr::V4(ip) => (0, ip.octets().to_vec()),
            IpAddr::V6(ip) => (6, ip.octets().to_vec()),
        };
        let cookie = find_cookie(&bytes, family, &address, number)?;
        if !cookie.is_empty() || !ip.is_loopback() {
            return Ok(cookie);
        }
    }
    let hostname = std::fs::read("/proc/sys/kernel/hostname")?;
    let hostname = hostname.strip_suffix(b"\n").unwrap_or(&hostname);
    find_cookie(&bytes, 256, hostname, number)
}

#[derive(Debug)]
struct XError(u8);

impl std::fmt::Display for XError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "X11 error {}", self.0)
    }
}
impl std::error::Error for XError {}

fn window_error(error: &io::Error) -> bool {
    error.get_ref().is_some_and(|error| error.is::<XError>())
}

struct Property {
    format: u8,
    data: Vec<u8>,
}

pub struct Monitor {
    stream: Stream,
    root: u32,
    max_request: usize,
    sequence: u16,
    void_sequence: Option<u16>,
    events: usize,
    atoms: [u32; 7],
}

impl Monitor {
    pub fn connect() -> io::Result<Self> {
        let display = std::env::var("DISPLAY")
            .map_err(|_| io::Error::new(io::ErrorKind::NotFound, "DISPLAY is not set"))?;
        let display = Display::parse(&display)?;
        let stream = display.connect()?;
        let cookie = authorization(&stream, display.number)?;
        Self::setup(
            stream,
            display.screen,
            if cookie.is_empty() { &[] } else { COOKIE },
            &cookie,
        )
    }

    /// A connected synthetic peer can exercise the same setup without host X state.
    pub fn from_stream(
        stream: UnixStream,
        screen: usize,
        auth_name: &[u8],
        auth_data: &[u8],
    ) -> io::Result<Self> {
        Self::setup(Stream::Unix(stream), screen, auth_name, auth_data)
    }

    fn setup(stream: Stream, screen: usize, name: &[u8], data: &[u8]) -> io::Result<Self> {
        if (!name.is_empty() && (name != COOKIE || data.len() != 16))
            || name.is_empty() && !data.is_empty()
        {
            return Err(invalid("unsupported X11 authentication"));
        }
        let mut request = vec![0; 12 + padded(name.len()) + padded(data.len())];
        request[0] = b'l';
        request[2..4].copy_from_slice(&11u16.to_le_bytes());
        request[6..8].copy_from_slice(&(name.len() as u16).to_le_bytes());
        request[8..10].copy_from_slice(&(data.len() as u16).to_le_bytes());
        request[12..12 + name.len()].copy_from_slice(name);
        let data_at = 12 + padded(name.len());
        request[data_at..data_at + data.len()].copy_from_slice(data);
        let mut io = FdIo::new(stream.as_fd())?;
        io.write_all(&request)?;
        let mut header = [0; 8];
        io.read_exact(&mut header)?;
        let mut body = vec![0; usize::from(u16_at(&header, 6)) * 4];
        io.read_exact(&mut body)?;
        if header[0] != 1 {
            if !matches!(header[0], 0 | 2) || usize::from(header[1]) > body.len() {
                return Err(invalid("malformed X11 setup failure"));
            }
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "X11 server rejected setup or authentication",
            ));
        }
        if u16_at(&header, 2) != 11 || u16_at(&header, 4) != 0 || body.len() < 32 {
            return Err(invalid("invalid X11 setup version or length"));
        }
        let max_request = usize::from(u16_at(&body, 18)) * 4;
        if max_request < 48 || screen >= usize::from(body[20]) {
            return Err(invalid("invalid X11 request limit or screen"));
        }
        let mut at = 32 + padded(usize::from(u16_at(&body, 16))) + usize::from(body[21]) * 8;
        let mut root = 0;
        for index in 0..usize::from(body[20]) {
            let record = body
                .get(at..at + 40)
                .ok_or_else(|| invalid("truncated X11 screen"))?;
            if index == screen {
                root = u32_at(record, 0);
            }
            let depths = record[39];
            at += 40;
            for _ in 0..depths {
                let depth = body
                    .get(at..at + 8)
                    .ok_or_else(|| invalid("truncated X11 depth"))?;
                at += 8 + usize::from(u16_at(depth, 2)) * 24;
                if at > body.len() {
                    return Err(invalid("truncated X11 visuals"));
                }
            }
        }
        if root <= 1 || at != body.len() {
            return Err(invalid("invalid X11 root or setup body"));
        }
        drop(io);
        Ok(Self {
            stream,
            root,
            max_request,
            sequence: 0,
            void_sequence: None,
            events: 0,
            atoms: [0; 7],
        })
    }

    fn send(&mut self, mut request: Vec<u8>) -> io::Result<u16> {
        if !request.len().is_multiple_of(4) || request.len() > self.max_request {
            return Err(invalid("invalid X11 request length"));
        }
        let words = (request.len() / 4) as u16;
        request[2..4].copy_from_slice(&words.to_le_bytes());
        FdIo::new(self.stream.as_fd())?.write_all(&request)?;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(self.sequence)
    }

    fn packet(&mut self) -> io::Result<Vec<u8>> {
        let mut bytes = vec![0; 32];
        let mut io = FdIo::new(self.stream.as_fd())?;
        io.read_exact(&mut bytes)?;
        let kind = bytes[0] & 127;
        if bytes[0] == 128 || bytes[0] == 129 || kind == 0 && bytes[1] == 0 {
            return Err(invalid("invalid X11 packet code"));
        }
        if kind == 1 || kind == 35 {
            let words = u32_at(&bytes, 4);
            if words > (MAX_PROPERTY / 4) as u32 {
                return Err(invalid("X11 packet exceeds 1 MiB"));
            }
            bytes.resize(32 + words as usize * 4, 0);
            io.read_exact(&mut bytes[32..])?;
        }
        Ok(bytes)
    }

    fn reply(&mut self, request: Vec<u8>) -> io::Result<Vec<u8>> {
        let sequence = self.send(request)?;
        let mut deferred_error = None;
        loop {
            let packet = self.packet()?;
            match packet[0] & 127 {
                0 => {
                    let error_sequence = u16_at(&packet, 2);
                    if error_sequence == sequence {
                        self.void_sequence = None;
                        return Err(io::Error::other(XError(packet[1])));
                    }
                    if Some(error_sequence) != self.void_sequence {
                        return Err(invalid("unexpected X11 error sequence"));
                    }
                    deferred_error = Some(packet[1]);
                }
                1 => {
                    if u16_at(&packet, 2) != sequence {
                        return Err(invalid("unexpected X11 reply sequence"));
                    }
                    self.void_sequence = None;
                    return match deferred_error {
                        Some(code) => Err(io::Error::other(XError(code))),
                        None => Ok(packet),
                    };
                }
                _ => {
                    self.events += 1;
                    if self.events > MAX_WINDOWS {
                        return Err(invalid("too many queued X11 events"));
                    }
                }
            }
        }
    }

    fn masks(&mut self, window: u32) -> io::Result<()> {
        let mut request = vec![2, 0, 0, 0];
        request.extend_from_slice(&window.to_le_bytes());
        request.extend_from_slice(&(1u32 << 11).to_le_bytes());
        request.extend_from_slice(&EVENT_MASK.to_le_bytes());
        self.void_sequence = Some(self.send(request)?);
        Ok(())
    }

    pub fn init(&mut self) -> io::Result<()> {
        self.masks(self.root)?;
        for (index, name) in [
            "_NET_WM_NAME",
            "WM_NAME",
            "WM_CLASS",
            "_NET_WM_STATE",
            "_NET_WM_STATE_ABOVE",
            "_NET_WM_WINDOW_TYPE_NOTIFICATION",
            "_NET_WM_WINDOW_TYPE",
        ]
        .iter()
        .enumerate()
        {
            let mut request = vec![0; 8 + padded(name.len())];
            request[0] = 16;
            request[4..6].copy_from_slice(&(name.len() as u16).to_le_bytes());
            request[8..8 + name.len()].copy_from_slice(name.as_bytes());
            let reply = self.reply(request)?;
            if reply.len() != 32 || u32_at(&reply, 8) == 0 {
                return Err(invalid("invalid X11 atom reply"));
            }
            self.atoms[index] = u32_at(&reply, 8);
        }
        Ok(())
    }

    fn tree(&mut self, window: u32) -> io::Result<(u32, Vec<u32>)> {
        let mut request = vec![15, 0, 0, 0];
        request.extend_from_slice(&window.to_le_bytes());
        let reply = self.reply(request)?;
        let count = usize::from(u16_at(&reply, 16));
        if reply.len() != 32 + count * 4 {
            return Err(invalid("invalid X11 QueryTree length"));
        }
        Ok((
            u32_at(&reply, 12),
            reply[32..]
                .as_chunks::<4>()
                .0
                .iter()
                .map(|v| u32_at(v, 0))
                .collect(),
        ))
    }

    fn property(&mut self, window: u32, atom: u32, kind: u32) -> io::Result<Property> {
        let mut request = vec![0; 24];
        request[0] = 20;
        put32(&mut request, 4, window);
        put32(&mut request, 8, atom);
        put32(&mut request, 12, kind);
        put32(&mut request, 20, (MAX_PROPERTY / 4) as u32);
        let reply = self.reply(request)?;
        let format = reply[1];
        let property_type = u32_at(&reply, 8);
        let count = u32_at(&reply, 16) as usize;
        let item_size = match format {
            0 if count == 0 && (property_type == 0 || kind != 0 && property_type != kind) => 0,
            8 if property_type != 0 => 1,
            16 if property_type != 0 => 2,
            32 if property_type != 0 => 4,
            _ => return Err(invalid("invalid X11 property format")),
        };
        let length = count
            .checked_mul(item_size)
            .filter(|&n| n <= MAX_PROPERTY)
            .ok_or_else(|| invalid("X11 property exceeds 1 MiB"))?;
        if reply.len() != 32 + padded(length) {
            return Err(invalid("invalid X11 property length"));
        }
        // A mismatched requested type has bytes_after but no returned value.
        if kind != 0 && property_type != 0 && property_type != kind {
            if count != 0 {
                return Err(invalid("invalid X11 mismatched property type"));
            }
            return Ok(Property {
                format: 0,
                data: Vec::new(),
            });
        }
        if u32_at(&reply, 12) != 0 {
            return Err(invalid("X11 property exceeds requested bound"));
        }
        Ok(Property {
            format,
            data: reply[32..32 + length].to_vec(),
        })
    }

    fn active(&mut self) -> io::Result<u32> {
        let mut stack = vec![self.root];
        let mut visited = HashSet::new();
        while let Some(window) = stack.pop() {
            if !visited.insert(window) || visited.len() > MAX_WINDOWS {
                return Err(invalid("cyclic or oversized X11 window tree"));
            }
            let (_, children) = self.tree(window)?;
            if stack.len() + children.len() > MAX_WINDOWS {
                return Err(invalid("oversized X11 window tree"));
            }
            stack.extend(children);
            let state = self.property(window, self.atoms[3], 4)?;
            if state.format == 32
                && !state.data.is_empty()
                && u32_at(&state.data, 0) == self.atoms[4]
            {
                let types = self.property(window, self.atoms[6], 4)?;
                if types.format != 32
                    || !types
                        .data
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .any(|v| u32_at(v, 0) == self.atoms[5])
                {
                    return Ok(window);
                }
            }
        }
        let reply = self.reply(vec![43, 0, 0, 0])?;
        if reply.len() != 32 {
            return Err(invalid("invalid X11 input focus reply"));
        }
        Ok(u32_at(&reply, 8))
    }

    fn title(&mut self, window: u32) -> io::Result<String> {
        match self.property(window, self.atoms[0], 0) {
            Ok(property) if property.format == 8 => {
                if let Ok(title) = String::from_utf8(property.data) {
                    return Ok(title);
                }
            }
            Ok(_) => (),
            Err(error) if window_error(&error) => (),
            Err(error) => return Err(error),
        }
        match self.property(window, self.atoms[1], 0) {
            Ok(property) if property.format == 8 => {
                Ok(property.data.into_iter().map(char::from).collect())
            }
            Ok(_) => Ok(String::new()),
            Err(error) if window_error(&error) => Ok(String::new()),
            Err(error) => Err(error),
        }
    }

    fn info(&mut self, mut window: u32) -> io::Result<(String, String)> {
        let mut visited = HashSet::new();
        while window != 0 {
            if !visited.insert(window) || visited.len() > MAX_WINDOWS {
                return Err(invalid("cyclic or oversized X11 parent chain"));
            }
            let property = self.property(window, self.atoms[2], 31)?;
            if property.format == 8
                && let Some(class) = property.data.split(|&byte| byte == 0).nth(1)
            {
                let class = class.iter().copied().map(char::from).collect();
                return Ok((class, self.title(window)?));
            }
            window = self.tree(window)?.0;
        }
        Ok(("root".to_owned(), String::new()))
    }

    pub fn run(
        &mut self,
        mut on_change: impl FnMut(&str, &str) -> io::Result<()>,
        mut wait: impl FnMut(BorrowedFd<'_>) -> io::Result<bool>,
    ) -> io::Result<()> {
        if self.atoms.contains(&0) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "X11 monitor is not initialized",
            ));
        }
        let mut last = (String::new(), String::new());
        loop {
            if self.events == 0 {
                if wait(self.stream.as_fd())? {
                    on_change(&last.0, &last.1)?;
                    continue;
                }
                let packet = self.packet()?;
                match packet[0] & 127 {
                    0 => continue,
                    1 => return Err(invalid("unsolicited X11 reply")),
                    _ => self.events = 1,
                }
            }
            self.events -= 1;
            let info = (|| {
                let window = self.active()?;
                if window <= 1 {
                    return Ok(None);
                }
                self.masks(window)?;
                self.info(window).map(Some)
            })();
            match info {
                Ok(Some(info)) if info != last => {
                    last = info;
                    on_change(&last.0, &last.1)?;
                }
                Ok(_) => (),
                Err(error) if window_error(&error) => (),
                Err(error) => return Err(error),
            }
        }
    }
}
