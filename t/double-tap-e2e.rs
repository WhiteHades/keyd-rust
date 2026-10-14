use keyd::config::{Config, config_add_entry, config_parse};
use keyd::keyboard::{KeyEvent, Keyboard, LayerState, Output};
use keyd::keys::*;
use std::ffi::OsStr;

#[derive(Default)]
struct Capture(Vec<(u8, u8)>);

impl Output for Capture {
    fn send_key(&mut self, code: u8, state: u8) {
        self.0.push((code, state));
    }
    fn on_layer_change(&mut self, _: &Config, _: &[LayerState], _: usize, _: bool) {}
    fn execute_command(&mut self, _: &OsStr) {
        panic!("double-tap key sequences must not execute commands");
    }
}

fn event(keyboard: &mut Keyboard<Capture>, code: u8, pressed: u8, timestamp: i32) -> i64 {
    keyboard.process_events(&[KeyEvent {
        code,
        pressed,
        timestamp,
    }])
}

fn tap(keyboard: &mut Keyboard<Capture>, code: u8, timestamp: i32) {
    event(keyboard, code, 1, timestamp);
    event(keyboard, code, 0, timestamp + 10);
}

fn keyboard(path: &OsStr) -> Keyboard<Capture> {
    let mut config = config_parse(path).unwrap();
    assert!(config.warnings.is_empty());
    config_add_entry(
        &mut config,
        "semicolon = overloadd(nav, semicolon, enter, 200)",
    )
    .unwrap();
    Keyboard::new(config, Capture::default())
}

fn check(keyboard: &Keyboard<Capture>, expected: &[(u8, u8)]) {
    assert_eq!(keyboard.output.0, expected);
    assert!(keyboard.keystate.iter().all(|&state| state == 0));
    assert!(
        keyboard.layer_state[1..]
            .iter()
            .all(|state| state.active == 0)
    );
}

fn main() {
    let path = std::env::args_os()
        .nth(1)
        .expect("workstation configuration path required");
    let mut k = keyboard(&path);
    event(&mut k, KEYD_SEMICOLON, 1, 0);
    assert_eq!(event(&mut k, KEYD_SEMICOLON, 0, 10), 200);
    assert!(k.output.0.is_empty());
    assert_eq!(event(&mut k, 0, 0, 209), 1);
    assert!(k.output.0.is_empty());
    assert_eq!(event(&mut k, 0, 0, 210), 0);
    check(&k, &[(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);

    let mut k = keyboard(&path);
    tap(&mut k, KEYD_SEMICOLON, 0);
    tap(&mut k, KEYD_SEMICOLON, 50);
    event(&mut k, 0, 0, 500);
    check(&k, &[(KEYD_ENTER, 1), (KEYD_ENTER, 0)]);

    let mut k = keyboard(&path);
    tap(&mut k, KEYD_SEMICOLON, 0);
    tap(&mut k, KEYD_A, 30);
    tap(&mut k, KEYD_SEMICOLON, 60);
    event(&mut k, 0, 0, 400);
    check(
        &k,
        &[
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_A, 1),
            (KEYD_A, 0),
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
        ],
    );

    for (input, output) in [
        (KEYD_H, KEYD_LEFT),
        (KEYD_J, KEYD_DOWN),
        (KEYD_K, KEYD_UP),
        (KEYD_L, KEYD_RIGHT),
    ] {
        let mut k = keyboard(&path);
        event(&mut k, KEYD_SEMICOLON, 1, 0);
        event(&mut k, input, 1, 1);
        assert_eq!(
            k.output.0,
            [(output, 1)],
            "navigation must activate without a timer"
        );
        event(&mut k, input, 1, 2);
        event(&mut k, input, 0, 3);
        event(&mut k, KEYD_SEMICOLON, 0, 4);
        event(&mut k, 0, 0, 500);
        check(&k, &[(output, 1), (output, 0)]);
    }

    let mut k = keyboard(&path);
    tap(&mut k, KEYD_SEMICOLON, 0);
    event(&mut k, KEYD_SEMICOLON, 1, 30);
    tap(&mut k, KEYD_J, 31);
    event(&mut k, KEYD_SEMICOLON, 0, 50);
    check(
        &k,
        &[
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_DOWN, 1),
            (KEYD_DOWN, 0),
        ],
    );

    let mut k = keyboard(&path);
    event(&mut k, KEYD_LEFTSHIFT, 1, 0);
    tap(&mut k, KEYD_SEMICOLON, 10);
    event(&mut k, KEYD_LEFTSHIFT, 0, 30);
    check(
        &k,
        &[
            (KEYD_LEFTSHIFT, 1),
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_LEFTSHIFT, 0),
        ],
    );

    let mut k = keyboard(&path);
    tap(&mut k, KEYD_SEMICOLON, 0);
    event(&mut k, KEYD_LEFTSHIFT, 1, 30);
    tap(&mut k, KEYD_SEMICOLON, 40);
    event(&mut k, KEYD_LEFTSHIFT, 0, 60);
    check(
        &k,
        &[
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_LEFTSHIFT, 1),
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_LEFTSHIFT, 0),
        ],
    );

    let mut k = keyboard(&path);
    event(&mut k, KEYD_LEFTCTRL, 1, 0);
    tap(&mut k, KEYD_SEMICOLON, 10);
    tap(&mut k, KEYD_SEMICOLON, 40);
    event(&mut k, KEYD_LEFTCTRL, 0, 60);
    check(
        &k,
        &[
            (KEYD_LEFTCTRL, 1),
            (KEYD_ENTER, 1),
            (KEYD_ENTER, 0),
            (KEYD_LEFTCTRL, 0),
        ],
    );

    let mut k = keyboard(&path);
    tap(&mut k, KEYD_RIGHTALT, 0);
    tap(&mut k, KEYD_SEMICOLON, 30);
    tap(&mut k, KEYD_SEMICOLON, 60);
    check(
        &k,
        &[
            (KEYD_LEFTALT, 1),
            (KEYD_LEFTCTRL, 1),
            (KEYD_LEFTALT, 0),
            (KEYD_LEFTCTRL, 0),
            (KEYD_ENTER, 1),
            (KEYD_ENTER, 0),
            (KEYD_ENTER, 1),
            (KEYD_ENTER, 0),
        ],
    );

    for second_release in [209, 210, 211, 500] {
        let mut k = keyboard(&path);
        tap(&mut k, KEYD_SEMICOLON, 0);
        event(&mut k, KEYD_SEMICOLON, 1, 50);
        event(&mut k, KEYD_SEMICOLON, 0, second_release);
        event(&mut k, 0, 0, 1000);
        if second_release < 210 {
            check(&k, &[(KEYD_ENTER, 1), (KEYD_ENTER, 0)]);
        } else {
            check(
                &k,
                &[
                    (KEYD_SEMICOLON, 1),
                    (KEYD_SEMICOLON, 0),
                    (KEYD_SEMICOLON, 1),
                    (KEYD_SEMICOLON, 0),
                ],
            );
        }
    }

    for count in [3, 4, 1000] {
        let mut k = keyboard(&path);
        for i in 0..count {
            tap(&mut k, KEYD_SEMICOLON, i * 20);
        }
        event(&mut k, 0, 0, count * 20 + 201);
        let mut expected = Vec::new();
        for _ in 0..count / 2 {
            expected.extend([(KEYD_ENTER, 1), (KEYD_ENTER, 0)]);
        }
        if count % 2 != 0 {
            expected.extend([(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);
        }
        check(&k, &expected);
    }

    let mut k = keyboard(&path);
    tap(&mut k, KEYD_SEMICOLON, 0);
    k.eval("reset").unwrap();
    event(&mut k, 0, 0, 500);
    check(&k, &[(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);

    let mut k = keyboard(&path);
    event(&mut k, KEYD_SEMICOLON, 1, 0);
    event(&mut k, KEYD_SEMICOLON, 1, 1);
    event(&mut k, KEYD_SEMICOLON, 1, 2);
    event(&mut k, KEYD_SEMICOLON, 0, 10);
    event(&mut k, 0, 0, 210);
    check(&k, &[(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);

    let mut k = keyboard(&path);
    event(&mut k, KEYD_SEMICOLON, 1, 0);
    event(&mut k, KEYD_J, 1, 1);
    event(&mut k, KEYD_SEMICOLON, 0, 2);
    event(&mut k, KEYD_J, 0, 3);
    tap(&mut k, KEYD_SEMICOLON, 10);
    event(&mut k, 0, 0, 220);
    check(
        &k,
        &[
            (KEYD_DOWN, 1),
            (KEYD_DOWN, 0),
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
        ],
    );

    let mut k = keyboard(&path);
    event(&mut k, KEYD_SEMICOLON, 1, 0);
    event(&mut k, KEYD_SEMICOLON, 0, 500);
    check(&k, &[(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);

    let mut k = keyboard(&path);
    k.config.overload_tap_timeout = 100;
    event(&mut k, KEYD_SEMICOLON, 1, 0);
    event(&mut k, KEYD_SEMICOLON, 0, 100);
    event(&mut k, 0, 0, 500);
    check(&k, &[]);

    let mut k = keyboard(&path);
    event(&mut k, KEYD_SEMICOLON, 1, 0);
    k.eval("reset").unwrap();
    event(&mut k, KEYD_SEMICOLON, 0, 10);
    event(&mut k, 0, 0, 210);
    check(&k, &[(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);

    let mut k = keyboard(&path);
    k.eval("nav.h = swap(control)").unwrap();
    event(&mut k, KEYD_SEMICOLON, 1, 0);
    tap(&mut k, KEYD_H, 1);
    event(&mut k, KEYD_SEMICOLON, 0, 20);
    check(&k, &[(KEYD_LEFTCTRL, 1), (KEYD_LEFTCTRL, 0)]);

    let mut k = keyboard(&path);
    k.eval("a = overloadd(nav, a, tab, 200)").unwrap();
    tap(&mut k, KEYD_SEMICOLON, 0);
    tap(&mut k, KEYD_A, 20);
    event(&mut k, 0, 0, 230);
    check(
        &k,
        &[
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_A, 1),
            (KEYD_A, 0),
        ],
    );

    let mut k = keyboard(&path);
    k.eval("f1 = timeout(overloadd(nav, semicolon, enter, 200), 100, noop)")
        .unwrap();
    tap(&mut k, KEYD_F1, 0);
    event(&mut k, 0, 0, 210);
    check(&k, &[(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);

    let mut k = keyboard(&path);
    k.eval("f1 = timeout(overloadd(nav, semicolon, enter, 200), 100, noop)")
        .unwrap();
    tap(&mut k, KEYD_SEMICOLON, 0);
    tap(&mut k, KEYD_F1, 20);
    event(&mut k, 0, 0, 230);
    check(
        &k,
        &[
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
        ],
    );

    for (input, number) in [
        (KEYD_A, KEYD_1),
        (KEYD_S, KEYD_2),
        (KEYD_D, KEYD_3),
        (KEYD_F, KEYD_4),
        (KEYD_G, KEYD_5),
    ] {
        let mut k = keyboard(&path);
        event(&mut k, KEYD_SEMICOLON, 1, 0);
        tap(&mut k, input, 1);
        event(&mut k, KEYD_SEMICOLON, 0, 20);
        check(
            &k,
            &[
                (KEYD_LEFTALT, 1),
                (number, 1),
                (number, 0),
                (KEYD_LEFTALT, 0),
            ],
        );
    }

    let mut k = keyboard(&path);
    event(&mut k, KEYD_LEFTALT, 1, 0);
    for (i, number) in [
        KEYD_1, KEYD_2, KEYD_3, KEYD_4, KEYD_5, KEYD_6, KEYD_7, KEYD_8, KEYD_9, KEYD_0,
    ]
    .into_iter()
    .enumerate()
    {
        tap(&mut k, number, i as i32 * 20 + 1);
    }
    event(&mut k, KEYD_LEFTALT, 0, 210);
    let mut expected = vec![(KEYD_LEFTALT, 1)];
    for number in [
        KEYD_1, KEYD_2, KEYD_3, KEYD_4, KEYD_5, KEYD_6, KEYD_7, KEYD_8, KEYD_9, KEYD_0,
    ] {
        expected.extend([(number, 1), (number, 0)]);
    }
    expected.push((KEYD_LEFTALT, 0));
    check(&k, &expected);

    let mut k = keyboard(&path);
    k.eval("semicolon+a = esc").unwrap();
    tap(&mut k, KEYD_SEMICOLON, 0);
    tap(&mut k, KEYD_SEMICOLON, 30);
    event(&mut k, 0, 0, 240);
    check(&k, &[(KEYD_ENTER, 1), (KEYD_ENTER, 0)]);

    let mut k = keyboard(&path);
    k.eval("f1 = timeout(overloadd(nav, semicolon, enter, 200), 100, noop)")
        .unwrap();
    event(&mut k, KEYD_F1, 1, 0);
    event(&mut k, KEYD_H, 1, 10);
    event(&mut k, KEYD_H, 0, 20);
    event(&mut k, KEYD_F1, 0, 30);
    event(&mut k, 0, 0, 230);
    check(&k, &[(KEYD_LEFT, 1), (KEYD_LEFT, 0)]);

    let mut k = keyboard(&path);
    k.eval("b = overloadt(nav, b, 1000)").unwrap();
    event(&mut k, KEYD_B, 1, 0);
    tap(&mut k, KEYD_SEMICOLON, 10);
    assert_eq!(event(&mut k, 0, 0, 1000), 0);
    assert_eq!(k.output.0, [(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);
    event(&mut k, KEYD_B, 0, 1001);
    check(&k, &[(KEYD_SEMICOLON, 1), (KEYD_SEMICOLON, 0)]);

    let mut k = keyboard(&path);
    k.eval("b = overloadt(nav, b, 1000)").unwrap();
    event(&mut k, KEYD_B, 1, 0);
    tap(&mut k, KEYD_SEMICOLON, 10);
    tap(&mut k, KEYD_SEMICOLON, 30);
    assert_eq!(event(&mut k, 0, 0, 1000), 0);
    assert_eq!(k.output.0, [(KEYD_ENTER, 1), (KEYD_ENTER, 0)]);
    event(&mut k, KEYD_B, 0, 1001);
    check(&k, &[(KEYD_ENTER, 1), (KEYD_ENTER, 0)]);

    let mut k = keyboard(&path);
    k.eval("b = overloadt(nav, b, 1000)").unwrap();
    event(&mut k, KEYD_B, 1, 0);
    tap(&mut k, KEYD_SEMICOLON, 10);
    tap(&mut k, KEYD_SEMICOLON, 300);
    assert_eq!(event(&mut k, 0, 0, 1000), 0);
    event(&mut k, KEYD_B, 0, 1001);
    check(
        &k,
        &[
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
            (KEYD_SEMICOLON, 1),
            (KEYD_SEMICOLON, 0),
        ],
    );

    for expression in [
        "semicolon = overloadd(nav, semicolon, enter)",
        "semicolon = overloadd(nav, semicolon, enter, 0)",
        "semicolon = overloadd(nav, semicolon, enter, -1)",
        "semicolon = overloadd(nav, semicolon, enter, 65536)",
        "semicolon = overloadd(nav, command(true), enter, 200)",
        "semicolon = overloadd(nav, semicolon, layer(control), 200)",
    ] {
        assert!(keyboard(&path).eval(expression).is_err(), "{expression}");
    }
    let mut config = config_parse(&path).unwrap();
    config
        .descriptors
        .resize(keyd::config::MAX_DESCRIPTORS - 2, Default::default());
    assert!(
        config_add_entry(
            &mut config,
            "semicolon = overloadd(nav, semicolon, enter, 200)"
        )
        .is_err()
    );
    assert_eq!(config.descriptors.len(), keyd::config::MAX_DESCRIPTORS);
    println!(
        "double-tap E2E passed: punctuation deadlines, exact Enter, interruptions, immediate navigation, modifiers, RightAlt, boundary releases, repeated pairs, reset, and invalid actions"
    );
}
