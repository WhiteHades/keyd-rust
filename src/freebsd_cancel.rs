// FreeBSD shutdown delivery and wake ownership. See LICENSE.
use crate::freebsd::{self as os, PollFd, SignalAction, SignalSet};
use std::{
    io,
    os::fd::{AsFd, AsRawFd, OwnedFd},
    sync::{
        OnceLock,
        atomic::{AtomicI32, Ordering},
    },
};

static SHUTDOWN: AtomicI32 = AtomicI32::new(0);
// Never drained while workers run: every waiter must observe shutdown readiness.
static WAKE: OnceLock<OwnedFd> = OnceLock::new();
static WAKE_RAW: AtomicI32 = AtomicI32::new(-1);
const MASK: SignalSet = [(1 << (os::SIGINT - 1)) | (1 << (os::SIGTERM - 1)), 0, 0, 0];
const MAX_POLL_FDS: usize = 128;

extern "C" fn on_shutdown(signal: i32) {
    SHUTDOWN.store(signal, Ordering::Release);
    let fd = WAKE_RAW.load(Ordering::Acquire);
    if fd >= 0 {
        let one = 1u64;
        loop {
            // SAFETY: the published nonblocking eventfd remains owned for process lifetime.
            let result = unsafe {
                os::syscall6(
                    os::nr::WRITE,
                    [fd as usize, &one as *const _ as usize, 8, 0, 0, 0],
                )
            };
            // EAGAIN is already readable; retry only an interrupted signal-safe write.
            if result != -4 {
                break;
            }
        }
    }
}

/// Install/reset before workers start. Reinstallation requires every worker stopped.
pub(crate) fn install_signals() -> io::Result<()> {
    let previous = os::signal_mask(os::SIG_BLOCK, Some(&MASK))?;
    let result = (|| {
        if WAKE.get().is_none() {
            let _ = WAKE.set(os::eventfd(0)?);
        }
        let wake = WAKE.get().unwrap();
        WAKE_RAW.store(wake.as_raw_fd(), Ordering::Release);
        let mut counter = [0; 8];
        loop {
            match os::read(wake.as_fd(), &mut counter) {
                Ok(8) => continue,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) => return Err(error),
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "invalid shutdown eventfd read",
                    ));
                }
            }
        }
        SHUTDOWN.store(0, Ordering::Release);
        for (signal, handler) in [
            (os::SIGINT, on_shutdown as *const () as usize),
            (os::SIGTERM, on_shutdown as *const () as usize),
            (os::SIGPIPE, 1),
        ] {
            let action = SignalAction {
                handler,
                ..SignalAction::default()
            };
            // SAFETY: a permanent signal-safe one-argument Rust callback or SIG_IGN.
            // No SA_RESTART or userspace restorer: FreeBSD returns through its vDSO.
            unsafe { os::signal_action(signal, Some(&action)) }?;
        }
        Ok(())
    })();
    os::signal_mask(os::SIG_SETMASK, Some(&previous))?;
    result
}

pub(crate) fn shutdown_signal() -> i32 {
    SHUTDOWN.load(Ordering::Acquire)
}

/// Atomically wait with shutdown unblocked, restoring the caller's mask on return.
/// The persistent wake fd also cancels waiters when another thread takes the signal.
/// At most 127 caller descriptors are supported, as in the Linux runtime.
pub(crate) fn poll_interruptible(fds: &mut [PollFd], timeout_ms: i32) -> io::Result<usize> {
    if fds.len() >= MAX_POLL_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "interruptible poll supports at most 127 descriptors",
        ));
    }
    let mut pollfds = [PollFd {
        fd: -1,
        events: 0,
        revents: 0,
    }; MAX_POLL_FDS];
    pollfds[..fds.len()].copy_from_slice(fds);
    let wake = WAKE_RAW.load(Ordering::Acquire);
    let count = fds.len() + usize::from(wake >= 0);
    if wake >= 0 {
        pollfds[fds.len()] = PollFd {
            fd: wake,
            events: os::POLLIN,
            revents: 0,
        };
    }
    let previous = os::signal_mask(os::SIG_BLOCK, Some(&MASK))?;
    let mut wait = previous;
    wait[0] &= !MASK[0];
    let result = if shutdown_signal() != 0 {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        os::poll_masked(&mut pollfds[..count], timeout_ms, Some(&wait))
    };
    os::signal_mask(os::SIG_SETMASK, Some(&previous))?;
    if shutdown_signal() != 0 || pollfds[fds.len()].revents != 0 {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let count = result?;
    fds.copy_from_slice(&pollfds[..fds.len()]);
    Ok(count)
}
