// Native replacement for scripts/keyd-application-mapper. See LICENSE.
pub mod config;
mod gnome;
mod kde;
pub mod wayland;
pub mod x11;

use crate::{linux, runtime};
use std::fs::{self, File};
use std::io::{self, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, Ordering};
use std::time::SystemTime;

static REFRESH: OnceLock<OwnedFd> = OnceLock::new();
static REFRESH_RAW: AtomicI32 = AtomicI32::new(-1);

#[cfg(target_arch = "x86_64")]
const EVENTFD2: usize = 290;
#[cfg(target_arch = "aarch64")]
const EVENTFD2: usize = 19;
#[cfg(target_arch = "x86")]
const EVENTFD2: usize = 328;
#[cfg(target_arch = "x86_64")]
const WRITE: usize = 1;
#[cfg(target_arch = "aarch64")]
const WRITE: usize = 64;
#[cfg(target_arch = "x86")]
const WRITE: usize = 4;

extern "C" fn refresh(_: i32) {
    let fd = REFRESH_RAW.load(Ordering::Acquire);
    if fd >= 0 {
        let count = 1u64;
        // SAFETY: the process-lifetime eventfd is nonblocking, and the signal
        // handler only writes a live fixed-size counter through a raw syscall.
        while unsafe {
            linux::syscall6(
                WRITE,
                [fd as usize, &count as *const _ as usize, 8, 0, 0, 0],
            )
        } == -4
        {}
    }
}

pub struct Refresh {
    pending: u64,
}

impl Refresh {
    fn new() -> io::Result<Self> {
        if REFRESH.get().is_none() {
            // SAFETY: scalar eventfd2 arguments return one uniquely owned descriptor.
            let fd = linux::syscall_result(unsafe {
                linux::syscall6(
                    EVENTFD2,
                    [0, linux::O_NONBLOCK | linux::O_CLOEXEC, 0, 0, 0, 0],
                )
            })?;
            let _ = REFRESH.set(unsafe { OwnedFd::from_raw_fd(fd as i32) });
        }
        REFRESH_RAW.store(REFRESH.get().unwrap().as_raw_fd(), Ordering::Release);
        runtime::install_usr1_handler(refresh)?;
        Ok(Self { pending: 0 })
    }

    pub fn wait(&mut self, fd: BorrowedFd<'_>) -> io::Result<bool> {
        if runtime::shutdown_signal() != 0 {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.pending != 0 {
            self.pending -= 1;
            return Ok(true);
        }
        let wake = REFRESH.get().unwrap();
        let mut fds = [
            linux::PollFd {
                fd: fd.as_raw_fd(),
                events: linux::POLLIN,
                revents: 0,
            },
            linux::PollFd {
                fd: wake.as_raw_fd(),
                events: linux::POLLIN,
                revents: 0,
            },
        ];
        loop {
            match runtime::poll_interruptible(&mut fds, -1) {
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted
                        && runtime::shutdown_signal() == 0 =>
                {
                    continue;
                }
                result => {
                    result?;
                    break;
                }
            }
        }
        if fds[1].revents != 0 {
            let mut counter = [0; 8];
            linux::read(wake.as_fd(), &mut counter)?;
            self.pending = u64::from_ne_bytes(counter) - 1;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

struct Mapper {
    config: config::Config,
    path: PathBuf,
    modified: SystemTime,
    verbose: bool,
    debug: bool,
    keyd: std::ffi::OsString,
}

fn output(arguments: std::fmt::Arguments<'_>) -> io::Result<()> {
    let stdout = io::stdout();
    let _lock = stdout.lock();
    writeln!(runtime::FdIo::new(stdout.as_fd())?, "{arguments}")
}

impl Mapper {
    fn change(&mut self, class: &str, title: &str) -> io::Result<()> {
        let class = config::normalize_class(class);
        let title = config::normalize_title(title);
        let modified = fs::metadata(&self.path)?.modified()?;
        if modified != self.modified {
            output(format_args!(
                "{}: Updated, reloading config...",
                self.path.display()
            ))?;
            self.config = config::Config::load(&self.path)?;
            self.modified = modified;
        }
        if self.verbose {
            output(format_args!("Active window: {class}|{title}"))?;
        }
        let bindings = self.config.bindings(&class, &title);
        if self.debug {
            for section in &self.config.sections {
                if config::matches(&class, &section.class)
                    && config::matches(&title, &section.title)
                {
                    output(format_args!(
                        "\tMatched {}|{}",
                        section.class, section.title
                    ))?;
                }
            }
        }
        let mut child = Command::new(&self.keyd)
            .arg("bind")
            .arg("reset")
            .args(bindings)
            .stdout(Stdio::null())
            .spawn()?;
        // shortcut: check only while a bind child runs, use pidfd when older Linux support is dropped.
        while child.try_wait()?.is_none() {
            match runtime::poll_interruptible(&mut [], 10) {
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted
                        && runtime::shutdown_signal() == 0 =>
                {
                    continue;
                }
                result => {
                    result?;
                }
            }
        }
        Ok(())
    }
}

fn lock(path: &Path) -> io::Result<File> {
    let file = File::create(path)?;
    #[cfg(target_arch = "x86_64")]
    const FLOCK: usize = 73;
    #[cfg(target_arch = "aarch64")]
    const FLOCK: usize = 32;
    #[cfg(target_arch = "x86")]
    const FLOCK: usize = 143;
    // SAFETY: flock takes a live descriptor and LOCK_EX | LOCK_NB scalar flags.
    linux::syscall_result(unsafe {
        linux::syscall6(FLOCK, [file.as_raw_fd() as usize, 6, 0, 0, 0, 0])
    })?;
    Ok(file)
}

pub fn run() -> io::Result<i32> {
    let mut verbose = false;
    let mut daemonize = false;
    let mut unknown = Vec::new();
    let mut after_separator = false;
    for arg in std::env::args_os().skip(1) {
        let text = arg.to_string_lossy();
        if after_separator || text == "--" {
            after_separator = true;
            unknown.push(text.into_owned());
            continue;
        }
        let (name, value) = text
            .split_once('=')
            .map_or((text.as_ref(), None), |(name, value)| (name, Some(value)));
        if name.starts_with("--") && name.len() > 2 {
            if let Some(option) = ["--help", "--verbose", "--daemonize"]
                .into_iter()
                .find(|option| option.starts_with(name))
            {
                let flag = option.as_bytes()[2] as char;
                if let Some(value) = value {
                    return argument_error(&format!(
                        "argument -{flag}/{}: ignored explicit argument {}",
                        option,
                        quote_argument(value)
                    ));
                }
                match option {
                    "--help" => return help(),
                    "--verbose" => verbose = true,
                    _ => daemonize = true,
                }
                continue;
            }
        } else if let Some(short) = text.strip_prefix('-')
            && matches!(short.as_bytes().first(), Some(b'h' | b'v' | b'd'))
        {
            let mut flags = Vec::new();
            let mut rest = short;
            while let Some(option @ ('h' | 'v' | 'd')) = rest.chars().next() {
                flags.push(option);
                rest = &rest[1..];
                if rest.starts_with('=') || rest.starts_with('-') {
                    let name = match option {
                        'h' => "-h/--help",
                        'v' => "-v/--verbose",
                        _ => "-d/--daemonize",
                    };
                    let value = rest.strip_prefix('=').unwrap_or(rest);
                    return argument_error(&format!(
                        "argument {name}: ignored explicit argument {}",
                        quote_argument(value)
                    ));
                }
            }
            if !rest.is_empty() {
                unknown.push(format!("-{rest}"));
            }
            for option in flags {
                match option {
                    'h' => return help(),
                    'v' => verbose = true,
                    _ => daemonize = true,
                }
            }
            continue;
        }
        unknown.push(text.into_owned());
    }
    if !unknown.is_empty() {
        return argument_error(&format!("unrecognized arguments: {}", unknown.join(" ")));
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "HOME not set"))?;
    let directory = PathBuf::from(home).join(".config/keyd");
    let path = directory.join("app.conf");
    if !path.exists() {
        return die("could not find app.conf, make sure it is in ~/.config/keyd/app.conf");
    }
    let config = config::Config::load(&path)?;
    let _lock = match lock(&directory.join("app.lock")) {
        Ok(lock) => lock,
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
            return die("only one instance may run at a time");
        }
        Err(error) => return Err(error),
    };
    let mut mapper = Mapper {
        config,
        modified: fs::metadata(&path)?.modified()?,
        path,
        verbose,
        debug: std::env::var_os("KEYD_DEBUG").is_some_and(|value| !value.is_empty()),
        keyd: std::env::var_os("KEYD_BIN").unwrap_or_else(|| "keyd".into()),
    };
    let mut monitor = if let Ok(mut monitor) = kde::Monitor::connect() {
        output(format_args!("kde detected"))?;
        monitor.init()?;
        Monitor::Kde(monitor)
    } else if let Ok(monitor) = wayland::Monitor::connect(wayland::Kind::Wlroots) {
        output(format_args!("wlroots detected"))?;
        Monitor::Wayland(wayland::Kind::Wlroots, monitor)
    } else if let Ok(monitor) = wayland::Monitor::connect(wayland::Kind::Cosmic) {
        output(format_args!("cosmic detected"))?;
        Monitor::Wayland(wayland::Kind::Cosmic, monitor)
    } else if let Ok(monitor) = gnome::Monitor::connect() {
        output(format_args!("Gnome detected"))?;
        if !monitor.init()? {
            return Ok(0);
        }
        Monitor::Gnome(monitor)
    } else if let Ok(mut monitor) = x11::Monitor::connect() {
        output(format_args!("X detected"))?;
        monitor.init()?;
        Monitor::X(monitor)
    } else {
        output(format_args!("Could not detect app environment :(."))?;
        return Ok(255);
    };
    if daemonize {
        let log = directory.join("app.log");
        output(format_args!(
            "Daemonizing, log output will be stored in {}...",
            log.display()
        ))?;
        runtime::daemonize(&log)?;
    }
    let mut refresh = if matches!(
        &monitor,
        Monitor::Kde(_) | Monitor::Wayland(wayland::Kind::Cosmic, _)
    ) {
        None
    } else {
        Some(Refresh::new()?)
    };
    let mut wait = |fd: BorrowedFd<'_>| {
        if let Some(refresh) = &mut refresh {
            return refresh.wait(fd);
        }
        let mut fds = [linux::PollFd {
            fd: fd.as_raw_fd(),
            events: linux::POLLIN,
            revents: 0,
        }];
        loop {
            match runtime::poll_interruptible(&mut fds, -1) {
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted
                        && runtime::shutdown_signal() == 0 =>
                {
                    continue;
                }
                result => {
                    result?;
                    return Ok(false);
                }
            }
        }
    };
    match &mut monitor {
        Monitor::Kde(monitor) => monitor.run(|class, title| mapper.change(class, title))?,
        Monitor::Wayland(_, monitor) => {
            monitor.run(|class, title| mapper.change(class, title), &mut wait)?
        }
        Monitor::Gnome(monitor) => {
            monitor.run(|class, title| mapper.change(class, title), &mut wait)?
        }
        Monitor::X(monitor) => {
            monitor.run(|class, title| mapper.change(class, title), &mut wait)?
        }
    }
    Ok(0)
}

fn help() -> io::Result<i32> {
    output(format_args!(
        "usage: keyd-application-mapper [-h] [-v] [-d]\n\noptions:\n  -h, --help       show this help message and exit\n  -v, --verbose    Log the active window (useful for discovering window and\n                   class names)\n  -d, --daemonize  fork and run in the background"
    ))?;
    Ok(0)
}

fn argument_error(message: &str) -> io::Result<i32> {
    let stderr = io::stderr();
    writeln!(
        runtime::FdIo::new(stderr.as_fd())?,
        "usage: keyd-application-mapper [-h] [-v] [-d]\nkeyd-application-mapper: error: {message}"
    )?;
    Ok(2)
}

fn quote_argument(value: &str) -> String {
    let quote = if value.contains('\'') && !value.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut text = String::from(quote);
    for ch in value.chars() {
        match ch {
            '\\' => text.push_str("\\\\"),
            '\n' => text.push_str("\\n"),
            '\r' => text.push_str("\\r"),
            '\t' => text.push_str("\\t"),
            ch if ch == quote => {
                text.push('\\');
                text.push(ch);
            }
            _ => text.push(ch),
        }
    }
    text.push(quote);
    text
}

enum Monitor {
    Kde(kde::Monitor),
    Wayland(wayland::Kind, wayland::Monitor),
    Gnome(gnome::Monitor),
    X(x11::Monitor),
}

fn die(message: &str) -> io::Result<i32> {
    let stderr = io::stderr();
    writeln!(runtime::FdIo::new(stderr.as_fd())?, "ERROR: {message}")?;
    Ok(0)
}
