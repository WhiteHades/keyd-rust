// Rust port of keyd's monitor.c. See LICENSE.

use crate::device::DeviceEvent;
use crate::evloop::{DeviceSlot, Event, EventKind, EventLoop};
use crate::{keys, linux, log, runtime};
use std::ffi::OsString;
use std::io::{self, Write};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::ffi::OsStrExt;

#[cfg(target_arch = "x86_64")]
const IOCTL: usize = 16;
#[cfg(target_arch = "aarch64")]
const IOCTL: usize = 29;
#[cfg(target_arch = "x86")]
const IOCTL: usize = 54;
#[cfg(target_arch = "x86_64")]
const FCNTL: usize = 72;
#[cfg(target_arch = "aarch64")]
const FCNTL: usize = 25;
#[cfg(target_arch = "x86")]
const FCNTL: usize = 55;
#[cfg(target_arch = "x86_64")]
const FSTAT: usize = 5;
#[cfg(target_arch = "aarch64")]
const FSTAT: usize = 80;
#[cfg(target_arch = "x86")]
const FSTAT: usize = 197; // fstat64

const ICANON: u32 = 2;
const ECHO: u32 = 8;

// TCGETS/TCSETS use the 36-byte kernel termios, not libc's larger structure.
fn terminal_flags(fd: BorrowedFd<'_>, flags: u32, enabled: bool) -> io::Result<()> {
    let mut termios = get_termios(fd)?;
    if enabled {
        termios[3] |= flags;
    } else {
        termios[3] &= !flags;
    }
    // SAFETY: TCSETS reads exactly the live nine-u32 kernel termios buffer.
    linux::syscall_result(unsafe {
        linux::syscall6(
            IOCTL,
            [
                fd.as_raw_fd() as usize,
                0x5402,
                termios.as_ptr() as usize,
                0,
                0,
                0,
            ],
        )
    })
    .map(|_| ())
}

fn get_termios(fd: BorrowedFd<'_>) -> io::Result<[u32; 9]> {
    let mut termios = [0u32; 9];
    // SAFETY: TCGETS writes exactly 36 bytes into this aligned live buffer.
    linux::syscall_result(unsafe {
        linux::syscall6(
            IOCTL,
            [
                fd.as_raw_fd() as usize,
                0x5401,
                termios.as_mut_ptr() as usize,
                0,
                0,
                0,
            ],
        )
    })?;
    Ok(termios)
}

fn stdout_is_fifo(fd: BorrowedFd<'_>) -> io::Result<bool> {
    // The kernel writes 144 bytes on x86_64, 128 on aarch64, 96 on x86.
    // The common buffer fits each ABI, with mode at 24 on x86_64 or 16 otherwise.
    let mut stat = [0u64; 18];
    // SAFETY: fstat receives a live aligned buffer at least as large as its ABI.
    linux::syscall_result(unsafe {
        linux::syscall6(
            FSTAT,
            [
                fd.as_raw_fd() as usize,
                stat.as_mut_ptr() as usize,
                0,
                0,
                0,
                0,
            ],
        )
    })?;
    #[cfg(target_arch = "x86_64")]
    let mode = stat[3] as u32;
    #[cfg(any(target_arch = "aarch64", target_arch = "x86"))]
    let mode = stat[2] as u32;
    Ok(mode & 0xf000 == 0x1000)
}

struct StdinCleanup<'fd>(BorrowedFd<'fd>);

impl Drop for StdinCleanup<'_> {
    fn drop(&mut self) {
        let _ = terminal_flags(self.0, ICANON, false);
        // SAFETY: F_SETFL takes scalar flags; the borrowed input stays open.
        let result = unsafe {
            linux::syscall6(
                FCNTL,
                [self.0.as_raw_fd() as usize, 4, linux::O_NONBLOCK, 0, 0, 0],
            )
        };
        // Do not risk a blocking drain if F_SETFL failed.
        if result >= 0 {
            let mut buffer = [0; 4096];
            while matches!(linux::read(self.0, &mut buffer), Ok(n) if n > 0) {}
        }
        // Source cleanup enables these flags, rather than restoring old flags,
        // and deliberately leaves stdin O_NONBLOCK, including non-TTY stdin.
        let _ = terminal_flags(self.0, ICANON | ECHO, true);
    }
}

struct Monitor {
    print_time: bool,
    last_time: i64,
    pipe_closed: bool,
}

impl Monitor {
    fn handle(
        &mut self,
        event: Event,
        devices: &mut [DeviceSlot],
        output: &mut impl Write,
    ) -> io::Result<i32> {
        if runtime::shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        match event.kind {
            EventKind::DeviceAdd(index) | EventKind::DeviceRemove(index) => {
                let device = &devices[index].device.metadata;
                let verb = if matches!(event.kind, EventKind::DeviceAdd(_)) {
                    b"added".as_slice()
                } else {
                    b"removed".as_slice()
                };
                log::write(
                    output,
                    0,
                    "device %s: %s %s (%s)\n",
                    &[
                        verb,
                        device.id.as_bytes(),
                        device.name.as_bytes(),
                        device.path.as_os_str().as_bytes(),
                    ],
                )?;
            }
            EventKind::DeviceInput(index, DeviceEvent::Key { code, pressed }) => {
                let device = &devices[index].device.metadata;
                if self.print_time && self.last_time != 0 {
                    let elapsed = (i64::from(event.timestamp) - self.last_time).to_string();
                    log::write(output, 0, "r{+%s} ms\t", &[elapsed.as_bytes()])?;
                }
                let name = keys::KEYCODE_TABLE[code as usize].name.unwrap_or("(null)");
                log::write(
                    output,
                    0,
                    "%s\t%s\t%s %s\n",
                    &[
                        device.name.as_bytes(),
                        device.id.as_bytes(),
                        name.as_bytes(),
                        if pressed != 0 { b"down" } else { b"up" },
                    ],
                )?;
            }
            EventKind::FdError(_) => self.pipe_closed = true,
            _ => {}
        }
        output.flush()?;
        self.last_time = i64::from(event.timestamp);
        Ok(0)
    }
}

/// args excludes the command name, as for the other Rust CLI entry points.
pub fn run(args: &[OsString]) -> io::Result<i32> {
    // SAFETY: stdout stays open for the process's monitor invocation.
    let output = unsafe { BorrowedFd::borrow_raw(1) };
    if args.len() == 1 && args[0] == "-h" {
        runtime::FdIo::new(output)?.write_all(
            b"Usage: keyd-rust monitor [-t]\n\n\t-t: Print the time in milliseconds between events.\n",
        )?;
        return Ok(0);
    }
    let mut monitor = Monitor {
        print_time: args.len() == 1 && args[0] == "-t",
        last_time: 0,
        pipe_closed: false,
    };
    // SAFETY: stdin stays open for the process's monitor invocation.
    let input = unsafe { BorrowedFd::borrow_raw(0) };
    let _cleanup = StdinCleanup(input);
    if get_termios(output).is_ok() {
        let _ = terminal_flags(input, ECHO, false);
    }
    let fifo = stdout_is_fifo(output)?;
    let mut event_loop = EventLoop::new()?;
    if fifo {
        event_loop.add_fd(output)?;
    }
    let mut stdout = runtime::FdIo::new(output)?;
    if let Err(error) =
        event_loop.announce_devices(|event, devices| monitor.handle(event, devices, &mut stdout))
    {
        if runtime::shutdown_signal() != 0 {
            return Ok(runtime::shutdown_signal());
        }
        if error.kind() == io::ErrorKind::BrokenPipe {
            return Ok(0);
        }
        return Err(error);
    }
    while runtime::shutdown_signal() == 0 && !monitor.pipe_closed {
        match event_loop.step(|event, devices| monitor.handle(event, devices, &mut stdout)) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => return Ok(0),
            Err(error) => return Err(error),
            Ok(()) => {}
        }
    }
    Ok(runtime::shutdown_signal())
}
