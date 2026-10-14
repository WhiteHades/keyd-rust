// Rust port of log.c/log.h. © 2019 Raheman Vaiya. See LICENSE.

use std::io::{self, Write};
use std::os::fd::AsFd;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

static COLOURS: AtomicBool = AtomicBool::new(false);
static LOG_LEVEL: AtomicI32 = AtomicI32::new(0);

thread_local! {
    // C's parser diagnostic survives successful commands and config warnings.
    static ERROR: std::cell::RefCell<[u8; 2048]> = const { std::cell::RefCell::new([0; 2048]) };
}

pub fn set_error(message: &str) {
    set_error_bytes(message.as_bytes());
}

pub fn set_error_bytes(message: &[u8]) {
    ERROR.with_borrow_mut(|buffer| {
        let size = message.len().min(buffer.len() - 1);
        buffer[..size].copy_from_slice(&message[..size]);
        buffer[size] = 0;
    });
}

pub fn error() -> Vec<u8> {
    ERROR
        .with_borrow(|buffer| buffer[..buffer.iter().position(|&byte| byte == 0).unwrap()].to_vec())
}

pub fn set_policy(stdout_is_terminal: bool, no_color_is_set: bool, level: i32) {
    COLOURS.store(stdout_is_terminal && !no_color_is_set, Ordering::Relaxed);
    LOG_LEVEL.store(level, Ordering::Relaxed);
}

pub fn colours_enabled() -> bool {
    COLOURS.load(Ordering::Relaxed)
}

pub fn level() -> i32 {
    LOG_LEVEL.load(Ordering::Relaxed)
}

#[derive(Debug)]
struct Diagnostic {
    label: Option<&'static str>,
    error: io::Error,
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, output: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = self.error.to_string();
        let suffix = self
            .error
            .raw_os_error()
            .map(|code| format!(" (os error {code})"));
        let message = suffix
            .as_ref()
            .and_then(|suffix| message.strip_suffix(suffix))
            .unwrap_or(&message);
        if let Some(label) = self.label {
            write!(output, "{label}: {message}")
        } else {
            output.write_str(message)
        }
    }
}

impl std::error::Error for Diagnostic {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

pub fn system_error(label: &'static str, error: io::Error) -> io::Error {
    io::Error::new(
        error.kind(),
        Diagnostic {
            label: Some(label),
            error,
        },
    )
}

pub fn diagnostic(error: io::Error) -> io::Error {
    io::Error::new(error.kind(), Diagnostic { label: None, error })
}

pub fn is_system_error(error: &io::Error) -> bool {
    error
        .get_ref()
        .and_then(|error| error.downcast_ref::<Diagnostic>())
        .is_some_and(|error| error.label.is_some())
}

pub fn is_diagnostic(error: &io::Error) -> bool {
    error
        .get_ref()
        .is_some_and(|error| error.is::<Diagnostic>())
}

pub fn perror(label: &'static str, error: io::Error) -> io::Result<()> {
    let _lock = io::stdout().lock();
    let stderr = io::stderr();
    writeln!(
        crate::runtime::FdIo::new(stderr.as_fd())?,
        "{}",
        system_error(label, error)
    )
}

// Format arguments bypass colour parsing; warning source prefixes retain C's markup.
pub fn colorize(template: &str, colours: bool) -> Vec<u8> {
    colorize_bytes(template.as_bytes(), colours)
}

fn colorize_bytes(bytes: &[u8], colours: bool) -> Vec<u8> {
    let mut output = Vec::with_capacity(1024);
    let mut inside_escape = false;
    let mut index = 0;
    while index < bytes.len() && bytes[index] != 0 && output.len() < 1024 {
        if bytes.get(index + 1) == Some(&b'{') {
            let code = match bytes[index] {
                b'r' => 1,
                b'g' => 2,
                b'y' => 3,
                b'b' => 4,
                b'm' => 5,
                b'c' => 6,
                b'w' => 7,
                _ => 0,
            };
            if code != 0 {
                if colours && 1024 - output.len() > 5 {
                    output.extend_from_slice(&[27, b'[', b'3', b'0' + code, b'm']);
                }
                inside_escape = true;
                index += 2;
                continue;
            }
        }
        if bytes[index] == b'}' && inside_escape {
            if colours && 1024 - output.len() > 4 {
                output.extend_from_slice(b"\x1b[0m");
            }
            inside_escape = false;
        } else {
            output.push(bytes[index]);
        }
        index += 1;
    }
    output
}

/// `%s` inserts one byte slice verbatim; `%%` inserts a percent sign.
/// Numeric arguments should be converted with their standard Display implementation.
pub fn write(
    output: &mut impl Write,
    level: i32,
    template: &'static str,
    arguments: &[&[u8]],
) -> io::Result<()> {
    if level > LOG_LEVEL.load(Ordering::Relaxed) {
        return Ok(());
    }
    write_template(output, template.as_bytes(), arguments, colours_enabled())
}

pub(crate) fn write_template(
    output: &mut impl Write,
    template: &[u8],
    arguments: &[&[u8]],
    colours: bool,
) -> io::Result<()> {
    let template = colorize_bytes(template, colours);
    write_arguments(output, &template, arguments)
}

pub(crate) fn write_arguments(
    output: &mut impl Write,
    template: &[u8],
    arguments: &[&[u8]],
) -> io::Result<()> {
    let mut rendered = Vec::with_capacity(template.len());
    #[cfg(target_env = "musl")]
    let mut boundaries = vec![0];
    let mut index = 0;
    let mut argument = 0;
    while index < template.len() {
        let specifier = if template[index..].starts_with(b"%s") {
            2
        } else if template[index..].starts_with(b"%ld") {
            3
        } else {
            0
        };
        if specifier != 0 {
            let value = arguments.get(argument).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidInput, "missing log argument")
            })?;
            #[cfg(target_env = "musl")]
            boundaries.push(rendered.len());
            rendered.extend_from_slice(value);
            #[cfg(target_env = "musl")]
            boundaries.push(rendered.len());
            argument += 1;
            index += specifier;
        } else if template[index] == b'%' && template.get(index + 1) == Some(&b'%') {
            rendered.push(b'%');
            index += 2;
        } else {
            rendered.push(template[index]);
            index += 1;
        }
    }
    #[cfg(target_env = "musl")]
    {
        // musl vfprintf writes each literal/argument span through __fwritex.
        boundaries.push(rendered.len());
        for span in boundaries.windows(2) {
            output.write_all(&rendered[span[0]..span[1]])?;
        }
        Ok(())
    }
    #[cfg(not(target_env = "musl"))]
    output.write_all(&rendered)
}

pub fn debug(
    output: &mut impl Write,
    level: i32,
    file: &'static str,
    line: u32,
    template: &'static str,
    arguments: &[&[u8]],
) -> io::Result<()> {
    write(
        output,
        level,
        "r{DEBUG:} b{%s:%s:} ",
        &[file.as_bytes(), line.to_string().as_bytes()],
    )?;
    write(output, level, template, arguments)?;
    write(output, level, "\n", &[])
}

pub fn stdout(level: i32, template: &'static str, arguments: &[&[u8]]) -> io::Result<()> {
    if level > self::level() {
        return Ok(());
    }
    let stdout = io::stdout();
    let _lock = stdout.lock();
    write(
        &mut crate::runtime::FdIo::new(stdout.as_fd())?,
        level,
        template,
        arguments,
    )
}

pub fn debug_stdout(
    level: i32,
    file: &'static str,
    line: u32,
    template: &'static str,
    arguments: &[&[u8]],
) -> io::Result<()> {
    if level > self::level() {
        return Ok(());
    }
    let stdout = io::stdout();
    let _lock = stdout.lock();
    debug(
        &mut crate::runtime::FdIo::new(stdout.as_fd())?,
        level,
        file,
        line,
        template,
        arguments,
    )
}

/// The caller supplies stderr and returns this status from its entry point.
pub fn fatal(
    stderr: &mut impl Write,
    template: &'static str,
    arguments: &[&[u8]],
) -> io::Result<i32> {
    // Fatal errors bypass the debug/log-level gate.
    stderr.write_all(&colorize("r{FATAL ERROR:} ", colours_enabled()))?;
    // Keep argument interpolation on the same safe path, even at a negative log level.
    write(stderr, i32::MIN, template, arguments)?;
    stderr.write_all(b"\n")?;
    Ok(255)
}
