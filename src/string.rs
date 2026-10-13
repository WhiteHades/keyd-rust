// keyd - A key remapping daemon.
// © 2019 Raheman Vaiya (see also: LICENSE). Ported from frozen f564288.

/// Decode the C byte algorithm, including its permissive handling of malformed
/// encodings. Missing or NUL continuation bytes return an error instead of the
/// C assertion failure or an out-of-bounds read.
pub fn utf8_read_char(s: &[u8]) -> Result<Option<(u32, usize)>, String> {
    let Some(&first) = s.first() else {
        return Ok(None);
    };
    if first == 0 {
        return Ok(None);
    }
    let len = match first {
        0xf0..=0xff => 4,
        0xe0..=0xef => 3,
        0xc0..=0xdf => 2,
        _ => 1,
    };
    if s.len() < len || s[1..len].contains(&0) {
        return Err("truncated UTF-8 character".into());
    }
    let code = match len {
        4 => {
            ((first as u32 & 7) << 18)
                | ((s[1] as u32 & 63) << 12)
                | ((s[2] as u32 & 63) << 6)
                | (s[3] as u32 & 63)
        }
        3 => ((first as u32 & 15) << 12) | ((s[1] as u32 & 63) << 6) | (s[2] as u32 & 63),
        2 => ((first as u32 & 31) << 6) | (s[1] as u32 & 63),
        _ => first as u32 & 127,
    };
    Ok(Some((code, len)))
}

pub fn utf8_strlen(s: &str) -> usize {
    s.split('\0').next().unwrap().chars().count()
}

pub fn utf8_strlen_bytes(mut s: &[u8]) -> Result<usize, String> {
    let mut count = 0;
    while let Some((_, len)) = utf8_read_char(s)? {
        count += 1;
        s = &s[len..];
    }
    Ok(count)
}

pub fn is_timeval(s: &str) -> bool {
    let bytes = s.as_bytes();
    let n = bytes.iter().take_while(|b| b.is_ascii_digit()).count();
    n > 0
        && bytes.get(n..n + 2) == Some(b"ms")
        && matches!(bytes.get(n + 2), None | Some(0 | b'\n'))
}

pub fn str_escape(s: &str) -> String {
    // Removing ASCII escapes preserves the caller's valid UTF-8.
    String::from_utf8(str_escape_bytes(s.as_bytes())).unwrap()
}

pub fn str_escape_bytes(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut bytes = s.split(|&byte| byte == 0).next().unwrap().iter().copied();
    while let Some(c) = bytes.next() {
        if c != b'\\' {
            out.push(c);
            continue;
        }
        match bytes.next() {
            Some(b'n') => out.push(b'\n'),
            Some(b't') => out.push(b'\t'),
            Some(c @ (b'\\' | b')' | b'(')) => out.push(c),
            Some(c) => {
                out.push(b'\\');
                out.push(c);
            }
            None => break,
        }
    }
    out
}
