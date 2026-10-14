// Rust port of scripts/dump-xkb-config. See LICENSE.

use keyd_rust::runtime::{self, FdIo};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsFd;
use std::process::{Command, ExitCode, Stdio};

const HEADER: &str = "/usr/include/xkbcommon/xkbcommon-keysyms.h";
const CODES: [(&str, &str); 46] = [
    ("24", "q"),
    ("25", "w"),
    ("26", "e"),
    ("27", "r"),
    ("28", "t"),
    ("29", "y"),
    ("30", "u"),
    ("31", "i"),
    ("32", "o"),
    ("33", "p"),
    ("34", "["),
    ("35", "]"),
    ("38", "a"),
    ("39", "s"),
    ("40", "d"),
    ("41", "f"),
    ("42", "g"),
    ("43", "h"),
    ("44", "j"),
    ("45", "k"),
    ("46", "l"),
    ("47", ";"),
    ("48", "'"),
    ("52", "z"),
    ("53", "x"),
    ("54", "c"),
    ("55", "v"),
    ("56", "b"),
    ("57", "n"),
    ("58", "m"),
    ("59", ","),
    ("60", "."),
    ("61", "/"),
    ("10", "1"),
    ("11", "2"),
    ("12", "3"),
    ("13", "4"),
    ("14", "5"),
    ("15", "6"),
    ("16", "7"),
    ("17", "8"),
    ("18", "9"),
    ("19", "0"),
    ("51", "\\"),
    ("20", "-"),
    ("21", "="),
];

fn read_text(reader: &mut impl Read) -> io::Result<String> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
    String::from_utf8(bytes)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "input is not valid UTF-8"))
}

fn command_text(name: &str, argument: &str) -> io::Result<String> {
    let mut child = Command::new(name)
        .arg(argument)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()?;
    let pipe = child.stdout.take().unwrap();
    let result = (|| {
        let text = read_text(&mut FdIo::new(pipe.as_fd())?)?;
        // shortcut: poll child exit every 10 ms, use pidfd if this needs lower latency.
        while child.try_wait()?.is_none() {
            match runtime::poll_interruptible(&mut [], 10) {
                Err(error)
                    if error.kind() == io::ErrorKind::Interrupted
                        && runtime::shutdown_signal() == 0 => {}
                result => {
                    result?;
                }
            }
        }
        // Perl's backticks retain stdout even when the platform command fails.
        Ok(text)
    })();
    if result.is_err() {
        let _ = child.kill();
        let _ = child.wait();
    }
    result
}

fn scalar(hex: &str) -> io::Result<char> {
    let code = if hex.is_empty() {
        Some(0)
    } else {
        u32::from_str_radix(hex, 16).ok()
    };
    code.and_then(char::from_u32)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid Unicode scalar"))
}

fn symbol_map(header: &str) -> io::Result<BTreeMap<&str, char>> {
    let mut symbols = BTreeMap::new();
    for line in header.split('\n') {
        for (index, _) in line.match_indices("XKB_KEY_") {
            let tail = &line[index + "XKB_KEY_".len()..];
            // The source regex greedily chooses the final U+ comment on the line.
            let Some((before, hex)) = tail.rsplit_once("U+") else {
                continue;
            };
            let limit = tail.find([' ', '\t']).unwrap_or(tail.len());
            let Some((end, _)) = before
                .char_indices()
                .take_while(|(index, _)| *index <= limit)
                .filter(|(_, value)| value.is_whitespace())
                .last()
            else {
                continue;
            };
            let digits = hex.bytes().take_while(u8::is_ascii_hexdigit).count();
            symbols.insert(&tail[..end], scalar(&hex[..digits])?);
            break;
        }
    }
    Ok(symbols)
}

fn symbol_char(
    symbol: Option<&str>,
    symbols: &BTreeMap<&str, char>,
    errors: &mut impl Write,
) -> io::Result<Option<char>> {
    let Some(symbol) = symbol.filter(|symbol| !symbol.is_empty() && *symbol != "0") else {
        return Ok(None);
    };
    for (index, _) in symbol.match_indices('U') {
        let hex = &symbol[index + 1..];
        let digits = hex.bytes().take_while(u8::is_ascii_hexdigit).count();
        if digits != 0 {
            return scalar(&hex[..digits]).map(Some);
        }
    }
    match symbols.get(symbol).copied().filter(|&value| value != '0') {
        Some(value) => Ok(Some(value)),
        None => {
            writeln!(errors, "Could not resolve sym: {symbol}")?;
            Ok(None)
        }
    }
}

fn key_line(line: &str) -> Option<(&str, &str)> {
    for (index, _) in line.match_indices("keycode") {
        let tail = line[index + "keycode".len()..].trim_start_matches(char::is_whitespace);
        let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            continue;
        }
        let rest = tail[digits..].trim_start_matches(char::is_whitespace);
        if let Some(rest) = rest.strip_prefix('=') {
            return Some((
                &tail[..digits],
                rest.trim_start_matches(char::is_whitespace),
            ));
        }
    }
    None
}

fn write_layer(output: &mut impl Write, layer: &BTreeMap<&str, char>) -> io::Result<()> {
    for (key, &value) in layer {
        let code = u32::from(value);
        // Perl's UTF-8 output encoder escapes Unicode noncharacters.
        if (0xfdd0..=0xfdef).contains(&code) || code & 0xffff >= 0xfffe {
            writeln!(output, "{key} = \\x{{{code:04X}}}")?;
        } else {
            writeln!(output, "{key} = {value}")?;
        }
    }
    Ok(())
}

fn keymap(
    symbols: &BTreeMap<&str, char>,
    keycodes: &str,
    errors: &mut impl Write,
) -> io::Result<[BTreeMap<&'static str, char>; 4]> {
    let mut layers: [BTreeMap<&str, char>; 4] = std::array::from_fn(|_| BTreeMap::new());
    for line in keycodes.split('\n') {
        let Some((code, rest)) = key_line(line) else {
            continue;
        };
        // Perl skips X code 19 because its keyd name "0" is false.
        let Some((_, key)) = CODES
            .iter()
            .find(|(number, key)| *number == code && *key != "0")
        else {
            continue;
        };
        let columns: Vec<_> = rest.split_whitespace().collect();
        for (layer, column) in layers.iter_mut().zip([0, 1, 4, 5]) {
            if let Some(value) = symbol_char(columns.get(column).copied(), symbols, errors)?
                .filter(|&value| value != '0')
            {
                // Empty/false symbols on repeated keycodes don't erase prior assignments.
                layer.insert(*key, value);
            }
        }
    }
    Ok(layers)
}

fn dump(
    layers: &[BTreeMap<&str, char>; 4],
    query: &str,
    output: &mut impl Write,
) -> io::Result<()> {
    let layout = query.find("layout:").map_or("", |index| {
        query[index + "layout:".len()..]
            .split_whitespace()
            .next()
            .unwrap_or("")
    });
    write!(output, "[{layout}:layout]\n\n")?;
    write_layer(output, &layers[0])?;
    writeln!(output, "shift = layer({layout}_shift)")?;
    if !layers[2].is_empty() {
        writeln!(output, "altgr = layer({layout}_altgr)")?;
    }
    writeln!(output)?;
    writeln!(output, "[{layout}_shift:S]")?;
    write_layer(output, &layers[1])?;
    if !layers[2].is_empty() {
        write!(output, "\n[{layout}_altgr:G]\n")?;
        write_layer(output, &layers[2])?;
        write!(output, "\n[{layout}_altgr+{layout}_shift]\n")?;
        write_layer(output, &layers[3])?;
    }
    Ok(())
}

fn run() -> io::Result<()> {
    let header = File::open(HEADER)?;
    let header = read_text(&mut FdIo::new(header.as_fd())?)?;
    let symbols = symbol_map(&header)?;
    let keycodes = command_text("xmodmap", "-pke")?;
    let stderr = io::stderr();
    let layers = keymap(&symbols, &keycodes, &mut FdIo::new(stderr.as_fd())?)?;
    let query = command_text("setxkbmap", "-query")?;
    let stdout = io::stdout();
    dump(&layers, &query, &mut FdIo::new(stdout.as_fd())?)
}

fn main() -> ExitCode {
    let result = runtime::install_signals().and_then(|()| run());
    let signal = runtime::shutdown_signal();
    if signal != 0 {
        return ExitCode::from((128 + signal) as u8);
    }
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let stderr = io::stderr();
            if let Ok(mut stderr) = FdIo::new(stderr.as_fd()) {
                let _ = writeln!(stderr, "keyd-rust-dump-xkb-config: {error}");
            }
            ExitCode::FAILURE
        }
    }
}
