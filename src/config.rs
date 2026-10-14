// Rust port of keyd's config.c. © 2019 Raheman Vaiya. See LICENSE.

use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

pub use crate::ini::ParseError;
use crate::ini::{IniSection, ini_parse_bytes, parse_kvp_bytes};
use crate::keys::*;
use crate::macros::{Macro, macro_parse_bytes};
use crate::string::{str_escape_bytes, utf8_strlen_bytes};

pub const MAX_LAYER_NAME_LEN: usize = 64;
pub const MAX_LAYERS: usize = 32;
pub const MAX_EXP_LEN: usize = 512;
pub const MAX_DESCRIPTORS: usize = 1024;
pub const MAX_MACROS: usize = 256;
pub const MAX_COMMANDS: usize = 64;
pub const MAX_CHORDS: usize = 64;
pub const MAX_CHORD_KEYS: usize = 8;
pub const MAX_IDS: usize = 64;
pub const ID_EXCLUDED: u8 = 1;
pub const ID_MOUSE: u8 = 2;
pub const ID_KEYBOARD: u8 = 4;
pub const ID_TRACKPAD: u8 = 8;
pub const ID_KEY: u8 = 16;
const MAX_LINES: usize = 4096;
const MAX_FILE_SIZE: usize = 65536;
const MAX_INCLUDES: usize = 128;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    #[default]
    None,
    KeySequence,
    OneShot,
    OneShotM,
    OneShotK,
    LayerM,
    Swap,
    SwapM,
    Layer,
    Layout,
    Clear,
    ClearM,
    Overload,
    OverloadTimeout,
    OverloadTimeoutTap,
    OverloadIdleTimeout,
    Toggle,
    ToggleM,
    Repeat,
    Macro,
    Macro2,
    Command,
    Timeout,
    ScrollToggleOn,
    ScrollToggleOff,
    ScrollToggle,
    Scroll,
    OverloadDoubleTap,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Descriptor {
    pub op: Op,
    pub args: [u16; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayerType {
    Normal,
    Layout,
    Composite,
}

#[derive(Clone, Debug)]
pub struct Chord {
    pub keys: Vec<u8>,
    pub descriptor: Descriptor,
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub name: OsString,
    pub kind: LayerType,
    pub mods: u8,
    pub keymap: [Descriptor; 256],
    pub chords: Vec<Chord>,
    pub constituents: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct ConfigId {
    pub id: OsString,
    pub flags: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigWarning {
    pub path: Option<PathBuf>,
    pub line: Option<usize>,
    pub message: OsString,
    pub literal_markup: bool,
    pub(crate) template: &'static str,
    pub(crate) arguments: Vec<OsString>,
}

#[derive(Clone, Debug)]
pub struct Config {
    pub path: PathBuf,
    pub layers: Vec<Layer>,
    pub descriptors: Vec<Descriptor>,
    pub macros: Vec<Macro>,
    pub commands: Vec<OsString>,
    pub aliases: [Option<OsString>; 256],
    pub wildcard: u8,
    pub ids: Vec<ConfigId>,
    pub macro_timeout: i64,
    pub macro_sequence_timeout: i64,
    pub macro_repeat_timeout: i64,
    pub oneshot_timeout: i64,
    pub overload_tap_timeout: i64,
    pub chord_interkey_timeout: i64,
    pub chord_hold_timeout: i64,
    pub layer_indicator: u8,
    pub disable_modifier_guard: bool,
    pub default_layout: OsString,
    pub warnings: Vec<ConfigWarning>,
    // C resets its warning counter after reading includes.
    pub warning_count: usize,
}

impl Default for Config {
    fn default() -> Self {
        let mut config = Self {
            path: PathBuf::new(),
            layers: Vec::new(),
            descriptors: Vec::new(),
            macros: Vec::new(),
            commands: Vec::new(),
            aliases: std::array::from_fn(|_| None),
            wildcard: 0,
            ids: Vec::new(),
            macro_timeout: 600,
            macro_sequence_timeout: 0,
            macro_repeat_timeout: 50,
            oneshot_timeout: 0,
            overload_tap_timeout: 0,
            chord_interkey_timeout: 50,
            chord_hold_timeout: 0,
            layer_indicator: 0,
            disable_modifier_guard: false,
            default_layout: OsString::new(),
            warnings: Vec::new(),
            warning_count: 0,
        };
        for (name, kind, mods) in [
            ("main", LayerType::Layout, 0),
            ("control", LayerType::Normal, MOD_CTRL),
            ("shift", LayerType::Normal, MOD_SHIFT),
            ("meta", LayerType::Normal, MOD_SUPER),
            ("alt", LayerType::Normal, MOD_ALT),
            ("altgr", LayerType::Normal, MOD_ALT_GR),
        ] {
            config.layers.push(Layer {
                name: name.into(),
                kind,
                mods,
                keymap: [Descriptor::default(); 256],
                chords: Vec::new(),
                constituents: Vec::new(),
            });
        }
        for (key, alias, layer) in [
            (KEYD_LEFTSHIFT, "shift", 2),
            (KEYD_RIGHTSHIFT, "shift", 2),
            (KEYD_LEFTALT, "alt", 4),
            (KEYD_RIGHTALT, "altgr", 5),
            (KEYD_LEFTMETA, "meta", 3),
            (KEYD_RIGHTMETA, "meta", 3),
            (KEYD_LEFTCTRL, "control", 1),
            (KEYD_RIGHTCTRL, "control", 1),
        ] {
            config.aliases[key as usize] = Some(alias.into());
            config.layers[0].keymap[key as usize] = Descriptor {
                op: Op::Layer,
                args: [layer, 0, 0],
            };
        }
        // C's default function bindings probe macros before parsing layer().
        crate::log::set_error("Invalid macro");
        config
    }
}

fn warning(template: &'static str, arguments: &[&[u8]]) -> ConfigWarning {
    let mut message = Vec::new();
    crate::log::write_arguments(&mut message, template.as_bytes(), arguments)
        .expect("fixed warning templates have matching arguments");
    ConfigWarning {
        path: None,
        line: None,
        message: OsString::from_vec(message),
        literal_markup: false,
        template,
        arguments: arguments
            .iter()
            .map(|bytes| OsString::from_vec(bytes.to_vec()))
            .collect(),
    }
}

fn warn(config: &mut Config, template: &'static str, arguments: &[&[u8]]) {
    config.warnings.push(warning(template, arguments));
    config.warning_count += 1;
}

fn lookup_keycode(name: &[u8]) -> Option<u8> {
    KEYCODE_TABLE.iter().enumerate().find_map(|(index, key)| {
        (key.name.is_some()
            && (key.name.map(str::as_bytes) == Some(name)
                || key.alt_name.map(str::as_bytes) == Some(name)))
        .then_some(index as u8)
        .filter(|&code| code != 0)
    })
}

pub fn config_get_layer_index(config: &Config, name: impl AsRef<OsStr>) -> Option<usize> {
    config
        .layers
        .iter()
        .position(|layer| layer.name == name.as_ref())
}

pub fn config_check_match(config: &Config, id: impl AsRef<OsStr>, flags: u8) -> i32 {
    for entry in &config.ids {
        if id.as_ref().as_bytes().starts_with(entry.id.as_bytes()) {
            if entry.flags & ID_EXCLUDED != 0 {
                return 0;
            }
            if entry.flags & flags != 0 {
                return 2;
            }
        }
    }
    i32::from(config.wildcard != 0)
}

// atoi accepts a numeric prefix and ignores trailing text.
fn atoi(value: &[u8]) -> i32 {
    let value = value.trim_ascii_start();
    let (negative, digits) = match value.first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let mut result = 0u64;
    for &byte in digits.iter().take_while(|byte| byte.is_ascii_digit()) {
        result = result.wrapping_mul(10).wrapping_add(u64::from(byte - b'0'));
    }
    if negative {
        result.wrapping_neg() as i32
    } else {
        result as i32
    }
}

fn truncate(value: &[u8], max: usize) -> OsString {
    OsString::from_vec(value[..value.len().min(max)].to_vec())
}

fn add_layer(config: &mut Config, section: &OsStr) -> Result<(), ParseError> {
    add_layer_inner(config, section)
        .inspect_err(|error| crate::log::set_error_bytes(error.as_bytes()))
}

fn add_layer_inner(config: &mut Config, section: &OsStr) -> Result<(), ParseError> {
    let section = section.as_bytes();
    if section.len() > MAX_LAYER_NAME_LEN {
        return Err([
            section,
            format!(" exceeds maximum section length ({MAX_LAYER_NAME_LEN}) (ignoring)").as_bytes(),
        ]
        .concat()
        .into());
    }
    let mut parts = section
        .split(|&byte| byte == b':')
        .filter(|s| !s.is_empty());
    let name = parts.next().ok_or("invalid layer name")?;
    let kind = parts.next();
    if config_get_layer_index(config, OsStr::from_bytes(name)).is_some() {
        return Ok(());
    }
    if config.layers.len() == MAX_LAYERS {
        return Err(format!("max layers ({MAX_LAYERS}) exceeded").into());
    }
    let mut layer = Layer {
        name: OsString::from_vec(name.to_vec()),
        kind: LayerType::Normal,
        mods: 0,
        keymap: [Descriptor::default(); 256],
        chords: Vec::new(),
        constituents: Vec::new(),
    };
    if name.contains(&b'+') {
        if kind.is_some() {
            return Err("composite layers cannot have a type.".into());
        }
        layer.kind = LayerType::Composite;
        for constituent in name.split(|&byte| byte == b'+').filter(|s| !s.is_empty()) {
            let index = config_get_layer_index(config, OsStr::from_bytes(constituent))
                .ok_or_else(|| [constituent, b" is not a valid layer"].concat())?;
            if layer.constituents.len() == 8 {
                return Err("max composite layers (8) exceeded".into());
            }
            layer.constituents.push(index);
        }
    } else if kind == Some(b"layout".as_slice()) {
        layer.kind = LayerType::Layout;
    } else if let Some(kind) = kind {
        if let Some(mods) = std::str::from_utf8(kind).ok().and_then(parse_modset) {
            layer.mods = mods;
        } else {
            warn(
                config,
                "\"%s\" is not a valid layer type, ignoring",
                &[kind],
            );
        }
    }
    config.layers.push(layer);
    Ok(())
}

struct Function<'a> {
    name: &'a [u8],
    args: Vec<&'a [u8]>,
}

fn parse_fn(input: &[u8]) -> Result<Option<Function<'_>>, ParseError> {
    let Some(open) = input.iter().position(|&byte| byte == b'(') else {
        return Ok(None);
    };
    let name = &input[..open];
    let bytes = input;
    let mut start = open + 1;
    while bytes.get(start) == Some(&b' ') {
        start += 1;
    }
    let mut cursor = start;
    let mut depth = 0i32;
    let mut args = Vec::new();
    while cursor < bytes.len() {
        match bytes[cursor] {
            b'\\' if cursor + 1 < bytes.len() => {
                cursor += 2;
                continue;
            }
            b'(' => depth += 1,
            b')' => depth -= 1,
            _ => (),
        }
        if depth == -1 || (bytes[cursor] == b',' && depth == 0) {
            if start != cursor {
                if args.len() == 5 {
                    return Err(ParseError::Fatal(
                        "maximum function arguments (5) exceeded".into(),
                    ));
                }
                args.push(&input[start..cursor]);
            }
            if depth == -1 {
                return Ok(Some(Function { name, args }));
            }
            cursor += 1;
            while bytes.get(cursor) == Some(&b' ') {
                cursor += 1;
            }
            start = cursor;
            continue;
        }
        cursor += 1;
    }
    Ok(None)
}

pub fn parse_macro_expression(input: &str) -> Result<Macro, String> {
    parse_macro_expression_bytes(input.as_bytes())
        .map_err(|error| String::from_utf8_lossy(&error).into_owned())
}

fn parse_macro_expression_bytes(input: &[u8]) -> Result<Macro, Vec<u8>> {
    parse_macro_expression_inner(input).inspect_err(|error| crate::log::set_error_bytes(error))
}

fn parse_macro_expression_inner(input: &[u8]) -> Result<Macro, Vec<u8>> {
    let input = input.split(|&byte| byte == 0).next().unwrap();
    if input.len() >= 1024 {
        return Err(b"macro size exceeded maximum size (1024)".to_vec());
    }
    if let Some(body) = input
        .strip_prefix(b"macro(")
        .and_then(|s| s.strip_suffix(b")"))
    {
        macro_parse_bytes(&str_escape_bytes(body))
    } else if std::str::from_utf8(input)
        .ok()
        .and_then(parse_key_sequence)
        .is_some()
        || utf8_strlen_bytes(input).map_err(String::into_bytes)? == 1
    {
        macro_parse_bytes(input)
    } else {
        Err(b"Invalid macro".to_vec())
    }
}

#[derive(Clone, Copy)]
enum Arg {
    Layer,
    Layout,
    Macro,
    Descriptor,
    KeySequence,
    Timeout,
    Sensitivity,
}

fn action(name: &str) -> Option<(Op, &'static [Arg], Option<&'static str>)> {
    use Arg::*;
    Some(match name {
        "swap" => (Op::Swap, &[Layer], None),
        "clear" => (Op::Clear, &[], None),
        "oneshot" => (Op::OneShot, &[Layer], None),
        "toggle" => (Op::Toggle, &[Layer], None),
        "clearm" => (Op::ClearM, &[Macro], None),
        "swapm" => (Op::SwapM, &[Layer, Macro], None),
        "togglem" => (Op::ToggleM, &[Layer, Macro], None),
        "layerm" => (Op::LayerM, &[Layer, Macro], None),
        "oneshotm" => (Op::OneShotM, &[Layer, Macro], None),
        "oneshotk" => (Op::OneShotK, &[Layer, KeySequence], None),
        "layer" => (Op::Layer, &[Layer], None),
        "overload" => (Op::Overload, &[Layer, Descriptor], None),
        "overloadd" => (
            Op::OverloadDoubleTap,
            &[Layer, KeySequence, KeySequence, Timeout],
            None,
        ),
        "overloadt" => (Op::OverloadTimeout, &[Layer, Descriptor, Timeout], None),
        "overloadt2" => (Op::OverloadTimeoutTap, &[Layer, Descriptor, Timeout], None),
        "overloadi" => (
            Op::OverloadIdleTimeout,
            &[Descriptor, Descriptor, Timeout],
            None,
        ),
        "timeout" => (Op::Timeout, &[Descriptor, Timeout, Descriptor], None),
        "macro2" => (Op::Macro2, &[Timeout, Timeout, Macro], None),
        "setlayout" => (Op::Layout, &[Layout], None),
        "repeat" => (Op::Repeat, &[], None),
        "scrollon" => (Op::ScrollToggleOn, &[Sensitivity], None),
        "scrolloff" => (Op::ScrollToggleOff, &[], None),
        "scrollt" => (Op::ScrollToggle, &[Sensitivity], None),
        "scroll" => (Op::Scroll, &[Sensitivity], None),
        "overload2" => (
            Op::OverloadTimeout,
            &[Layer, Descriptor, Timeout],
            Some("overloadt"),
        ),
        "overload3" => (
            Op::OverloadTimeoutTap,
            &[Layer, Descriptor, Timeout],
            Some("overloadt2"),
        ),
        "toggle2" => (Op::ToggleM, &[Layer, Macro], Some("togglem")),
        "swap2" => (Op::SwapM, &[Layer, Macro], Some("swapm")),
        _ => return None,
    })
}

fn parse_descriptor(config: &mut Config, input: &[u8]) -> Result<Descriptor, ParseError> {
    let mut descriptor = Descriptor::default();
    if input.is_empty() {
        return Ok(descriptor);
    }
    if let Some((code, mods)) = std::str::from_utf8(input).ok().and_then(parse_key_sequence) {
        let layer = match code {
            KEYD_LEFTSHIFT => Some("shift"),
            KEYD_LEFTCTRL => Some("control"),
            KEYD_LEFTMETA => Some("meta"),
            KEYD_LEFTALT => Some("alt"),
            KEYD_RIGHTALT => Some("altgr"),
            _ => None,
        };
        if let Some(layer) = layer {
            let name = KEYCODE_TABLE[code as usize].name.unwrap_or("UNKNOWN");
            warn(
                config,
                "You should use b{layer(%s)} instead of assigning to b{%s} directly.",
                &[layer.as_bytes(), name.as_bytes()],
            );
            // This markup contains only fixed layer names and the built-in key table.
            config.warnings.last_mut().unwrap().literal_markup = true;
            descriptor.op = Op::Layer;
            descriptor.args[0] = config_get_layer_index(config, layer)
                .ok_or_else(|| format!("{layer} is not a valid layer"))?
                as u16;
        } else {
            descriptor.op = Op::KeySequence;
            descriptor.args[0] = code as u16;
            descriptor.args[1] = mods as u16;
        }
        return Ok(descriptor);
    }
    if let Some(body) = input
        .strip_prefix(b"command(")
        .and_then(|s| s.strip_suffix(b")"))
    {
        if input.len() > 256 {
            return Err("max command length (256) exceeded".into());
        }
        if config.commands.len() == MAX_COMMANDS {
            return Err(format!("max commands ({MAX_COMMANDS}), exceeded").into());
        }
        descriptor.op = Op::Command;
        descriptor.args[0] = config.commands.len() as u16;
        config
            .commands
            .push(OsString::from_vec(str_escape_bytes(body)));
        return Ok(descriptor);
    }
    let candidate = parse_macro_expression_bytes(input);
    if let Ok(entry) = candidate {
        if config.macros.len() == MAX_MACROS {
            return Err(format!("max macros ({MAX_MACROS}), exceeded").into());
        }
        descriptor.op = Op::Macro;
        descriptor.args[0] = config.macros.len() as u16;
        config.macros.push(entry);
        return Ok(descriptor);
    } else if (input.starts_with(b"macro(") && input.ends_with(b")"))
        || utf8_strlen_bytes(input).is_ok_and(|count| count == 1)
    {
        return Err(candidate.unwrap_err().into());
    }
    let Function { name, args } = parse_fn(input)?.ok_or("invalid key or action")?;
    if name == b"lettermod" {
        if args.len() != 4 {
            return Err("lettermod requires 4 arguments".into());
        }
        let rewritten = [
            b"overloadi(".as_slice(),
            args[1],
            b", overloadt2(",
            args[0],
            b", ",
            args[1],
            b", ",
            args[3],
            b"), ",
            args[2],
            b")",
        ]
        .concat();
        let rewritten = &rewritten[..rewritten.len().min(1023)];
        // Keep the rewritten expression alive while its argument slices are used.
        return parse_descriptor(config, rewritten);
    }
    let name = std::str::from_utf8(name).map_err(|_| "invalid key or action")?;
    let (op, types, preferred) = action(name).ok_or("invalid key or action")?;
    if let Some(preferred) = preferred {
        warn(
            config,
            "%s is deprecated (renamed to %s).",
            &[name.as_bytes(), preferred.as_bytes()],
        );
    }
    if args.len() != types.len() {
        return Err(format!(
            "{name} requires {} {}",
            types.len(),
            if types.len() == 1 {
                "argument"
            } else {
                "arguments"
            }
        )
        .into());
    }
    descriptor.op = op;
    if op == Op::OverloadDoubleTap
        && !std::str::from_utf8(args[3])
            .ok()
            .and_then(|value| value.parse::<u16>().ok())
            .is_some_and(|value| value > 0)
    {
        return Err("overloadd timeout must be between 1 and 65535 milliseconds".into());
    }
    let mut values = [0; 4];
    // C allocates nested descriptors and macros from the last argument first.
    for index in (0..types.len()).rev() {
        let value = args[index];
        values[index] = match types[index] {
            Arg::Layer | Arg::Layout => {
                let layout = matches!(types[index], Arg::Layout);
                if !layout && value == b"main" {
                    return Err("the main layer cannot be toggled".into());
                }
                let found = config_get_layer_index(config, OsStr::from_bytes(value));
                let valid = found.is_some_and(|i| {
                    if layout {
                        i == 0 || config.layers[i].kind == LayerType::Layout
                    } else {
                        config.layers[i].kind != LayerType::Layout
                    }
                });
                if !valid {
                    return Err([
                        value,
                        if layout {
                            b" is not a valid layout".as_slice()
                        } else {
                            b" is not a valid layer".as_slice()
                        },
                    ]
                    .concat()
                    .into());
                }
                found.unwrap() as u16
            }
            Arg::Descriptor | Arg::KeySequence => {
                let nested = parse_descriptor(config, value)?;
                if config.descriptors.len() == MAX_DESCRIPTORS {
                    return Err("maximum descriptors exceeded".into());
                }
                if matches!(types[index], Arg::KeySequence) && nested.op != Op::KeySequence {
                    return Err([value, b" is not a valid keysequence"].concat().into());
                }
                let index = config.descriptors.len() as u16;
                config.descriptors.push(nested);
                index
            }
            Arg::Macro => {
                if config.macros.len() == MAX_MACROS {
                    return Err(format!("max macros ({MAX_MACROS}), exceeded").into());
                }
                let entry = parse_macro_expression_bytes(value)?;
                let index = config.macros.len() as u16;
                config.macros.push(entry);
                index
            }
            Arg::Timeout | Arg::Sensitivity => atoi(value) as u16,
        };
    }
    if op == Op::OverloadDoubleTap {
        if config.descriptors.len() == MAX_DESCRIPTORS {
            return Err("maximum descriptors exceeded".into());
        }
        descriptor.args = [values[0], config.descriptors.len() as u16, 0];
        config.descriptors.push(Descriptor {
            op: Op::Timeout,
            args: [values[1], values[3], values[2]],
        });
    } else {
        descriptor.args.copy_from_slice(&values[..3]);
    }
    Ok(descriptor)
}

pub fn config_add_entry(
    config: &mut Config,
    expression: impl AsRef<OsStr>,
) -> Result<(), ParseError> {
    let expression = expression
        .as_ref()
        .as_bytes()
        .split(|&byte| byte == 0)
        .next()
        .unwrap();
    add_entry(config, expression).inspect_err(|error| crate::log::set_error_bytes(error.as_bytes()))
}

fn add_entry(config: &mut Config, expression: &[u8]) -> Result<(), ParseError> {
    if expression.len() >= MAX_EXP_LEN {
        return Err([
            expression,
            format!(" exceeds maximum expression length ({MAX_EXP_LEN})").as_bytes(),
        ]
        .concat()
        .into());
    }
    let dot = expression.iter().position(|&byte| byte == b'.');
    let paren = expression.iter().position(|&byte| byte == b'(');
    let (layer_name, binding) = match dot {
        Some(dot) if dot != 0 && paren.is_none_or(|paren| dot < paren) => {
            (&expression[..dot], &expression[dot + 1..])
        }
        _ => (b"main".as_slice(), expression),
    };
    let (key, value) = parse_kvp_bytes(binding);
    let index = config_get_layer_index(config, OsStr::from_bytes(layer_name))
        .ok_or_else(|| [layer_name, b" is not a valid layer"].concat())?;
    let descriptor = parse_descriptor(config, value.unwrap_or(b""))?;
    if key.contains(&b'+') {
        let mut keys = Vec::new();
        for key in key.split(|&byte| byte == b'+').filter(|s| !s.is_empty()) {
            let code = lookup_keycode(key).ok_or_else(|| [key, b" is not a valid key"].concat())?;
            if keys.len() == MAX_CHORD_KEYS {
                return Err(format!("chords cannot contain more than {} keys", keys.len()).into());
            }
            keys.push(code);
        }
        let layer = &mut config.layers[index];
        // C replaces an existing chord when all new keys occur in it, even a subset.
        if let Some(chord) = layer
            .chords
            .iter_mut()
            .find(|chord| keys.iter().all(|key| chord.keys.contains(key)))
        {
            chord.descriptor = descriptor;
        } else {
            if layer.chords.len() == MAX_CHORDS {
                return Err(format!("max chords exceeded({})", layer.chords.len()).into());
            }
            layer.chords.push(Chord { keys, descriptor });
        }
    } else {
        let mut found = false;
        for (code, alias) in config.aliases.iter().enumerate() {
            if alias.as_deref().map_or(b"".as_slice(), OsStr::as_bytes) == key {
                config.layers[index].keymap[code] = descriptor;
                found = true;
            }
        }
        if !found {
            let code = lookup_keycode(key)
                .ok_or_else(|| [key, b" is not a valid key or alias"].concat())?;
            config.layers[index].keymap[code as usize] = descriptor;
        }
    }
    Ok(())
}

fn parse_global_section(config: &mut Config, section: &IniSection) {
    for entry in &section.entries {
        let Some(value) = entry.value.as_deref() else {
            warn(config, "invalid global option", &[]);
            continue;
        };
        let value = value.as_bytes();
        let number = atoi(value) as i64;
        match entry.key.as_bytes() {
            b"macro_timeout" => config.macro_timeout = number,
            b"macro_sequence_timeout" => config.macro_sequence_timeout = number,
            b"disable_modifier_guard" => config.disable_modifier_guard = number as u8 != 0,
            b"oneshot_timeout" => config.oneshot_timeout = number,
            b"chord_hold_timeout" => config.chord_hold_timeout = number,
            b"chord_timeout" => config.chord_interkey_timeout = number,
            b"default_layout" => config.default_layout = truncate(value, MAX_LAYER_NAME_LEN - 1),
            b"macro_repeat_timeout" => config.macro_repeat_timeout = number,
            b"layer_indicator" => config.layer_indicator = number as u8,
            b"overload_tap_timeout" => config.overload_tap_timeout = number,
            _ => warn(
                config,
                "%s is not a valid global option",
                &[entry.key.as_bytes()],
            ),
        }
    }
}

fn parse_id_section(config: &mut Config, section: &IniSection) -> Result<(), ParseError> {
    for entry in &section.entries {
        let key = entry.key.as_bytes();
        if key == b"*" {
            config.wildcard = 1;
            continue;
        }
        let (id, flags) = if let Some(id) = key.strip_prefix(b"m:") {
            (truncate(id, 63), ID_MOUSE)
        } else if let Some(id) = key.strip_prefix(b"k:") {
            (truncate(id, 63), ID_KEYBOARD | ID_KEY)
        } else if let Some(id) = key.strip_prefix(b"-") {
            (truncate(id, 63), ID_EXCLUDED)
        } else if key.len() < 63 {
            (
                OsString::from_vec(key.to_vec()),
                ID_KEYBOARD | ID_KEY | ID_MOUSE,
            )
        } else {
            warn(config, "%s is not a valid device id", &[key]);
            continue;
        };
        if config.ids.len() == MAX_IDS {
            return Err(ParseError::Fatal(format!(
                "max device ids ({MAX_IDS}) exceeded"
            )));
        }
        config.ids.push(ConfigId { id, flags });
    }
    Ok(())
}

fn parse_alias_section(config: &mut Config, section: &IniSection) {
    for entry in &section.entries {
        let Some(name) = entry.value.as_deref() else {
            warn(config, "invalid alias", &[]);
            continue;
        };
        if let Some(code) = lookup_keycode(entry.key.as_bytes()) {
            if name.len() >= 32 {
                warn(
                    config,
                    "%s exceeds the maximum alias length (%ld)",
                    &[name.as_bytes(), b"31"],
                );
            } else {
                if let Some(alias_code) = lookup_keycode(name.as_bytes()) {
                    config.layers[0].keymap[code as usize] = Descriptor {
                        op: Op::KeySequence,
                        args: [alias_code as u16, 0, 0],
                    };
                }
                config.aliases[code as usize] = Some(name.to_os_string());
            }
        } else {
            warn(
                config,
                "failed to define alias %s, %s is not a valid keycode",
                &[name.as_bytes(), entry.key.as_bytes()],
            );
        }
    }
}

fn read_line(reader: &mut impl BufRead, path: &Path) -> Result<Option<Vec<u8>>, ParseError> {
    let mut line = Vec::new();
    let count = reader
        .take(4097)
        .read_until(b'\n', &mut line)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if count == 0 {
        return Ok(None);
    }
    if line.last() == Some(&b'\n') {
        line.pop();
    }
    if line.len() >= 4096 {
        return Err(ParseError::Fatal(
            "maximum config line length (4095) exceeded".into(),
        ));
    }
    // C strings ignore bytes following a NUL in each physical line.
    if let Some(nul) = line.iter().position(|&byte| byte == 0) {
        line.truncate(nul);
    }
    Ok(Some(line))
}

fn resolve_include_path(config_path: &Path, include: &[u8]) -> Result<Option<PathBuf>, ParseError> {
    let parent = config_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        // C ignores dirname's returned "." for a path without a slash.
        .unwrap_or(config_path);
    if parent.as_os_str().as_encoded_bytes().len() + include.len() + 2 >= 4096 {
        return Err(ParseError::Fatal(
            "maximum include path length (4095) exceeded".into(),
        ));
    }
    for directory in [parent, Path::new(crate::DATA_DIR)] {
        // C concatenates paths rather than treating an absolute include specially.
        let mut candidate = directory.as_os_str().to_os_string();
        candidate.push("/");
        candidate.push(OsStr::from_bytes(include));
        let candidate = PathBuf::from(candidate);
        let Ok(root) = directory.canonicalize() else {
            continue;
        };
        let Ok(resolved) = candidate.canonicalize() else {
            continue;
        };
        if resolved
            .as_os_str()
            .as_encoded_bytes()
            .starts_with(root.as_os_str().as_encoded_bytes())
        {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn append_line(
    content: &mut Vec<u8>,
    source_map: &mut Vec<(PathBuf, usize)>,
    line: &[u8],
    path: &Path,
    number: usize,
) -> Result<(), ParseError> {
    if content.len() + line.len() + 2 >= MAX_FILE_SIZE {
        return Err(ParseError::Fatal(format!(
            "maximum config file size ({MAX_FILE_SIZE}) exceeded"
        )));
    }
    if source_map.len() == MAX_LINES {
        return Err(ParseError::Fatal(format!(
            "maximum config lines ({MAX_LINES}) exceeded"
        )));
    }
    content.extend_from_slice(line);
    content.push(b'\n');
    source_map.push((path.into(), number));
    Ok(())
}

#[derive(Debug)]
pub(crate) enum LoadError {
    Parse(ParseError),
    Output(std::io::Error),
}

impl From<ParseError> for LoadError {
    fn from(error: ParseError) -> Self {
        Self::Parse(error)
    }
}

impl From<std::io::Error> for LoadError {
    fn from(error: std::io::Error) -> Self {
        Self::Output(error)
    }
}

fn emit_warnings(
    config: &mut Config,
    emit: &mut impl FnMut(&ConfigWarning) -> std::io::Result<()>,
) -> Result<(), LoadError> {
    for warning in config.warnings.drain(..) {
        emit(&warning)?;
    }
    Ok(())
}

pub fn config_parse(path: impl AsRef<Path>) -> Result<Config, ParseError> {
    let mut warnings = Vec::new();
    match config_parse_with_warnings(path, |warning| {
        warnings.push(warning.clone());
        Ok(())
    }) {
        Ok(mut config) => {
            config.warnings = warnings;
            Ok(config)
        }
        Err(LoadError::Parse(error)) => Err(error),
        Err(LoadError::Output(_)) => unreachable!("warning collection cannot fail"),
    }
}

pub(crate) fn config_parse_with_warnings(
    path: impl AsRef<Path>,
    mut emit: impl FnMut(&ConfigWarning) -> std::io::Result<()>,
) -> Result<Config, LoadError> {
    let path = path.as_ref();
    let file = File::open(path)
        .map_err(|error| ParseError::from(format!("{}: {error}", path.display())))?;
    let mut reader = BufReader::new(file);
    let mut config = Config {
        path: path.into(),
        ..Config::default()
    };
    let mut content = Vec::new();
    let mut source_map: Vec<(PathBuf, usize)> = Vec::new();
    let mut includes = 0;
    let mut number = 1;
    while let Some(line) = read_line(&mut reader, path)? {
        if let Some(include) = line.strip_prefix(b"include ") {
            if includes == MAX_INCLUDES {
                return Err(ParseError::Fatal(format!(
                    "maximum includes ({MAX_INCLUDES}) exceeded"
                ))
                .into());
            }
            let Some(resolved) = resolve_include_path(path, include)? else {
                config.warnings.push(ConfigWarning {
                    path: Some(path.into()),
                    line: Some(number),
                    ..warning("failed to resolve include path %s", &[include])
                });
                emit_warnings(&mut config, &mut emit)?;
                number += 1;
                continue;
            };
            let included_file = match File::open(&resolved) {
                Ok(file) => file,
                Err(_) => {
                    config.warnings.push(ConfigWarning {
                        path: Some(path.into()),
                        line: Some(number),
                        ..warning("failed to open %s", &[resolved.as_os_str().as_bytes()])
                    });
                    emit_warnings(&mut config, &mut emit)?;
                    number += 1;
                    continue;
                }
            };
            includes += 1;
            let mut included_reader = BufReader::new(included_file);
            let mut included_number = 1;
            while let Some(line) = read_line(&mut included_reader, &resolved)? {
                append_line(
                    &mut content,
                    &mut source_map,
                    &line,
                    &resolved,
                    included_number,
                )?;
                included_number += 1;
            }
        } else {
            append_line(&mut content, &mut source_map, &line, path, number)?;
        }
        number += 1;
    }
    let sections = match ini_parse_bytes(&content, None) {
        Ok(sections) => sections,
        Err(error) if error.is_fatal() => return Err(error.into()),
        Err(error) => {
            warn(&mut config, "%s", &[error.as_bytes()]);
            emit_warnings(&mut config, &mut emit)?;
            return Ok(config);
        }
    };
    for section in &sections {
        let result = match section.name.as_bytes() {
            b"ids" => parse_id_section(&mut config, section),
            b"aliases" => {
                parse_alias_section(&mut config, section);
                Ok(())
            }
            b"global" => {
                parse_global_section(&mut config, section);
                Ok(())
            }
            _ => {
                if let Err(error) = add_layer(&mut config, &section.name) {
                    warn(&mut config, "%s", &[error.as_bytes()]);
                }
                Ok(())
            }
        };
        emit_warnings(&mut config, &mut emit)?;
        result?;
    }
    for section in &sections {
        if matches!(section.name.as_bytes(), b"ids" | b"aliases" | b"global") {
            continue;
        }
        let name = section
            .name
            .as_bytes()
            .split(|&byte| byte == b':')
            .find(|s| !s.is_empty())
            .unwrap_or(b"");
        for entry in &section.entries {
            let previous_warnings = config.warnings.len();
            let result = match &entry.value {
                None => {
                    warn(&mut config, "invalid binding", &[]);
                    Ok(())
                }
                Some(value) => {
                    let expression = truncate(
                        &[name, b".", entry.key.as_bytes(), b" = ", value.as_bytes()].concat(),
                        MAX_EXP_LEN - 1,
                    );
                    match config_add_entry(&mut config, &expression) {
                        Err(error) if error.is_fatal() => Err(error),
                        Err(error) => {
                            warn(&mut config, "%s", &[error.as_bytes()]);
                            Ok(())
                        }
                        Ok(()) => Ok(()),
                    }
                }
            };
            if let Some((path, line)) = source_map.get(entry.line - 1) {
                for warning in &mut config.warnings[previous_warnings..] {
                    warning.path = Some(path.clone());
                    warning.line = Some(*line);
                }
            }
            emit_warnings(&mut config, &mut emit)?;
            result?;
        }
    }
    Ok(config)
}
