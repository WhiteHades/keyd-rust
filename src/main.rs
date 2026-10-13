// Rust entry point for keyd's compatible command-line interface. See LICENSE.
use keyd::{cli, daemon, ipc, linux, log, monitor, runtime};
use std::io::{self, IsTerminal, Read, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

fn fatal_config(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<keyd::config::ParseError>())
        .is_some_and(keyd::config::ParseError::is_fatal)
}

fn check_buffer_capacity(fd: BorrowedFd<'_>) -> usize {
    // SAFETY: fd remains borrowed; ManuallyDrop prevents the metadata wrapper closing it.
    let file = std::mem::ManuallyDrop::new(unsafe { std::fs::File::from_raw_fd(fd.as_raw_fd()) });
    // shortcut: GNU stdio buffer policy, verify libc policy when enabling other targets.
    file.metadata()
        .map(|metadata| metadata.blksize())
        .ok()
        .filter(|&size| size > 0 && size < 8192)
        .map_or(8192, |size| size as usize)
}

struct CheckOutput<'a, W> {
    output: &'a mut W,
    buffer: Vec<u8>,
}

impl<W: Write> Write for CheckOutput<'_, W> {
    fn write(&mut self, mut bytes: &[u8]) -> io::Result<usize> {
        let length = bytes.len();
        while !bytes.is_empty() {
            if self.buffer.len() == self.buffer.capacity() {
                self.flush()?;
            }
            let count = bytes.len().min(self.buffer.capacity() - self.buffer.len());
            self.buffer.extend_from_slice(&bytes[..count]);
            bytes = &bytes[count..];
        }
        Ok(length)
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        // A shutdown Interrupted error must not enter Write's default retry loop.
        self.write(bytes).map(|_| ())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.output.write_all(&self.buffer)?;
        self.buffer.clear();
        self.output.flush()
    }
}

fn listen(stdout: &mut impl Write) -> io::Result<i32> {
    let stream = cli::listen_connect(Path::new(keyd::SOCKET_PATH))?;
    let mut connection = runtime::FdIo::new(stream.as_fd())?;
    let mut buffer = [0; 512];
    loop {
        let mut fds = [
            linux::PollFd {
                fd: 1,
                events: linux::POLLERR,
                revents: 0,
            },
            linux::PollFd {
                fd: stream.as_raw_fd(),
                events: linux::POLLIN,
                revents: 0,
            },
        ];
        runtime::poll_interruptible(&mut fds, -1)?;
        if fds[0].revents != 0 {
            return Ok(255);
        }
        if fds[1].revents != 0 {
            let count = connection.read(&mut buffer)?;
            if count == 0 {
                return Ok(255);
            }
            stdout.write_all(&buffer[..count])?;
        }
    }
}

fn run() -> io::Result<i32> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.is_empty() {
        return daemon::run();
    }
    let socket = Path::new(keyd::SOCKET_PATH);
    let arguments = &args[1..];
    let command = cli::command(&args[0]);
    if command == cli::Command::Monitor {
        if runtime::original_stdout_closed() && !(arguments.len() == 1 && arguments[0] == "-h") {
            return Err(log::system_error("fstat", io::Error::from_raw_os_error(9)));
        }
        return monitor::run(arguments);
    }
    let stdout = io::stdout();
    let mut stdout = runtime::FdIo::new(stdout.as_fd())?;
    let stdin = io::stdin();
    let stderr = io::stderr();
    match command {
        cli::Command::Help => {
            cli::help(&mut stdout)?;
            Ok(0)
        }
        cli::Command::Version => {
            cli::version(&mut stdout)?;
            Ok(0)
        }
        cli::Command::ListKeys => {
            cli::list_keys(&mut stdout)?;
            Ok(0)
        }
        cli::Command::Check => {
            let mut stderr = runtime::FdIo::new(stderr.as_fd())?;
            if io::stdout().is_terminal() {
                cli::check(
                    arguments,
                    Path::new(keyd::CONFIG_DIR),
                    &mut stdout,
                    &mut stderr,
                )
            } else {
                let mut output = CheckOutput {
                    output: &mut stdout,
                    buffer: Vec::with_capacity(check_buffer_capacity(io::stdout().as_fd())),
                };
                let result = cli::check(
                    arguments,
                    Path::new(keyd::CONFIG_DIR),
                    &mut output,
                    &mut stderr,
                );
                // C abort discards pending fully buffered stdout, unlike normal exit.
                if !result.as_ref().is_err_and(fatal_config) {
                    output.flush()?;
                }
                result
            }
        }
        cli::Command::Monitor => unreachable!(),
        cli::Command::Bind => cli::add_bindings(arguments, socket, &mut stdout),
        cli::Command::Input => cli::run_input(
            arguments,
            &mut runtime::FdIo::new(stdin.as_fd())?,
            socket,
            ipc::INPUT,
            &mut stdout,
        ),
        cli::Command::Do => cli::run_input(
            arguments,
            &mut runtime::FdIo::new(stdin.as_fd())?,
            socket,
            ipc::MACRO,
            &mut stdout,
        ),
        cli::Command::Reload => cli::reload(socket, &mut stdout),
        cli::Command::Listen => listen(&mut stdout),
    }
}

fn main() {
    let level = std::env::var_os("KEYD_DEBUG").map_or(0, |value| cli::atoi_timeout(&value) as i32);
    log::set_policy(
        io::stdout().is_terminal(),
        std::env::var_os("NO_COLOR").is_some(),
        level,
    );
    let result = runtime::install_signals().and_then(|()| {
        log::debug_stdout(1, "src/main.rs", line!(), "Debug mode activated", &[])?;
        run()
    });
    let status = if runtime::shutdown_signal() != 0 {
        runtime::shutdown_signal()
    } else {
        match result {
            Ok(status) => status,
            Err(error) => {
                let fatal_config = fatal_config(&error);
                let stderr = io::stderr();
                let mut stderr = match runtime::FdIo::new(stderr.as_fd()) {
                    Ok(stderr) => stderr,
                    Err(_) if fatal_config => std::process::abort(),
                    Err(_) => std::process::exit(255),
                };
                if error.kind() == io::ErrorKind::InvalidInput
                    && error.raw_os_error().is_none()
                    && !log::is_diagnostic(&error)
                {
                    let _ = log::fatal(&mut stderr, "%s", &[error.to_string().as_bytes()]);
                } else {
                    let _ = writeln!(stderr, "{error}");
                }
                // C assertions terminate the process for these defined parser limits.
                if fatal_config {
                    std::process::abort();
                }
                255
            }
        }
    };
    std::process::exit(if runtime::shutdown_signal() != 0 {
        runtime::shutdown_signal()
    } else {
        status
    });
}
