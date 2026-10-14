// keyd © 2019 Raheman Vaiya. FreeBSD descriptor ABI port. See LICENSE.
#[cfg(not(all(
    target_os = "freebsd",
    target_arch = "x86_64",
    target_pointer_width = "64",
    target_endian = "little"
)))]
compile_error!("this descriptor boundary requires FreeBSD amd64");

use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

pub(crate) mod nr {
    pub const READ: usize = 3;
    pub const WRITE: usize = 4;
    pub const CLOSE: usize = 6;
    pub const FCNTL: usize = 92;
    pub const OPENAT: usize = 499;
    pub const PPOLL: usize = 545;
}

pub(crate) const O_WRONLY: usize = 1;
pub(crate) const O_RDWR: usize = 2;
pub(crate) const O_NONBLOCK: usize = 4;
pub(crate) const O_CLOEXEC: usize = 0x100000;

/// # Safety
/// The caller supplies the selected syscall's live, correctly aligned arguments.
pub(crate) unsafe fn syscall6(number: usize, args: [usize; 6]) -> isize {
    let result: isize;
    let failed: u8;
    // SAFETY: FreeBSD amd64 uses these registers and reports errno through CF.
    // rdx may hold a second return value, so it is also declared clobbered.
    unsafe {
        core::arch::asm!(
            "syscall", "setc {failed}",
            inlateout("rax") number => result,
            in("rdi") args[0], in("rsi") args[1],
            inlateout("rdx") args[2] => _,
            in("r10") args[3], in("r8") args[4], in("r9") args[5],
            failed = lateout(reg_byte) failed,
            lateout("rcx") _, lateout("r11") _, options(nostack)
        );
    }
    if failed == 0 { result } else { -result }
}

pub(crate) fn syscall_result(result: isize) -> io::Result<usize> {
    if result < 0 {
        Err(io::Error::from_raw_os_error(-result as i32))
    } else {
        Ok(result as usize)
    }
}

pub(crate) fn open(path: &Path, flags: usize) -> io::Result<OwnedFd> {
    if flags & !(O_WRONLY | O_RDWR | O_NONBLOCK | O_CLOEXEC) != 0 || flags & 3 == 3 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    // SAFETY: the terminated path lives through openat; creation is not requested.
    let fd = syscall_result(unsafe {
        syscall6(
            nr::OPENAT,
            [
                (-100_isize) as usize,
                path.as_ptr() as usize,
                flags | O_CLOEXEC,
                0,
                0,
                0,
            ],
        )
    })?;
    // SAFETY: successful openat returns a new uniquely owned descriptor.
    Ok(unsafe { OwnedFd::from_raw_fd(fd as i32) })
}

pub(crate) fn read(fd: BorrowedFd<'_>, bytes: &mut [u8]) -> io::Result<usize> {
    // SAFETY: the descriptor and writable slice live through read.
    syscall_result(unsafe {
        syscall6(
            nr::READ,
            [
                fd.as_raw_fd() as usize,
                bytes.as_mut_ptr() as usize,
                bytes.len(),
                0,
                0,
                0,
            ],
        )
    })
}

pub(crate) fn write(fd: BorrowedFd<'_>, bytes: &[u8]) -> io::Result<usize> {
    // SAFETY: the descriptor and initialized slice live through write.
    syscall_result(unsafe {
        syscall6(
            nr::WRITE,
            [
                fd.as_raw_fd() as usize,
                bytes.as_ptr() as usize,
                bytes.len(),
                0,
                0,
                0,
            ],
        )
    })
}

pub(crate) fn write_all(fd: BorrowedFd<'_>, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        match write(fd, bytes) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => bytes = &bytes[n..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

pub(crate) fn close(fd: OwnedFd) -> io::Result<()> {
    // SAFETY: consume unique ownership once; close is never retried after EINTR.
    syscall_result(unsafe { syscall6(nr::CLOSE, [fd.into_raw_fd() as usize, 0, 0, 0, 0, 0]) })
        .map(|_| ())
}

pub(crate) fn descriptor_flags(fd: BorrowedFd<'_>) -> io::Result<usize> {
    // SAFETY: F_GETFD takes a scalar descriptor and no pointer argument.
    syscall_result(unsafe { syscall6(nr::FCNTL, [fd.as_raw_fd() as usize, 1, 0, 0, 0, 0]) })
}

pub(crate) fn status_flags(fd: BorrowedFd<'_>) -> io::Result<usize> {
    // SAFETY: F_GETFL takes a scalar descriptor and no pointer argument.
    syscall_result(unsafe { syscall6(nr::FCNTL, [fd.as_raw_fd() as usize, 3, 0, 0, 0, 0]) })
}

pub(crate) fn set_nonblocking(fd: BorrowedFd<'_>, enabled: bool) -> io::Result<()> {
    let flags = status_flags(fd)?;
    let flags = if enabled {
        flags | O_NONBLOCK
    } else {
        flags & !O_NONBLOCK
    };
    // SAFETY: F_SETFL takes only scalar flags, retaining all unrelated bits.
    syscall_result(unsafe { syscall6(nr::FCNTL, [fd.as_raw_fd() as usize, 4, flags, 0, 0, 0]) })
        .map(|_| ())
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PollFd {
    pub fd: i32,
    pub events: i16,
    pub revents: i16,
}

pub(crate) const POLLIN: i16 = 1;
pub(crate) const POLLOUT: i16 = 4;
pub(crate) const POLLERR: i16 = 8;
pub(crate) const POLLHUP: i16 = 16;
pub(crate) const POLLNVAL: i16 = 32;

pub(crate) fn poll(fds: &mut [PollFd], timeout_ms: i32) -> io::Result<usize> {
    let count = u32::try_from(fds.len()).map_err(|_| io::ErrorKind::InvalidInput)?;
    let timeout = [
        timeout_ms as isize / 1000,
        timeout_ms as isize % 1000 * 1_000_000,
    ];
    let timeout_ptr = if timeout_ms < 0 {
        0
    } else {
        timeout.as_ptr() as usize
    };
    // SAFETY: pollfd and timespec have the FreeBSD amd64 ABI and live storage.
    // FreeBSD ppoll takes four arguments, without Linux's signal-mask-size word.
    syscall_result(unsafe {
        syscall6(
            nr::PPOLL,
            [
                fds.as_mut_ptr() as usize,
                count as usize,
                timeout_ptr,
                0,
                0,
                0,
            ],
        )
    })
}
