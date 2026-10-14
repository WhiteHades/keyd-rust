// Rust port of keyd's process setup and command execution. See LICENSE.

#[cfg(target_arch = "x86")]
use crate::linux::socketcall as socket_syscall;
#[cfg(not(target_arch = "x86"))]
use crate::linux::syscall6 as socket_syscall;
use crate::linux::{self, syscall_result, syscall6};
use std::ffi::{CString, OsStr};
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

#[path = "nss.rs"]
mod nss;

#[path = "fd_io.rs"]
mod fd_io;
pub use fd_io::FdIo;

#[cfg(target_arch = "x86_64")]
mod nr {
    pub const SIGACTION: usize = 13;
    pub const SIGPROCMASK: usize = 14;
    pub const PPOLL: usize = 271;
    pub const EVENTFD2: usize = 290;
    pub const SOCKET: usize = 41;
    pub const CONNECT: usize = 42;
    pub const FCNTL: usize = 72;
    pub const NANOSLEEP: usize = 35;
    pub const FORK: usize = 57;
    pub const EXECVE: usize = 59;
    pub const EXIT: usize = 60;
    pub const WAIT4: usize = 61;
    pub const SETGID: usize = 106;
    pub const SCHED_SETSCHEDULER: usize = 144;
    pub const MLOCKALL: usize = 151;
    pub const OPENAT: usize = 257;
    pub const DUP3: usize = 292;
    pub const CLOSE: usize = 3;
    pub const WRITE: usize = 1;
}

#[cfg(target_arch = "aarch64")]
mod nr {
    pub const SIGACTION: usize = 134;
    pub const SIGPROCMASK: usize = 135;
    pub const PPOLL: usize = 73;
    pub const EVENTFD2: usize = 19;
    pub const SOCKET: usize = 198;
    pub const CONNECT: usize = 203;
    pub const FCNTL: usize = 25;
    pub const NANOSLEEP: usize = 101;
    pub const FORK: usize = 220; // clone(SIGCHLD, NULL), with no shared resources.
    pub const EXECVE: usize = 221;
    pub const EXIT: usize = 93;
    pub const WAIT4: usize = 260;
    pub const SETGID: usize = 144;
    pub const SCHED_SETSCHEDULER: usize = 119;
    pub const MLOCKALL: usize = 230;
    pub const OPENAT: usize = 56;
    pub const DUP3: usize = 24;
    pub const CLOSE: usize = 57;
    pub const WRITE: usize = 64;
}

#[cfg(target_arch = "x86")]
mod nr {
    pub const SIGACTION: usize = 174;
    pub const SIGPROCMASK: usize = 175;
    pub const PPOLL: usize = 309;
    pub const EVENTFD2: usize = 328;
    pub const SOCKET: usize = 1; // socketcall operation.
    pub const CONNECT: usize = 3; // socketcall operation.
    pub const FCNTL: usize = 55;
    pub const NANOSLEEP: usize = 162;
    pub const FORK: usize = 2;
    pub const EXECVE: usize = 11;
    pub const EXIT: usize = 1;
    pub const WAIT4: usize = 114;
    pub const SETGID: usize = 214;
    pub const SCHED_SETSCHEDULER: usize = 156;
    pub const MLOCKALL: usize = 152;
    pub const OPENAT: usize = 295;
    pub const DUP3: usize = 330;
    pub const CLOSE: usize = 6;
    pub const WRITE: usize = 4;
}

static SHUTDOWN: AtomicI32 = AtomicI32::new(0);
static STDOUT_WAS_CLOSED: AtomicBool = AtomicBool::new(false);

extern "C" fn remember_stdout() {
    // SAFETY: the ELF initializer uses only scalar, signal-safe kernel operations.
    let closed = unsafe { syscall6(nr::FCNTL, [1, 1, 0, 0, 0, 0]) } == -9;
    STDOUT_WAS_CLOSED.store(closed, Ordering::Relaxed);
}

// Rust opens /dev/null for closed stdio before main; retain monitor's C fstat failure.
#[used]
#[unsafe(link_section = ".init_array")]
static REMEMBER_STDOUT: extern "C" fn() = remember_stdout;

pub fn original_stdout_closed() -> bool {
    STDOUT_WAS_CLOSED.load(Ordering::Relaxed)
}
// One process-lifetime owner; exec closes the descriptor, polling never drains it.
static SHUTDOWN_WAKE: OnceLock<OwnedFd> = OnceLock::new();
static SHUTDOWN_WAKE_RAW: AtomicI32 = AtomicI32::new(-1);
const SHUTDOWN_MASK: u64 = (1u64 << (2 - 1)) | (1u64 << (15 - 1));
const MAX_POLL_FDS: usize = 128;

extern "C" fn on_shutdown(signal: i32) {
    SHUTDOWN.store(signal, Ordering::Release);
    let fd = SHUTDOWN_WAKE_RAW.load(Ordering::Acquire);
    if fd >= 0 {
        let counter = 1u64;
        loop {
            // SAFETY: the published nonblocking eventfd stays owned for the
            // process lifetime; raw write of eight live bytes is signal-safe.
            let result = unsafe {
                syscall6(
                    nr::WRITE,
                    [fd as usize, &counter as *const _ as usize, 8, 0, 0, 0],
                )
            };
            // EINTR retries only the signal-safe write; EAGAIN is already readable.
            if result != -4 {
                break;
            }
        }
    }
}

#[unsafe(naked)]
unsafe extern "C" fn restore_signal() {
    #[cfg(target_arch = "x86")]
    // One-argument handlers lack SA_SIGINFO, so i386 delivers the legacy frame.
    core::arch::naked_asm!("pop eax", "mov eax, 119", "int 0x80");
    #[cfg(target_arch = "x86_64")]
    core::arch::naked_asm!("mov rax, 15", "syscall");
    #[cfg(target_arch = "aarch64")]
    core::arch::naked_asm!("mov x8, #139", "svc #0");
}

#[repr(C)]
struct SignalAction {
    handler: usize,
    flags: usize,
    restorer: usize,
    mask: u64,
}

/// Install/reset before starting workers and use poll_interruptible for waits.
/// Reinstallation requires all workers stopped, since it drains the wake counter.
/// On EINTR, check shutdown_signal, stop pending macro/input work, drop device
/// resources, and return that signal.
pub fn install_signals() -> io::Result<()> {
    let mut previous_mask = 0u64;
    // SAFETY: block shutdown through reset/installation using live kernel masks.
    syscall_result(unsafe {
        syscall6(
            nr::SIGPROCMASK,
            [
                0,
                &SHUTDOWN_MASK as *const _ as usize,
                &mut previous_mask as *mut _ as usize,
                8,
                0,
                0,
            ],
        )
    })?;
    let result = (|| {
        if SHUTDOWN_WAKE.get().is_none() {
            // SAFETY: eventfd2 takes scalar count/flags and returns a new descriptor.
            let fd = syscall_result(unsafe {
                syscall6(
                    nr::EVENTFD2,
                    [0, linux::O_NONBLOCK | linux::O_CLOEXEC, 0, 0, 0, 0],
                )
            })?;
            // SAFETY: successful eventfd2 transfers unique descriptor ownership.
            let owned = unsafe { OwnedFd::from_raw_fd(fd as i32) };
            let _ = SHUTDOWN_WAKE.set(owned);
        }
        let wake = SHUTDOWN_WAKE.get().unwrap();
        SHUTDOWN_WAKE_RAW.store(wake.as_raw_fd(), Ordering::Release);
        let mut counter = [0; 8];
        loop {
            match linux::read(wake.as_fd(), &mut counter) {
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
        let shutdown_handler = on_shutdown as *const () as usize;
        for (signal, handler) in [(15, shutdown_handler), (2, shutdown_handler), (13, 1)] {
            let action = SignalAction {
                handler,
                // No SA_RESTART: blocking kernel calls must return control for cleanup.
                flags: 0x0400_0000, // SA_RESTORER on the supported Linux architectures.
                restorer: restore_signal as *const () as usize,
                mask: 0,
            };
            // SAFETY: action has the kernel layout, lives through rt_sigaction, and
            // the permanent Rust callback/restorer obey the Linux signal ABI.
            syscall_result(unsafe {
                syscall6(
                    nr::SIGACTION,
                    [signal, &action as *const _ as usize, 0, 8, 0, 0],
                )
            })?;
        }
        Ok(())
    })();
    // SAFETY: restore the caller's mask even if creation or installation failed.
    syscall_result(unsafe {
        syscall6(
            nr::SIGPROCMASK,
            [2, &previous_mask as *const _ as usize, 0, 8, 0, 0],
        )
    })?;
    result
}

pub fn shutdown_signal() -> i32 {
    SHUTDOWN.load(Ordering::Acquire)
}

pub(crate) fn install_usr1_handler(handler: extern "C" fn(i32)) -> io::Result<()> {
    let action = SignalAction {
        handler: handler as *const () as usize,
        flags: 0x0400_0000,
        restorer: restore_signal as *const () as usize,
        mask: 0,
    };
    // SAFETY: this permanent callback and restorer use the kernel signal ABI.
    syscall_result(unsafe {
        syscall6(
            nr::SIGACTION,
            [10, &action as *const _ as usize, 0, 8, 0, 0],
        )
    })
    .map(|_| ())
}

/// Poll without a signal race, including shutdown delivered to another thread.
/// SIGINT/SIGTERM are unblocked during the wait; the caller's mask is restored.
/// Shutdown returns Interrupted; at most 127 caller descriptors are supported.
/// Before install_signals, no wake fd is added or signal handler installed.
pub fn poll_interruptible(fds: &mut [linux::PollFd], timeout_ms: i32) -> io::Result<usize> {
    if fds.len() >= MAX_POLL_FDS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "interruptible poll supports at most 127 descriptors",
        ));
    }
    let mut pollfds = [linux::PollFd {
        fd: -1,
        events: 0,
        revents: 0,
    }; MAX_POLL_FDS];
    pollfds[..fds.len()].copy_from_slice(fds);
    let wake_fd = SHUTDOWN_WAKE_RAW.load(Ordering::Acquire);
    let nr_fds = fds.len() + usize::from(wake_fd >= 0);
    if wake_fd >= 0 {
        pollfds[fds.len()] = linux::PollFd {
            fd: wake_fd,
            events: linux::POLLIN,
            revents: 0,
        };
    }
    let mut previous_mask = 0u64;
    // SAFETY: rt_sigprocmask reads/writes live kernel-sized u64 signal sets.
    syscall_result(unsafe {
        syscall6(
            nr::SIGPROCMASK,
            [
                0,
                &SHUTDOWN_MASK as *const _ as usize,
                &mut previous_mask as *mut _ as usize,
                8,
                0,
                0,
            ],
        )
    })?;
    // Block delivery through the check, then atomically unblock in ppoll.
    let wait_mask = previous_mask & !SHUTDOWN_MASK;
    let result = if shutdown_signal() != 0 {
        Err(io::ErrorKind::Interrupted.into())
    } else {
        let mut timeout = [
            timeout_ms as isize / 1000,
            timeout_ms as isize % 1000 * 1_000_000,
        ];
        let timeout_ptr = if timeout_ms < 0 {
            0
        } else {
            timeout.as_mut_ptr() as usize
        };
        // SAFETY: pollfds, timespec and the u64 signal mask have the Linux ABI
        // and stay live through ppoll; a null timeout requests an indefinite wait.
        syscall_result(unsafe {
            syscall6(
                nr::PPOLL,
                [
                    pollfds.as_mut_ptr() as usize,
                    nr_fds,
                    timeout_ptr,
                    &wait_mask as *const _ as usize,
                    8,
                    0,
                ],
            )
        })
    };
    // SAFETY: restore the saved thread mask on success, timeout, or interruption.
    syscall_result(unsafe {
        syscall6(
            nr::SIGPROCMASK,
            [2, &previous_mask as *const _ as usize, 0, 8, 0, 0],
        )
    })?;
    if shutdown_signal() != 0 || pollfds[fds.len()].revents != 0 {
        return Err(io::ErrorKind::Interrupted.into());
    }
    let count = result?;
    fds.copy_from_slice(&pollfds[..fds.len()]);
    Ok(count)
}

pub(crate) fn connect_unix(path: &std::path::Path) -> io::Result<std::os::unix::net::UnixStream> {
    let path = path.as_os_str().as_bytes();
    connect_unix_bytes(path, false)
}

pub(crate) fn connect_unix_abstract(name: &[u8]) -> io::Result<std::os::unix::net::UnixStream> {
    connect_unix_bytes(name, true)
}

fn connect_unix_bytes(
    path: &[u8],
    abstract_name: bool,
) -> io::Result<std::os::unix::net::UnixStream> {
    if path.is_empty() || path.len() > 107 || !abstract_name && path.contains(&0) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid Unix socket path",
        ));
    }
    let mut address = [0u8; 110];
    address[..2].copy_from_slice(&1u16.to_ne_bytes()); // AF_UNIX
    let start = 2 + usize::from(abstract_name);
    address[start..start + path.len()].copy_from_slice(path);
    // SAFETY: socket takes scalar arguments and returns one new owned descriptor.
    let fd = syscall_result(unsafe {
        socket_syscall(
            nr::SOCKET,
            [1, 1 | linux::O_NONBLOCK | linux::O_CLOEXEC, 0, 0, 0, 0],
        )
    })
    .map_err(|error| crate::log::system_error("socket", error))?;
    // SAFETY: successful socket transfers unique descriptor ownership.
    let fd = unsafe { OwnedFd::from_raw_fd(fd as i32) };
    loop {
        if shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        // SAFETY: the initialized sockaddr_un bytes live through connect, and
        // the nonblocking socket cannot miss shutdown while entering a wait.
        match syscall_result(unsafe {
            socket_syscall(
                nr::CONNECT,
                [
                    fd.as_raw_fd() as usize,
                    address.as_ptr() as usize,
                    path.len() + 3,
                    0,
                    0,
                    0,
                ],
            )
        }) {
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                // A full Unix backlog has no client readiness fd until connected.
                match poll_interruptible(&mut [], 10) {
                    Err(error)
                        if error.kind() == io::ErrorKind::Interrupted && shutdown_signal() == 0 =>
                    {
                        continue;
                    }
                    result => {
                        result?;
                    }
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted && shutdown_signal() == 0 => {
                continue;
            }
            Err(error) => return Err(error),
        }
    }
    let stream = std::os::unix::net::UnixStream::from(fd);
    stream.set_nonblocking(false)?;
    Ok(stream)
}

pub fn set_realtime() -> io::Result<()> {
    let priority = 49i32;
    // SAFETY: pid 0 selects the calling thread; sched_param is one live i32.
    syscall_result(unsafe {
        syscall6(
            nr::SCHED_SETSCHEDULER,
            [0, 1, &priority as *const _ as usize, 0, 0, 0],
        )
    })
    .map(|_| ())
}

pub fn lock_memory() -> io::Result<()> {
    // SAFETY: mlockall(MCL_CURRENT | MCL_FUTURE) takes only scalar flags.
    syscall_result(unsafe { syscall6(nr::MLOCKALL, [3, 0, 0, 0, 0, 0]) }).map(|_| ())
}

pub fn set_keyd_group() -> io::Result<()> {
    let Some(id) = nss::keyd_group_id()? else {
        let stderr = io::stderr();
        FdIo::new(stderr.as_fd())?.write_all(
            b"WARNING: failed to set effective group to \"keyd-rust\" (make sure the group exists)\n",
        )?;
        return Ok(());
    };
    // SAFETY: gid_t is a scalar u32 on the gated Linux platforms.
    syscall_result(unsafe { syscall6(nr::SETGID, [id as usize, 0, 0, 0, 0, 0]) }).map(|_| ())
}

/// One kernel sleep, without EINTR retry, with Linux's 32-bit useconds_t cast.
pub fn sleep_us_once(micros: u64) -> io::Result<()> {
    let micros = u64::from(micros as u32);
    let request = [
        (micros / 1_000_000) as isize,
        ((micros % 1_000_000) * 1000) as isize,
    ];
    // SAFETY: the kernel timespec consists of two live aligned native words.
    syscall_result(unsafe { syscall6(nr::NANOSLEEP, [request.as_ptr() as usize, 0, 0, 0, 0, 0]) })
        .map(|_| ())
}

unsafe fn fork_raw() -> isize {
    #[cfg(any(target_arch = "x86", target_arch = "x86_64"))]
    let arguments = [0; 6];
    #[cfg(target_arch = "aarch64")]
    let arguments = [17, 0, 0, 0, 0, 0];
    let mut previous_mask = 0u64;
    // SAFETY: valid kernel masks block delivery across fork and child publication.
    let blocked = unsafe {
        syscall6(
            nr::SIGPROCMASK,
            [
                0,
                &SHUTDOWN_MASK as *const _ as usize,
                &mut previous_mask as *mut _ as usize,
                8,
                0,
                0,
            ],
        )
    };
    if blocked < 0 {
        return blocked;
    }
    // SAFETY: fork/clone(SIGCHLD, NULL) copies the process. In the child, callers
    // use only raw async-signal-safe syscalls, never allocation or Rust drops.
    let result = unsafe { syscall6(nr::FORK, arguments) };
    if result == 0 {
        // Child shutdown is private; its inherited eventfd still refers to parent.
        SHUTDOWN_WAKE_RAW.store(-1, Ordering::Release);
    }
    // SAFETY: restore the saved valid thread mask after fork success or failure.
    unsafe {
        syscall6(
            nr::SIGPROCMASK,
            [2, &previous_mask as *const _ as usize, 0, 8, 0, 0],
        );
    }
    result
}

unsafe fn child_exit(status: usize) -> ! {
    // SAFETY: raw _exit, not libc exit; this fork child has only one thread.
    unsafe {
        syscall6(nr::EXIT, [status, 0, 0, 0, 0, 0]);
    }
    loop {
        core::hint::spin_loop();
    }
}

unsafe fn child_error(message: &'static [u8]) -> ! {
    // SAFETY: stderr is a scalar descriptor and message is permanently live.
    unsafe {
        syscall6(
            nr::WRITE,
            [2, message.as_ptr() as usize, message.len(), 0, 0, 0],
        );
        child_exit(255)
    }
}

/// Called by the application mapper before starting any threads or holding locks.
pub(crate) fn daemonize(log: &std::path::Path) -> io::Result<()> {
    let file = std::fs::File::create(log)?;
    // Keep the owner outside stdio, even when the mapper starts with closed fds.
    let log = file.as_fd().try_clone_to_owned()?;
    drop(file);
    for target in [1, 2] {
        // SAFETY: log stays owned; dup3 replaces only the mapper's stdout/stderr.
        syscall_result(unsafe {
            syscall6(nr::DUP3, [log.as_raw_fd() as usize, target, 0, 0, 0, 0])
        })?;
    }
    // SAFETY: this caller is single-threaded, with no library lock held. Parents
    // use raw exit, and the final child restores its signal setup before returning.
    let child = unsafe { fork_raw() };
    syscall_result(child)?;
    if child != 0 {
        unsafe { child_exit(0) }
    }
    let child = unsafe { fork_raw() };
    if child < 0 {
        unsafe { child_error(b"keyd-rust-application-mapper: fork failed\n") }
    }
    if child != 0 {
        unsafe { child_exit(0) }
    }
    install_signals()
}

unsafe fn command_child(
    argv: *const *const std::ffi::c_char,
    envp: *const *const std::ffi::c_char,
) -> ! {
    // SAFETY: all strings and pointer arrays were prepared before the first fork
    // and remain alive in the child copy until execve or raw exit.
    unsafe {
        let child = fork_raw();
        if child < 0 {
            child_error(b"keyd-rust: fork failed\n");
        }
        if child != 0 {
            child_exit(0);
        }

        let fd = syscall6(
            nr::OPENAT,
            [
                (-100isize) as usize,
                c"/dev/null".as_ptr() as usize,
                linux::O_RDWR | linux::O_CLOEXEC,
                0,
                0,
                0,
            ],
        );
        if fd < 0 {
            child_error(b"keyd-rust: open /dev/null failed\n");
        }
        for target in 0..3 {
            if fd as usize == target {
                continue;
            }
            if syscall6(nr::DUP3, [fd as usize, target, 0, 0, 0, 0]) < 0 {
                child_error(b"keyd-rust: dup failed\n");
            }
        }
        if fd > 2 {
            syscall6(nr::CLOSE, [fd as usize, 0, 0, 0, 0, 0]);
        }
        // If open returned a standard descriptor, clear its close-on-exec flag
        // by duplicating one of the other /dev/null descriptors back over it.
        if fd <= 2 {
            let other = if fd == 0 { 1 } else { 0 };
            if syscall6(nr::DUP3, [other, fd as usize, 0, 0, 0, 0]) < 0 {
                child_error(b"keyd-rust: dup failed\n");
            }
        }
        syscall6(
            nr::EXECVE,
            [
                c"/bin/sh".as_ptr() as usize,
                argv as usize,
                envp as usize,
                0,
                0,
                0,
            ],
        );
        child_error(b"keyd-rust: exec /bin/sh failed\n")
    }
}

/// Run an authorized config command via /bin/sh -c. The detached shell's exit
/// status is intentionally not collected, as in the source double-fork path.
pub fn execute_command(command: impl AsRef<OsStr>) -> io::Result<()> {
    let command = CString::new(command.as_ref().as_encoded_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "command contains NUL"))?;
    let environment: Vec<CString> = std::env::vars_os()
        .map(|(name, value)| {
            let mut entry = name.as_bytes().to_vec();
            entry.push(b'=');
            entry.extend_from_slice(value.as_bytes());
            CString::new(entry).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "environment contains NUL")
            })
        })
        .collect::<io::Result<_>>()?;
    let argv = [
        c"/bin/sh".as_ptr(),
        c"-c".as_ptr(),
        command.as_ptr(),
        std::ptr::null(),
    ];
    let mut envp: Vec<_> = environment.iter().map(|entry| entry.as_ptr()).collect();
    envp.push(std::ptr::null());
    // SAFETY: all child data has been allocated; command_child never returns,
    // unwinds, or calls non-async-signal-safe library code in either child.
    let child = unsafe { fork_raw() };
    if child == 0 {
        unsafe {
            command_child(argv.as_ptr(), envp.as_ptr());
        }
    }
    if child < 0 {
        // SAFETY: wait(NULL) selects any child with null status/rusage pointers.
        // C calls it once and ignores its result after a failed first fork.
        unsafe {
            syscall6(nr::WAIT4, [usize::MAX, 0, 0, 0, 0, 0]);
        }
        return Ok(());
    }
    let child = syscall_result(child)?;
    let mut status = 0i32;
    loop {
        // SAFETY: wait only for our intermediate child; status is writable and
        // live. Retry EINTR here only to reap it, never to resume a macro delay.
        match syscall_result(unsafe {
            syscall6(
                nr::WAIT4,
                [child, &mut status as *mut _ as usize, 0, 0, 0, 0],
            )
        }) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
            Ok(_) => break,
        }
    }
    if status != 0 {
        return Err(io::Error::other("command launcher failed"));
    }
    Ok(())
}
