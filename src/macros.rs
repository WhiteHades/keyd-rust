// keyd - A key remapping daemon.
// © 2019 Raheman Vaiya (see also: LICENSE). Ported from frozen f564288.

use crate::keys::{KEYCODE_TABLE, MOD_SHIFT, MODIFIERS, parse_key_sequence};
use crate::runtime::{shutdown_signal, sleep_us_once};
use crate::string::utf8_read_char;
use crate::unicode::{unicode_get_sequence, unicode_lookup_index};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MacroKind {
    KeySequence,
    Hold,
    Release,
    Unicode,
    Timeout,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MacroEntry {
    pub kind: MacroKind,
    pub data: u16,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Macro {
    pub entries: Vec<MacroEntry>,
}

pub fn macro_parse(s: &str) -> Result<Macro, String> {
    parse(s.as_bytes())
        .map_err(|error| String::from_utf8_lossy(&error).into_owned())
        .inspect_err(|error| crate::log::set_error(error))
}

/// Preserve C's permissive byte decoding and stop at the first NUL.
/// Incomplete characters return safe errors; codepoints absent from the table are skipped.
pub fn macro_parse_bytes(s: &[u8]) -> Result<Macro, Vec<u8>> {
    parse(s).inspect_err(|error| crate::log::set_error_bytes(error))
}

fn parse(s: &[u8]) -> Result<Macro, Vec<u8>> {
    let mut entries = Vec::new();
    let mut add = |kind, data| -> Result<(), Vec<u8>> {
        if entries.len() == 256 {
            return Err(b"maximum macro size (256) exceeded".to_vec());
        }
        entries.push(MacroEntry { kind, data });
        Ok(())
    };
    for tok in s
        .split(|&byte| byte == 0)
        .next()
        .unwrap()
        .split(|&byte| byte == b' ')
        .filter(|s| !s.is_empty())
    {
        if let Some((code, mods)) = key_sequence(tok) {
            add(MacroKind::KeySequence, ((mods as u16) << 8) | code as u16)?;
        } else if tok.contains(&b'+') {
            for key in tok.split(|&byte| byte == b'+').filter(|s| !s.is_empty()) {
                if timeval(key) {
                    add(MacroKind::Timeout, parse_timeout(key)?)?;
                } else if let Some((code, _)) = key_sequence(key) {
                    add(MacroKind::Hold, code as u16)?;
                } else {
                    let mut error = key.to_vec();
                    error.extend_from_slice(b" is not a valid key");
                    return Err(error);
                }
            }
            add(MacroKind::Release, 0)?;
        } else if timeval(tok) {
            add(MacroKind::Timeout, parse_timeout(tok)?)?;
        } else {
            let mut text = tok;
            while let Some((codepoint, len)) = utf8_read_char(text).map_err(String::into_bytes)? {
                if len == 1 && codepoint < 128 {
                    for (i, ent) in KEYCODE_TABLE.iter().enumerate() {
                        if ent.name.is_some_and(|s| s.as_bytes() == &text[..1]) {
                            add(MacroKind::KeySequence, i as u16)?;
                            break;
                        }
                        if ent.shifted_name.is_some_and(|s| s.as_bytes() == &text[..1]) {
                            add(MacroKind::KeySequence, ((MOD_SHIFT as u16) << 8) | i as u16)?;
                            break;
                        }
                    }
                } else {
                    let idx = unicode_lookup_index(codepoint);
                    // C deliberately excludes index 0 here, though lookup itself returns it.
                    if idx > 0 {
                        add(MacroKind::Unicode, idx as u16)?;
                    }
                }
                text = &text[len..];
            }
        }
    }
    Ok(Macro { entries })
}

fn key_sequence(s: &[u8]) -> Option<(u8, u8)> {
    std::str::from_utf8(s).ok().and_then(parse_key_sequence)
}

fn timeval(s: &[u8]) -> bool {
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    digits > 0
        && s.get(digits..digits + 2) == Some(b"ms")
        && matches!(s.get(digits + 2), None | Some(b'\n'))
}

fn parse_timeout(s: &[u8]) -> Result<u16, Vec<u8>> {
    let digits = s.iter().take_while(|b| b.is_ascii_digit()).count();
    // C atoi is undefined on int overflow; reject it rather than invent a result.
    std::str::from_utf8(&s[..digits])
        .ok()
        .and_then(|digits| digits.parse::<i32>().ok())
        .map(|n| n as u16)
        .ok_or_else(|| b"macro timeout exceeds C int range".to_vec())
}

/// `timeout` is microseconds, as in C. The return value counts milliseconds,
/// excluding the extra modifier-settling sleep and preserving C's double-based
/// accounting after each entry. Timeout first narrows to native C size_t;
/// sleep arguments truncate to Linux's u32 type.
/// Shutdown stops callbacks and returns the accounting completed so far.
pub fn macro_execute(macro_: &Macro, timeout: u64, mut output: impl FnMut(u8, u8)) -> i64 {
    macro_execute_while(macro_, timeout, |code, state| {
        output(code, state);
        true
    })
}

/// A false callback result stops all further output and delays immediately,
/// returning the accounting completed so far, without requesting shutdown.
pub fn macro_execute_while(
    macro_: &Macro,
    timeout: u64,
    mut output: impl FnMut(u8, u8) -> bool,
) -> i64 {
    let timeout = timeout as usize as u64;
    assert!(
        macro_.entries.len() <= 256,
        "maximum macro size (256) exceeded"
    );
    let mut output = |code, state| shutdown_signal() == 0 && output(code, state);
    let mut hold_start = None;
    let mut time = 0i64;
    for (i, ent) in macro_.entries.iter().enumerate() {
        if shutdown_signal() != 0 {
            return time;
        }
        match ent.kind {
            MacroKind::Hold => {
                hold_start.get_or_insert(i);
                if !output(ent.data as u8, 1) {
                    return time;
                }
            }
            MacroKind::Release => {
                if let Some(start) = hold_start.take() {
                    // C releases every entry since the first hold, including timeouts.
                    for held in &macro_.entries[start..i] {
                        if !output(held.data as u8, 0) {
                            return time;
                        }
                    }
                }
            }
            MacroKind::Unicode => {
                for code in unicode_get_sequence(ent.data) {
                    if !output(code, 1) || !output(code, 0) {
                        return time;
                    }
                }
            }
            MacroKind::KeySequence => {
                let code = ent.data as u8;
                let mods = (ent.data >> 8) as u8;
                for modifier in MODIFIERS {
                    if mods & modifier.mask != 0 && !output(modifier.key, 1) {
                        return time;
                    }
                }
                if mods != 0 && timeout != 0 && !delay(timeout) {
                    return time;
                }
                if !output(code, 1) || !output(code, 0) {
                    return time;
                }
                for modifier in MODIFIERS {
                    if mods & modifier.mask != 0 && !output(modifier.key, 0) {
                        return time;
                    }
                }
            }
            MacroKind::Timeout => {
                if !delay(u64::from(ent.data) * 1000) {
                    return time;
                }
                time += ent.data as i64;
            }
        }
        if shutdown_signal() != 0 {
            return time;
        }
        if timeout != 0 {
            if !delay(timeout) {
                return time;
            }
            // 1E3 is a double in C, including its rounding for large timeouts.
            time = (time as f64 + timeout as f64 / 1E3) as i64;
        }
    }
    time
}

fn delay(micros: u64) -> bool {
    if shutdown_signal() != 0 {
        return false;
    }
    // C ignores usleep errors; nonshutdown signals still advance the macro.
    let _ = sleep_us_once(micros);
    shutdown_signal() == 0
}
