// Rust port of keyd's IPC wire contract. © 2019 Raheman Vaiya. See LICENSE.

use crate::{log, runtime};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::Path;

pub const MAX_MESSAGE_SIZE: usize = 4096;
pub const SOCKET_PATH: &str = crate::SOCKET_PATH;
pub const SUCCESS: u32 = 0;
pub const FAIL: u32 = 1;
pub const BIND: u32 = 2;
pub const INPUT: u32 = 3;
pub const MACRO: u32 = 4;
pub const RELOAD: u32 = 5;
pub const LAYER_LISTEN: u32 = 6;
pub const WIRE_SIZE: usize = 8 + MAX_MESSAGE_SIZE + size_of::<usize>();

pub struct Message {
    pub kind: u32,
    pub timeout: u32,
    pub data: [u8; MAX_MESSAGE_SIZE],
    pub len: usize,
}

impl Message {
    pub fn new(kind: u32, payload: &[u8], timeout: u32) -> io::Result<Self> {
        if payload.len() > MAX_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "maximum message size exceeded",
            ));
        }
        let mut message = Self {
            kind,
            timeout,
            data: [0; MAX_MESSAGE_SIZE],
            len: payload.len(),
        };
        message.data[..payload.len()].copy_from_slice(payload);
        Ok(message)
    }

    pub fn read(reader: &mut impl Read) -> io::Result<Self> {
        let mut bytes = [0u8; WIRE_SIZE];
        reader.read_exact(&mut bytes)?;
        let mut data = [0; MAX_MESSAGE_SIZE];
        data.copy_from_slice(&bytes[8..8 + MAX_MESSAGE_SIZE]);
        Ok(Self {
            kind: u32::from_ne_bytes(bytes[..4].try_into().unwrap()),
            timeout: u32::from_ne_bytes(bytes[4..8].try_into().unwrap()),
            data,
            len: usize::from_ne_bytes(bytes[8 + MAX_MESSAGE_SIZE..].try_into().unwrap()),
        })
    }

    pub fn write(&self, writer: &mut impl Write) -> io::Result<()> {
        if self.len > MAX_MESSAGE_SIZE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "maximum message size exceeded",
            ));
        }
        let mut bytes = [0u8; WIRE_SIZE];
        bytes[..4].copy_from_slice(&self.kind.to_ne_bytes());
        bytes[4..8].copy_from_slice(&self.timeout.to_ne_bytes());
        bytes[8..8 + MAX_MESSAGE_SIZE].copy_from_slice(&self.data);
        bytes[8 + MAX_MESSAGE_SIZE..].copy_from_slice(&self.len.to_ne_bytes());
        writer.write_all(&bytes)
    }

    pub fn validate_request(&self) -> Result<(), &'static str> {
        if self.len >= MAX_MESSAGE_SIZE {
            return Err("maximum message size exceeded");
        }
        if self.timeout > 1_000_000 {
            return Err("timeout cannot exceed 1000 ms");
        }
        Ok(())
    }

    pub fn payload(&self) -> io::Result<&[u8]> {
        self.data.get(..self.len).ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidData, "maximum message size exceeded")
        })
    }
}

pub fn connect(path: impl AsRef<Path>) -> io::Result<UnixStream> {
    runtime::connect_unix(path.as_ref()).map_err(|error| {
        if log::is_system_error(&error) {
            return error;
        }
        log::diagnostic(io::Error::new(error.kind(), format!(
            "ERROR: Failed to connect to \"{}\", make sure the daemon is running and you have permission to access the socket.",
            path.as_ref().display())))
    })
}

pub fn exec(
    path: impl AsRef<Path>,
    kind: u32,
    data: &[u8],
    timeout: u32,
    output: &mut impl Write,
) -> io::Result<bool> {
    let message = Message::new(kind, data, timeout)?;
    let stream = connect(path)?;
    let mut connection = runtime::FdIo::new(stream.as_fd())?;
    message.write(&mut connection)?;
    let reply = Message::read(&mut connection)?;
    let payload = reply.payload()?;
    if !payload.is_empty() {
        output.write_all(payload)?;
        output.write_all(b"\n")?;
    }
    Ok(reply.kind == FAIL)
}
