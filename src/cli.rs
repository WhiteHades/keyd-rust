// Rust port of the command frontends in keyd.c/check.c. © 2019 Raheman Vaiya.

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};

use crate::{config, ipc, keys, log};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Help,
    Version,
    Check,
    Monitor,
    Bind,
    Input,
    Do,
    Listen,
    Reload,
    ListKeys,
}

pub fn command(name: &OsStr) -> Command {
    match name.as_bytes() {
        b"help" | b"-h" | b"--help" => Command::Help,
        b"version" | b"-v" | b"--version" => Command::Version,
        b"check" | b"-c" | b"--check" => Command::Check,
        b"monitor" | b"-m" | b"--monitor" => Command::Monitor,
        b"bind" | b"-e" | b"--expression" => Command::Bind,
        b"input" | b"" => Command::Input,
        b"do" => Command::Do,
        b"listen" => Command::Listen,
        b"reload" => Command::Reload,
        b"list-keys" => Command::ListKeys,
        _ => Command::Help,
    }
}

pub fn help(output: &mut impl Write) -> io::Result<()> {
    output.write_all(concat!(
        "usage: keyd [-v] [-h] [command] [<args>]\n\n",
        "Commands:\n",
        "    monitor [-t]                   Print key events in real time.\n",
        "    check [<config file>...]       Check the supplied config files for errors. If no files are supplied, all .conf files in the config directory will be checked.\n",
        "    list-keys                      Print a list of valid key names.\n",
        "    reload                         Trigger a reload .\n",
        "    listen                         Print layer state changes of the running keyd daemon to stdout.\n",
        "    bind <binding> [<binding>...]  Add the supplied bindings to all loaded configs.\n",
        "Options:\n",
        "    -v, --version                  Print the current version and exit.\n",
        "    -h, --help                     Print help and exit.\n",
    ).as_bytes())
}

pub fn version(output: &mut impl Write) -> io::Result<()> {
    writeln!(output, "keyd {}", crate::BUILD_VERSION)
}

pub fn list_keys(output: &mut impl Write) -> io::Result<()> {
    for key in &keys::KEYCODE_TABLE {
        for name in [key.name, key.alt_name, key.shifted_name]
            .into_iter()
            .flatten()
        {
            writeln!(output, "{name}")?;
        }
    }
    Ok(())
}

/// `args` contains only the arguments after the command name.
pub fn check(
    args: &[OsString],
    config_directory: &Path,
    stdout: &mut impl Write,
    stderr: &mut impl Write,
) -> io::Result<i32> {
    let paths = if args.is_empty() {
        let entries = match std::fs::read_dir(config_directory) {
            Ok(entries) => entries,
            Err(error) => {
                // perror does not include Rust's "(os error N)" suffix.
                let message = error.to_string();
                writeln!(
                    stderr,
                    "chdir: {}",
                    message.split(" (os error ").next().unwrap_or(&message)
                )?;
                return Ok(255);
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if path.as_os_str().as_bytes().ends_with(b".conf") {
                paths.push(path);
            }
        }
        paths
    } else {
        args.iter().map(PathBuf::from).collect()
    };
    let mut status = 0;
    for path in paths {
        log::write(stdout, 0, "Parsing b{%s}\n", &[path.as_os_str().as_bytes()])?;
        match config::config_parse_with_warnings(&path, |item| warning(item, stdout)) {
            Ok(config) => {
                if config.warning_count != 0 {
                    status = 255;
                }
            }
            Err(config::LoadError::Parse(error)) if error.is_fatal() => {
                return Err(io::Error::other(error));
            }
            Err(config::LoadError::Parse(_)) => {
                log::write(stdout, 0, "\tr{FAILED} (file does not exist?)\n", &[])?;
                status = 255;
            }
            Err(config::LoadError::Output(error)) => return Err(error),
        }
    }
    if status == 0 {
        log::write(stdout, 0, "\nNo errors found.\n", &[])?;
    }
    Ok(status)
}

pub(crate) fn warning(warning: &config::ConfigWarning, stdout: &mut impl Write) -> io::Result<()> {
    let mut template = b"\ty{WARNING:} ".to_vec();
    if let (Some(path), Some(line)) = (&warning.path, warning.line) {
        template.extend_from_slice(b"b{");
        for &byte in path.as_os_str().as_bytes() {
            template.push(byte);
            // Do not copy C's format-string access through a percent in a source path.
            if byte == b'%' {
                template.push(b'%');
            }
        }
        template.extend_from_slice(format!("}}:r{{{line}}}: ").as_bytes());
    }
    template.extend_from_slice(warning.template.as_bytes());
    template.push(b'\n');
    // config_warn truncates its snprintf template before colour/argument expansion.
    template.truncate(1023);
    let arguments: Vec<_> = warning.arguments.iter().map(|arg| arg.as_bytes()).collect();
    log::write_template(stdout, &template, &arguments, log::colours_enabled())
}

pub fn add_bindings(args: &[OsString], socket: &Path, output: &mut impl Write) -> io::Result<i32> {
    let mut status = 0;
    for argument in args {
        let bytes = argument.as_bytes();
        reject_nul(bytes)?;
        if ipc::exec(socket, ipc::BIND, bytes, 0, output)? {
            status = 255;
        }
    }
    if status == 0 {
        output.write_all(b"Success\n")?;
    }
    Ok(status)
}

/// C atoi's numeric-prefix/sign behavior, followed by conversion to uint32_t.
pub fn atoi_timeout(value: &OsStr) -> u32 {
    let mut bytes = value.as_bytes();
    while bytes
        .first()
        .is_some_and(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r' | 11 | 12))
    {
        bytes = &bytes[1..];
    }
    let negative = bytes.first() == Some(&b'-');
    if negative || bytes.first() == Some(&b'+') {
        bytes = &bytes[1..];
    }
    let mut number = 0u64;
    let limit = i64::MAX as u64 + u64::from(negative);
    for byte in bytes.iter().take_while(|byte| byte.is_ascii_digit()) {
        number = number
            .saturating_mul(10)
            .saturating_add(u64::from(byte - b'0'))
            .min(limit);
    }
    if negative {
        number.wrapping_neg() as u32
    } else {
        number as u32
    }
}

fn reject_nul(bytes: &[u8]) -> io::Result<()> {
    if bytes.contains(&0) {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "command argument contains NUL",
        ))
    } else {
        Ok(())
    }
}

pub fn run_input(
    args: &[OsString],
    stdin: &mut impl Read,
    socket: &Path,
    kind: u32,
    output: &mut impl Write,
) -> io::Result<i32> {
    let (timeout, args) = if args.len() >= 2 && args[0] == "-t" {
        reject_nul(args[1].as_bytes())?;
        (atoi_timeout(&args[1]), &args[2..])
    } else {
        (0, args)
    };
    let mut input = Vec::with_capacity(ipc::MAX_MESSAGE_SIZE);
    if args.is_empty() {
        let mut buffer = [0; 512];
        while input.len() < ipc::MAX_MESSAGE_SIZE {
            if crate::runtime::shutdown_signal() != 0 {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let limit = buffer.len().min(ipc::MAX_MESSAGE_SIZE - input.len());
            match stdin.read(&mut buffer[..limit]) {
                Ok(0) => break,
                Ok(count) => input.extend_from_slice(&buffer[..count]),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => return Err(error),
            }
        }
    } else {
        for (index, argument) in args.iter().enumerate() {
            let bytes = argument.as_bytes();
            reject_nul(bytes)?;
            if index != 0 {
                input.push(b' ');
            }
            if input.len().saturating_add(bytes.len()) >= ipc::MAX_MESSAGE_SIZE {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "maximum input length exceeded",
                ));
            }
            input.extend_from_slice(bytes);
        }
    }
    if input.len() >= ipc::MAX_MESSAGE_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "maximum input length exceeded",
        ));
    }
    Ok(i32::from(ipc::exec(socket, kind, &input, timeout, output)?))
}

pub fn reload(socket: &Path, output: &mut impl Write) -> io::Result<i32> {
    ipc::exec(socket, ipc::RELOAD, b"", 0, output)?;
    Ok(0)
}

/// The parent supplies the poll loop, including stdout POLLERR handling.
pub fn listen_connect(socket: &Path) -> io::Result<UnixStream> {
    let stream = ipc::connect(socket)?;
    ipc::Message::new(ipc::LAYER_LISTEN, b"", 0)?
        .write(&mut crate::runtime::FdIo::new(stream.as_fd())?)?;
    Ok(stream)
}
