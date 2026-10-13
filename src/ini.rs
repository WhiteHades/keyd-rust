// Rust port of keyd's ini.c. © 2019 Raheman Vaiya. See LICENSE.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;

pub const MAX_SECTIONS: usize = 32;
pub const MAX_SECTION_ENTRIES: usize = 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    Recoverable(Vec<u8>),
    Fatal(String),
}

impl ParseError {
    pub fn is_fatal(&self) -> bool {
        matches!(self, Self::Fatal(_))
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Self::Recoverable(message) => message,
            Self::Fatal(message) => message.as_bytes(),
        }
    }
}

impl From<String> for ParseError {
    fn from(message: String) -> Self {
        Self::Recoverable(message.into_bytes())
    }
}

impl From<&str> for ParseError {
    fn from(message: &str) -> Self {
        Self::Recoverable(message.as_bytes().to_vec())
    }
}

impl From<Vec<u8>> for ParseError {
    fn from(message: Vec<u8>) -> Self {
        Self::Recoverable(message)
    }
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&String::from_utf8_lossy(self.as_bytes()))
    }
}

impl std::error::Error for ParseError {}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IniEntry {
    pub key: OsString,
    pub value: Option<OsString>,
    pub line: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IniSection {
    pub name: OsString,
    pub entries: Vec<IniEntry>,
    pub line: usize,
}

pub fn parse_kvp(input: &str) -> (&str, Option<&str>) {
    let (key, value) = parse_kvp_bytes(input.as_bytes());
    // ASCII delimiters cannot split a character in the caller's valid UTF-8.
    (
        std::str::from_utf8(key).unwrap(),
        value.map(|s| std::str::from_utf8(s).unwrap()),
    )
}

pub fn parse_kvp_bytes(input: &[u8]) -> (&[u8], Option<&[u8]>) {
    let start = usize::from(input.starts_with(b"="));
    match input[start..].iter().position(|&byte| byte == b'=') {
        Some(offset) => {
            let offset = start + offset;
            let end = input[..offset]
                .iter()
                .rposition(|byte| !matches!(byte, b' ' | b'\t'))
                .map_or(0, |index| index + 1);
            let start = input[offset + 1..]
                .iter()
                .position(|byte| !matches!(byte, b' ' | b'\t'))
                .map_or(input.len(), |index| offset + 1 + index);
            (&input[..end], Some(&input[start..]))
        }
        None => (input, None),
    }
}

pub fn ini_parse_string(
    input: &str,
    default_section_name: Option<&str>,
) -> Result<Vec<IniSection>, ParseError> {
    ini_parse_bytes(input.as_bytes(), default_section_name.map(str::as_bytes))
}

pub fn ini_parse_bytes(
    input: &[u8],
    default_section_name: Option<&[u8]>,
) -> Result<Vec<IniSection>, ParseError> {
    let mut sections: Vec<IniSection> = Vec::new();
    for (index, line) in input.split(|&byte| byte == b'\n').enumerate() {
        let line = line.trim_ascii();
        if line.is_empty() || line.starts_with(b"#") {
            continue;
        }
        if line.starts_with(b"[") && line.ends_with(b"]") {
            if sections.len() == MAX_SECTIONS {
                return Err(ParseError::Fatal(format!(
                    "maximum sections ({MAX_SECTIONS}) exceeded"
                )));
            }
            let name = &line[1..line.len() - 1];
            if name.len() >= 256 {
                return Err("maximum INI section name length (255) exceeded".into());
            }
            sections.push(IniSection {
                name: OsString::from_vec(name.to_vec()),
                entries: Vec::new(),
                line: index + 1,
            });
            continue;
        }
        if sections.is_empty() {
            let name = default_section_name.ok_or("Invalid config file (missing [ids] section)")?;
            if name.len() >= 256 {
                return Err("maximum INI section name length (255) exceeded".into());
            }
            sections.push(IniSection {
                name: OsString::from_vec(name.to_vec()),
                entries: Vec::new(),
                line: 0,
            });
        }
        let section = sections.last_mut().unwrap();
        if section.entries.len() == MAX_SECTION_ENTRIES {
            return Err(ParseError::Fatal(format!(
                "maximum section entries ({MAX_SECTION_ENTRIES}) exceeded"
            )));
        }
        let (key, value) = parse_kvp_bytes(line);
        section.entries.push(IniEntry {
            key: OsString::from_vec(key.to_vec()),
            value: value.map(|value| OsString::from_vec(value.to_vec())),
            line: index + 1,
        });
    }
    Ok(sections)
}
