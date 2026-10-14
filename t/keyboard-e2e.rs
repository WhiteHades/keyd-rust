use keyd_rust::config::{Config, config_parse};
use keyd_rust::keyboard::{KeyEvent, Keyboard, LayerState, Output};
use keyd_rust::keys::*;
use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

#[derive(Default)]
struct Capture {
    keys: Vec<(u8, u8)>,
    layers: Vec<(OsString, bool)>,
    commands: Vec<OsString>,
}

impl Output for Capture {
    fn send_key(&mut self, code: u8, state: u8) {
        self.keys.push((code, state));
    }
    fn on_layer_change(&mut self, config: &Config, _: &[LayerState], layer: usize, active: bool) {
        self.layers
            .push((config.layers[layer].name.clone(), active));
    }
    fn execute_command(&mut self, command: &OsStr) {
        self.commands.push(command.into());
    }
}

fn code(name: &str) -> u8 {
    match name {
        "control" => KEYD_LEFTCTRL,
        "shift" => KEYD_LEFTSHIFT,
        "meta" => KEYD_LEFTMETA,
        "alt" => KEYD_LEFTALT,
        _ => KEYCODE_TABLE
            .iter()
            .position(|entry| entry.name == Some(name))
            .unwrap() as u8,
    }
}

fn fixture(path: &Path) -> (Vec<KeyEvent>, Vec<(u8, u8)>) {
    let text = std::fs::read_to_string(path).unwrap();
    let mut input = Vec::new();
    let mut expected = Vec::new();
    let mut output = false;
    let mut time = 0i32;
    for line in text.lines() {
        if line.starts_with('#') {
            continue;
        }
        let line = line.trim_start_matches(' ');
        if line.is_empty() {
            output = true;
            continue;
        }
        if let Some(delay) = line.strip_suffix("ms") {
            time += delay.parse::<i32>().unwrap();
            continue;
        }
        let mut words = line.split_whitespace();
        let code = code(words.next().unwrap());
        let state = match words.next().unwrap() {
            "down" => 1,
            "up" => 0,
            value => panic!("invalid state {value}"),
        };
        assert!(words.next().is_none());
        if output {
            expected.push((code, state));
        } else {
            input.push(KeyEvent {
                code,
                pressed: state,
                timestamp: time,
            });
        }
    }
    assert!(output, "fixture must include independent expected events");
    (input, expected)
}

fn main() {
    let mut args = std::env::args_os().skip(1);
    let config = config_parse(args.next().expect("configuration path required")).unwrap();
    let mut keyboard = Keyboard::new(config, Capture::default());
    let mut checked = 0;
    for path in args {
        let path = Path::new(&path);
        let (input, expected) = fixture(path);
        keyboard.output.keys.clear();
        keyboard.process_events(&input);
        assert_eq!(keyboard.output.keys, expected, "{}", path.display());
        checked += 1;
    }
    assert!(checked > 0);

    let mut keyboard = Keyboard::new(Config::default(), Capture::default());
    keyboard.eval("a = b").unwrap();
    keyboard.process_events(&[
        KeyEvent {
            code: KEYD_A,
            pressed: 1,
            timestamp: 0,
        },
        KeyEvent {
            code: KEYD_A,
            pressed: 1,
            timestamp: 1,
        },
    ]);
    keyboard.eval("a = c").unwrap();
    keyboard.process_events(&[KeyEvent {
        code: KEYD_A,
        pressed: 0,
        timestamp: 2,
    }]);
    assert_eq!(keyboard.output.keys, [(KEYD_B, 1), (KEYD_B, 0)]);
    keyboard.eval("reset").unwrap();
    keyboard.process_events(&[
        KeyEvent {
            code: KEYD_A,
            pressed: 1,
            timestamp: 3,
        },
        KeyEvent {
            code: KEYD_A,
            pressed: 0,
            timestamp: 4,
        },
    ]);
    assert_eq!(&keyboard.output.keys[2..], [(KEYD_A, 1), (KEYD_A, 0)]);
    assert!(keyboard.eval("missing.a = b").is_err());

    keyboard.eval("f1 = command(true)").unwrap();
    keyboard.process_events(&[
        KeyEvent {
            code: KEYD_F1,
            pressed: 1,
            timestamp: 5,
        },
        KeyEvent {
            code: KEYD_F1,
            pressed: 0,
            timestamp: 6,
        },
    ]);
    assert_eq!(keyboard.output.commands, ["true"]);

    let mut raw_keyboard = Keyboard::new(Config::default(), Capture::default());
    raw_keyboard
        .eval(OsStr::from_bytes(b"f1 = command(printf '\xff')"))
        .unwrap();
    raw_keyboard.process_events(&[
        KeyEvent {
            code: KEYD_F1,
            pressed: 1,
            timestamp: 0,
        },
        KeyEvent {
            code: KEYD_F1,
            pressed: 0,
            timestamp: 1,
        },
    ]);
    assert_eq!(
        raw_keyboard.output.commands,
        [OsStr::from_bytes(b"printf '\xff'")]
    );
    raw_keyboard.eval(OsStr::from_bytes(b"reset")).unwrap();
    assert!(raw_keyboard.config.commands.is_empty());

    keyboard.eval("f2 = toggle(control)").unwrap();
    keyboard.process_events(&[
        KeyEvent {
            code: KEYD_F2,
            pressed: 1,
            timestamp: 7,
        },
        KeyEvent {
            code: KEYD_F2,
            pressed: 0,
            timestamp: 8,
        },
        KeyEvent {
            code: KEYD_F2,
            pressed: 1,
            timestamp: 9,
        },
        KeyEvent {
            code: KEYD_F2,
            pressed: 0,
            timestamp: 10,
        },
    ]);
    assert_eq!(
        keyboard.output.layers,
        [("control".into(), true), ("control".into(), false)]
    );
    assert!(keyboard.keystate.iter().all(|&state| state == 0));

    let mut keyboard = Keyboard::new(Config::default(), Capture::default());
    keyboard.eval("f10 = macro(b)").unwrap();
    assert_eq!(
        keyboard.process_events(&[KeyEvent {
            code: KEYD_F10,
            pressed: 1,
            timestamp: 0
        }]),
        600
    );
    assert_eq!(keyboard.output.keys, [(KEYD_B, 1), (KEYD_B, 0)]);
    keyboard.eval("reset").unwrap();
    assert_eq!(
        keyboard.process_events(&[KeyEvent {
            timestamp: 600,
            ..KeyEvent::default()
        }]),
        50
    );
    keyboard.process_events(&[KeyEvent {
        code: KEYD_F10,
        timestamp: 601,
        ..KeyEvent::default()
    }]);
    assert_eq!(keyboard.output.keys, [(KEYD_B, 1), (KEYD_B, 0)]);

    let mut keyboard = Keyboard::new(Config::default(), Capture::default());
    keyboard.eval("f10 = overload(control, macro(b))").unwrap();
    keyboard.process_events(&[KeyEvent {
        code: KEYD_F10,
        pressed: 1,
        timestamp: 0,
    }]);
    keyboard.eval("reset").unwrap();
    keyboard.process_events(&[KeyEvent {
        code: KEYD_F10,
        pressed: 0,
        timestamp: 1,
    }]);
    assert_eq!(
        keyboard.output.keys,
        [(KEYD_LEFTCTRL, 1), (KEYD_LEFTCTRL, 0)]
    );

    for release_before_timeout in [false, true] {
        let config = Config {
            chord_hold_timeout: 100,
            ..Config::default()
        };
        let mut keyboard = Keyboard::new(config, Capture::default());
        keyboard.eval("a+s = b").unwrap();
        assert_eq!(
            keyboard.process_events(&[
                KeyEvent {
                    code: KEYD_A,
                    pressed: 1,
                    timestamp: 0
                },
                KeyEvent {
                    code: KEYD_S,
                    pressed: 1,
                    timestamp: 1
                },
            ]),
            49
        );
        assert!(keyboard.output.keys.is_empty());
        keyboard.eval("reset").unwrap();
        if release_before_timeout {
            assert_eq!(
                keyboard.process_events(&[KeyEvent {
                    code: KEYD_A,
                    pressed: 0,
                    timestamp: 2
                }]),
                48
            );
        }
        assert_eq!(
            keyboard.process_events(&[KeyEvent {
                timestamp: 101,
                ..KeyEvent::default()
            }]),
            0
        );
        let mut expected = vec![(KEYD_A, 1), (KEYD_S, 1)];
        if release_before_timeout {
            expected.push((KEYD_A, 0));
        }
        assert_eq!(keyboard.output.keys, expected);
    }
    println!(
        "keyboard E2E passed: {checked} baseline traces, dynamic bindings, repeated events, cached releases, reset, commands, and layer notifications"
    );
}
