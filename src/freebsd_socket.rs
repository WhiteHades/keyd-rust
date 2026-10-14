// FreeBSD local stream socket ABI. See LICENSE.
use crate::freebsd::{syscall_result, syscall6};
use std::{
    io,
    os::fd::{AsRawFd, BorrowedFd, FromRawFd, OwnedFd},
    os::unix::ffi::OsStrExt,
    path::Path,
};

pub(crate) const SOCKET: usize = 97;
pub(crate) const CONNECT: usize = 98;
pub(crate) const BIND: usize = 104;
pub(crate) const LISTEN: usize = 106;
pub(crate) const GETSOCKOPT: usize = 118;
pub(crate) const SOCK_NONBLOCK: usize = 0x2000_0000;
pub(crate) const SOCK_CLOEXEC: usize = 0x1000_0000;
pub(crate) const SOL_SOCKET: usize = 0xffff;
pub(crate) const SO_ERROR: usize = 0x1007;
pub(crate) const ADDRESS_SIZE: usize = 106;
pub(crate) const PATH_SIZE: usize = 104;

pub(crate) fn address(path: &Path) -> io::Result<[u8; ADDRESS_SIZE]> {
    let path = path.as_os_str().as_bytes();
    if path.is_empty() || path.len() >= PATH_SIZE || path.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "socket path must contain 1 to 103 non-NUL bytes",
        ));
    }
    let mut address = [0; ADDRESS_SIZE];
    // Match frozen C's zero sun_len, one-byte AF_UNIX and full sockaddr_un size.
    address[1] = 1;
    address[2..2 + path.len()].copy_from_slice(path);
    Ok(address)
}

pub(crate) fn socket(nonblocking: bool) -> io::Result<OwnedFd> {
    let flags = 1 | SOCK_CLOEXEC | if nonblocking { SOCK_NONBLOCK } else { 0 };
    // SAFETY: AF_UNIX/SOCK_STREAM takes only scalar arguments and returns a new fd.
    let fd = syscall_result(unsafe { syscall6(SOCKET, [1, flags, 0, 0, 0, 0]) })?;
    // SAFETY: successful socket transfers one new uniquely owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

pub(crate) fn bind(fd: BorrowedFd<'_>, path: &Path) -> io::Result<()> {
    let address = address(path)?;
    // SAFETY: this initialized, length-checked sockaddr_un lives through bind.
    syscall_result(unsafe {
        syscall6(
            BIND,
            [
                fd.as_raw_fd() as usize,
                address.as_ptr() as usize,
                address.len(),
                0,
                0,
                0,
            ],
        )
    })
    .map(|_| ())
}

pub(crate) fn listen(fd: BorrowedFd<'_>, backlog: i32) -> io::Result<()> {
    // SAFETY: listen takes scalar arguments, and the descriptor remains borrowed.
    syscall_result(unsafe {
        syscall6(
            LISTEN,
            [fd.as_raw_fd() as usize, backlog as usize, 0, 0, 0, 0],
        )
    })
    .map(|_| ())
}

pub(crate) fn connect(fd: BorrowedFd<'_>, path: &Path) -> io::Result<()> {
    let address = address(path)?;
    // SAFETY: the initialized sockaddr_un lives through connect; flags belong to fd.
    // FreeBSD's full Unix backlog returns ECONNREFUSED, not Linux's EAGAIN.
    syscall_result(unsafe {
        syscall6(
            CONNECT,
            [
                fd.as_raw_fd() as usize,
                address.as_ptr() as usize,
                address.len(),
                0,
                0,
                0,
            ],
        )
    })
    .map(|_| ())
}

pub(crate) fn socket_error(fd: BorrowedFd<'_>) -> io::Result<i32> {
    let mut error = 0i32;
    let mut length = size_of::<i32>() as u32;
    // SAFETY: SO_ERROR writes a live aligned int and updates a four-byte socklen_t.
    syscall_result(unsafe {
        syscall6(
            GETSOCKOPT,
            [
                fd.as_raw_fd() as usize,
                SOL_SOCKET,
                SO_ERROR,
                &mut error as *mut _ as usize,
                &mut length as *mut _ as usize,
                0,
            ],
        )
    })?;
    if length != size_of::<i32>() as u32 {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(error)
}
