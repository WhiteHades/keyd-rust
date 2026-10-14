#[cfg(target_os = "freebsd")]
use crate::freebsd as os;
#[cfg(target_os = "linux")]
use crate::linux as os;
use os::{syscall_result, syscall6};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, BorrowedFd};

use super::{poll_interruptible, shutdown_signal};

#[cfg(target_os = "linux")]
const FCNTL: usize = super::nr::FCNTL;
#[cfg(target_os = "freebsd")]
const FCNTL: usize = os::nr::FCNTL;

/// Borrowed pollable I/O, with shutdown cancellation once install_signals runs.
/// File flags are shared by descriptor aliases: do not change them concurrently.
/// Drop restores the original flags; the descriptor itself stays open.
/// Use read/read_exact: std's read_to_end/read_to_string retry shutdown EINTR.
pub struct FdIo<'fd> {
    fd: BorrowedFd<'fd>,
    flags: usize,
    deadline: Option<std::time::Instant>,
}

impl<'fd> FdIo<'fd> {
    pub fn new(fd: BorrowedFd<'fd>) -> io::Result<Self> {
        // SAFETY: F_GETFL/F_SETFL use scalar arguments; fd stays borrowed and open.
        let flags =
            syscall_result(unsafe { syscall6(FCNTL, [fd.as_raw_fd() as usize, 3, 0, 0, 0, 0]) })?;
        syscall_result(unsafe {
            syscall6(
                FCNTL,
                [fd.as_raw_fd() as usize, 4, flags | os::O_NONBLOCK, 0, 0, 0],
            )
        })?;
        Ok(Self {
            fd,
            flags,
            deadline: None,
        })
    }

    pub fn with_deadline(fd: BorrowedFd<'fd>, deadline: std::time::Instant) -> io::Result<Self> {
        let mut io = Self::new(fd)?;
        io.deadline = Some(deadline);
        Ok(io)
    }

    fn check_deadline(&self) -> io::Result<()> {
        if self
            .deadline
            .is_some_and(|deadline| deadline <= std::time::Instant::now())
        {
            return Err(io::ErrorKind::TimedOut.into());
        }
        Ok(())
    }

    fn wait(&self, events: i16) -> io::Result<()> {
        let mut fds = [os::PollFd {
            fd: self.fd.as_raw_fd(),
            events,
            revents: 0,
        }];
        loop {
            self.check_deadline()?;
            let timeout = self.deadline.map_or(-1, |deadline| {
                (deadline
                    .saturating_duration_since(std::time::Instant::now())
                    .as_millis()
                    + 1)
                .min(i32::MAX as u128) as i32
            });
            match poll_interruptible(&mut fds, timeout) {
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted && shutdown_signal() == 0 =>
                {
                    continue;
                }
                Ok(0) if self.deadline.is_some() => return Err(io::ErrorKind::TimedOut.into()),
                result => return result.map(|_| ()),
            }
        }
    }
}

impl Drop for FdIo<'_> {
    fn drop(&mut self) {
        // SAFETY: restore saved scalar flags while the borrowed fd is still open.
        unsafe {
            syscall6(
                FCNTL,
                [self.fd.as_raw_fd() as usize, 4, self.flags, 0, 0, 0],
            );
        }
    }
}

impl Read for FdIo<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            if shutdown_signal() != 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.check_deadline()?;
            match os::read(self.fd, bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.wait(os::POLLIN)?,
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted && shutdown_signal() == 0 =>
                {
                    continue;
                }
                result => return result,
            }
        }
    }

    fn read_exact(&mut self, mut bytes: &mut [u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let n = self.read(bytes)?;
            if n == 0 {
                return Err(io::ErrorKind::UnexpectedEof.into());
            }
            bytes = &mut bytes[n..];
        }
        Ok(())
    }
}

impl Write for FdIo<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            if shutdown_signal() != 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            self.check_deadline()?;
            match os::write(self.fd, bytes) {
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => self.wait(4)?, // POLLOUT
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted && shutdown_signal() == 0 =>
                {
                    continue;
                }
                result => return result,
            }
        }
    }

    fn write_all(&mut self, mut bytes: &[u8]) -> io::Result<()> {
        while !bytes.is_empty() {
            let n = self.write(bytes)?;
            if n == 0 {
                return Err(io::ErrorKind::WriteZero.into());
            }
            bytes = &bytes[n..];
        }
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
