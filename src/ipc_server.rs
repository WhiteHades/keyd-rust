// Rust port of keyd's IPC server lifecycle. © 2019 Raheman Vaiya. See LICENSE.

#[cfg(target_arch = "x86")]
use crate::linux::socketcall as socket_syscall;
#[cfg(not(target_arch = "x86"))]
use crate::linux::syscall6 as socket_syscall;
use crate::{linux, log};
use std::fs::{self, File, Permissions};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixListener;
use std::path::Path;

#[cfg(target_arch = "x86_64")]
const OPENAT: usize = 257;
#[cfg(target_arch = "aarch64")]
const OPENAT: usize = 56;
#[cfg(target_arch = "x86")]
const OPENAT: usize = 295;
#[cfg(target_arch = "x86_64")]
const SOCKET: usize = 41;
#[cfg(target_arch = "aarch64")]
const SOCKET: usize = 198;
#[cfg(target_arch = "x86_64")]
const BIND: usize = 49;
#[cfg(target_arch = "aarch64")]
const BIND: usize = 200;
#[cfg(target_arch = "x86_64")]
const LISTEN: usize = 50;
#[cfg(target_arch = "aarch64")]
const LISTEN: usize = 201;
#[cfg(target_arch = "x86")]
const SOCKET: usize = 1;
#[cfg(target_arch = "x86")]
const BIND: usize = 2;
#[cfg(target_arch = "x86")]
const LISTEN: usize = 4;

pub struct Server {
    pub listener: UnixListener,
    _lock: File,
}

pub fn bind(path: impl AsRef<Path>) -> io::Result<Server> {
    bind_with_setup(path, || Ok(()))
}

pub(crate) fn bind_with_setup(
    path: impl AsRef<Path>,
    setup: impl FnOnce() -> io::Result<()>,
) -> io::Result<Server> {
    let path = path.as_ref();
    let bytes = path.as_os_str().as_bytes();
    if bytes.is_empty() || bytes.len() > 107 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path must contain 1 to 107 bytes",
        ));
    }
    let mut lockpath = path.as_os_str().to_os_string();
    lockpath.push(".lock");
    let lockpath = linux::path_cstring(Path::new(&lockpath))?;
    // SAFETY: socket takes scalar arguments and returns a new owned descriptor.
    let socket = linux::syscall_result(unsafe {
        socket_syscall(SOCKET, [1, 1 | linux::O_CLOEXEC, 0, 0, 0, 0])
    })
    .map_err(|error| log::system_error("socket", error))
    // SAFETY: successful socket transfers unique descriptor ownership.
    .map(|fd| unsafe { OwnedFd::from_raw_fd(fd as i32) });
    // C creates the socket before group setup, but checks its error afterward.
    setup()?;
    let socket = socket?;
    // SAFETY: the terminated path lives through openat; O_RDONLY|O_CREAT needs mode 0600.
    let fd = linux::syscall_result(unsafe {
        linux::syscall6(
            OPENAT,
            [
                (-100_isize) as usize,
                lockpath.as_ptr() as usize,
                0x40,
                0o600,
                0,
                0,
            ],
        )
    })
    .map_err(|error| log::system_error("open", error))?;
    // SAFETY: successful openat returns a new, uniquely owned descriptor.
    let lock = File::from(unsafe { OwnedFd::from_raw_fd(fd as i32) });
    lock.try_lock()?;

    // The C server ignores unlink errors; bind reports an occupied path.
    let _ = fs::remove_file(path);
    let mut address = [0u8; 110];
    address[..2].copy_from_slice(&1u16.to_ne_bytes()); // AF_UNIX
    address[2..2 + bytes.len()].copy_from_slice(bytes);
    // SAFETY: the initialized sockaddr_un lives through bind, and socket is open.
    linux::syscall_result(unsafe {
        socket_syscall(
            BIND,
            [
                socket.as_raw_fd() as usize,
                address.as_ptr() as usize,
                address.len(),
                0,
                0,
                0,
            ],
        )
    })
    .map_err(|error| {
        log::diagnostic(io::Error::new(
            error.kind(),
            format!("failed to bind to socket {}", path.display()),
        ))
    })?;
    // SAFETY: listen takes only scalar arguments and the listener stays open.
    linux::syscall_result(unsafe {
        socket_syscall(LISTEN, [socket.as_raw_fd() as usize, 20, 0, 0, 0, 0])
    })
    .map_err(|error| log::system_error("listen", error))?;
    // The C server also ignores chmod errors and leaves both paths at exit.
    let _ = fs::set_permissions(path, Permissions::from_mode(0o660));
    Ok(Server {
        listener: UnixListener::from(socket),
        _lock: lock,
    })
}
