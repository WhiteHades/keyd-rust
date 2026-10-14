// Rust port of keyd's keyboard.c. © 2019 Raheman Vaiya. See LICENSE.

use std::ffi::OsStr;
use std::sync::atomic::{AtomicI64, Ordering};

use crate::config::{
    Config, Descriptor, LayerType, MAX_DESCRIPTORS, MAX_LAYERS, Op, config_add_entry,
};
use crate::keys::*;
use crate::log;
use crate::macros::{MacroKind, macro_execute_while};

const CACHE_SIZE: usize = 16;
const CHORD_SLOTS: usize = (KEYD_CHORD_MAX - KEYD_CHORD_1 + 1) as usize;
static ACTIVATION_CLOCK: AtomicI64 = AtomicI64::new(1);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct KeyEvent {
    pub code: u8,
    pub pressed: u8,
    pub timestamp: i32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LayerState {
    pub activation_time: i64,
    pub active: u8,
    pub toggled: bool,
    pub oneshot_depth: u8,
}

pub trait Output {
    fn send_key(&mut self, code: u8, state: u8);
    fn failed(&self) -> bool {
        false
    }
    fn on_layer_change(
        &mut self,
        config: &Config,
        states: &[LayerState],
        layer: usize,
        active: bool,
    );
    fn execute_command(&mut self, command: &OsStr);
}

#[derive(Clone, Copy, Default)]
struct CacheEntry {
    code: u8,
    descriptor: Descriptor,
    descriptor_layer: usize,
    layer: usize,
    tap: Option<TapPress>,
}

#[derive(Clone, Copy)]
struct PendingTap {
    code: u8,
    descriptor: Descriptor,
    layer: usize,
    single: Descriptor,
    mods: u8,
    expiration: i64,
}

#[derive(Clone, Copy)]
struct TapPress {
    start: i64,
    interrupted: bool,
    single: Descriptor,
    double: Descriptor,
    window: i64,
    first: Option<PendingTap>,
}

#[derive(Clone, Copy, Default)]
struct ActiveChord {
    active: bool,
    keys: [u8; 8],
    len: usize,
    descriptor: Descriptor,
    layer: usize,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum ChordState {
    Resolving,
    #[default]
    Inactive,
    PendingDisambiguation,
    PendingHoldTimeout,
}

#[derive(Default)]
struct PendingChord {
    queue: [KeyEvent; 32],
    len: usize,
    matched: Option<(usize, usize)>,
    last_code_time: i64,
    state: ChordState,
}

#[derive(Clone, Copy, Default)]
struct PendingTimeout {
    code: u8,
    layer: usize,
    spontaneous: bool,
    expiration: i64,
    activation_time: i64,
    action1: Descriptor,
    action2: Descriptor,
}

#[derive(Default)]
struct PendingOverload {
    code: u8,
    layer: usize,
    expiration: i64,
    resolve_on_interrupt: bool,
    queue: [KeyEvent; 32],
    len: usize,
    action1: Descriptor,
    action2: Descriptor,
}

#[derive(Default)]
pub struct ScrollState {
    pub x: i32,
    pub y: i32,
    pub sensitivity: i32,
    pub active: bool,
}

pub struct Keyboard<O: Output> {
    original_config: Config,
    pub config: Config,
    pub output: O,
    cache: [CacheEntry; CACHE_SIZE],
    last_pressed_output_code: u8,
    last_pressed_code: u8,
    oneshot_latch: bool,
    inhibit_modifier_guard: bool,
    active_macro: Option<usize>,
    active_macro_layer: usize,
    macro_timeout: i64,
    oneshot_timeout: i64,
    macro_repeat_interval: i64,
    overload_start_time: i64,
    last_simple_key_time: i64,
    last_event_time: i64,
    timeouts: [i64; 128],
    nr_timeouts: usize,
    active_chords: [ActiveChord; CHORD_SLOTS],
    chord: PendingChord,
    pending_timeout: PendingTimeout,
    pending_overload: PendingOverload,
    pending_tap: Option<PendingTap>,
    pub layer_state: [LayerState; MAX_LAYERS],
    last_repeatable_action: Descriptor,
    pub keystate: [u8; 256],
    pub scroll: ScrollState,
}

fn send_key<O: Output>(
    output: &mut O,
    states: &mut [u8; 256],
    last: &mut u8,
    code: u8,
    pressed: u8,
) {
    if output.failed() || code == KEYD_NOOP || code == KEYD_EXTERNAL_MOUSE_BUTTON {
        return;
    }
    if pressed != 0 {
        *last = code;
    }
    if states[code as usize] != pressed {
        states[code as usize] = pressed;
        output.send_key(code, pressed);
    }
}

impl<O: Output> Keyboard<O> {
    pub fn new(config: Config, output: O) -> Self {
        assert!(!config.layers.is_empty() && config.layers.len() <= MAX_LAYERS);
        let mut layer_state = [LayerState::default(); MAX_LAYERS];
        layer_state[0].active = 1;
        if !config.default_layout.is_empty() {
            if let Some(idx) = config.layers.iter().position(|layer| {
                layer.kind == LayerType::Layout && layer.name == config.default_layout
            }) {
                layer_state[idx].active = 1;
                layer_state[idx].activation_time = 1;
            } else {
                let _ = log::stdout(
                    0,
                    "\tWARNING: could not find default layout %s.\n",
                    &[config.default_layout.as_encoded_bytes()],
                );
            }
        }
        Self {
            original_config: config.clone(),
            config,
            output,
            cache: [CacheEntry::default(); CACHE_SIZE],
            last_pressed_output_code: 0,
            last_pressed_code: 0,
            oneshot_latch: false,
            inhibit_modifier_guard: false,
            active_macro: None,
            active_macro_layer: 0,
            macro_timeout: 0,
            oneshot_timeout: 0,
            macro_repeat_interval: 0,
            overload_start_time: 0,
            last_simple_key_time: 0,
            last_event_time: 0,
            timeouts: [0; 128],
            nr_timeouts: 0,
            active_chords: [ActiveChord::default(); CHORD_SLOTS],
            chord: PendingChord::default(),
            pending_timeout: PendingTimeout::default(),
            pending_overload: PendingOverload::default(),
            pending_tap: None,
            layer_state,
            last_repeatable_action: Descriptor::default(),
            keystate: [0; 256],
            scroll: ScrollState::default(),
        }
    }

    fn cache_index(&self, code: u8) -> Option<usize> {
        self.cache.iter().position(|entry| entry.code == code)
    }

    fn cache_set(&mut self, code: u8, entry: CacheEntry) -> bool {
        let mut slot = None;
        for (i, cached) in self.cache.iter().enumerate() {
            if cached.code == code {
                slot = Some(i);
                break;
            } else if cached.code == 0 {
                slot = Some(i);
            }
        }
        if let Some(slot) = slot {
            self.cache[slot] = CacheEntry { code, ..entry };
            true
        } else {
            false
        }
    }

    fn send_key(&mut self, code: u8, pressed: u8) {
        send_key(
            &mut self.output,
            &mut self.keystate,
            &mut self.last_pressed_output_code,
            code,
            pressed,
        );
    }

    fn reset_keystate(&mut self) {
        for code in 0..256 {
            if self.output.failed() {
                return;
            }
            if self.keystate[code] != 0 {
                self.output.send_key(code as u8, 0);
                self.keystate[code] = 0;
            }
        }
    }

    fn clear_mod(&mut self, code: u8) {
        let guard = self.last_pressed_output_code == code
            && matches!(code, KEYD_LEFTMETA | KEYD_LEFTALT | KEYD_RIGHTALT)
            && !self.inhibit_modifier_guard
            && !self.config.disable_modifier_guard;
        if guard && self.keystate[KEYD_LEFTCTRL as usize] == 0 {
            self.send_key(KEYD_LEFTCTRL, 1);
            self.send_key(code, 0);
            self.send_key(KEYD_LEFTCTRL, 0);
        } else {
            self.send_key(code, 0);
        }
    }

    fn active_mods(&self, excluded: Option<usize>, mut mods: u8) -> u8 {
        for (i, layer) in self.config.layers.iter().enumerate() {
            if self.layer_state[i].active == 0 {
                continue;
            }
            let excluded = excluded.is_some_and(|index| {
                index == i
                    || (self.config.layers[index].kind == LayerType::Composite
                        && self.config.layers[index].constituents.contains(&i))
            });
            if !excluded {
                mods |= layer.mods;
            }
        }
        mods
    }

    fn set_mods(&mut self, mods: u8) {
        for modifier in MODIFIERS {
            if mods & modifier.mask != 0 {
                if self.keystate[modifier.key as usize] == 0 {
                    self.send_key(modifier.key, 1);
                }
            } else if self.keystate[modifier.key as usize] != 0 {
                self.clear_mod(modifier.key);
            }
        }
    }

    fn update_mods(&mut self, excluded: Option<usize>, mods: u8) -> u8 {
        let mods = self.active_mods(excluded, mods);
        self.set_mods(mods);
        mods
    }

    fn emit_tap(&mut self, action: Descriptor, mods: u8, time: i64) {
        let code = action.args[0] as u8;
        let mods = mods | action.args[1] as u8;
        self.set_mods(mods);
        self.send_key(code, 0);
        self.send_key(code, 1);
        self.send_key(code, 0);
        self.last_repeatable_action = action;
        self.last_repeatable_action.args[1] = mods as u16;
        self.clear_oneshot();
        self.update_mods(None, 0);
        if mods == 0 || mods == MOD_SHIFT {
            self.last_simple_key_time = time;
        }
    }

    fn resolve_taps(&mut self, code: u8, pressed: bool, time: i64, flush: bool) {
        if self
            .pending_tap
            .is_some_and(|tap| flush || time >= tap.expiration || (code != 0 && code != tap.code))
        {
            let tap = self.pending_tap.take().unwrap();
            self.emit_tap(tap.single, tap.mods, time);
        }
        for slot in 0..self.cache.len() {
            let entry = &mut self.cache[slot];
            if let Some(tap) = &mut entry.tap {
                if code != 0 && code != entry.code && pressed {
                    tap.interrupted = true;
                }
                if tap.first.is_some_and(|first| {
                    flush || time >= first.expiration || (code != 0 && code != entry.code)
                }) {
                    let first = tap.first.take().unwrap();
                    self.emit_tap(first.single, first.mods, time);
                }
            }
        }
    }

    fn finish_tap(&mut self, code: u8, entry: CacheEntry, time: i64) {
        let tap = entry.tap.unwrap();
        self.deactivate_layer(entry.layer);
        self.update_mods(None, 0);
        let mods = self.active_mods(Some(entry.descriptor_layer), 0);
        let valid = !tap.interrupted
            && (self.config.overload_tap_timeout == 0
                || time - tap.start < self.config.overload_tap_timeout);
        if let Some(first) = tap.first {
            if valid && time - tap.start < tap.window {
                self.emit_tap(tap.double, mods, time);
            } else {
                self.emit_tap(first.single, first.mods, time);
            }
        } else if valid {
            if time - tap.start < tap.window {
                self.pending_tap = Some(PendingTap {
                    code,
                    descriptor: entry.descriptor,
                    layer: entry.descriptor_layer,
                    single: tap.single,
                    mods,
                    expiration: time + tap.window,
                });
            } else {
                self.emit_tap(tap.single, mods, time);
            }
        }
    }

    fn execute_macro(&mut self, layer: usize, index: usize) -> i64 {
        if self.output.failed() {
            return 0;
        }
        let time;
        let single = self.config.macros.get(index).and_then(|macro_| {
            (macro_.entries.len() == 1 && macro_.entries[0].kind == MacroKind::KeySequence)
                .then(|| macro_.entries[0].data)
        });
        if let Some(data) = single {
            self.update_mods(Some(layer), (data >> 8) as u8);
            self.send_key(data as u8, 1);
            self.send_key(data as u8, 0);
            time = 0;
        } else {
            self.update_mods(Some(layer), 0);
            if self.output.failed() {
                return 0;
            }
            let output = &mut self.output;
            let states = &mut self.keystate;
            let last = &mut self.last_pressed_output_code;
            // Reset zeroes unused C slots even when a pending action still refers to one.
            time = self.config.macros.get(index).map_or(0, |macro_| {
                macro_execute_while(
                    macro_,
                    self.config.macro_sequence_timeout as u64,
                    |code, pressed| {
                        send_key(output, states, last, code, pressed);
                        !output.failed()
                    },
                )
            });
        }
        self.update_mods(None, 0);
        time
    }

    fn auxiliary_descriptor(&self, index: u16) -> Descriptor {
        assert!((index as usize) < MAX_DESCRIPTORS);
        self.config
            .descriptors
            .get(index as usize)
            .copied()
            .unwrap_or_default()
    }

    fn lookup_descriptor(&self, code: u8) -> (Descriptor, usize) {
        if (KEYD_CHORD_1..=KEYD_CHORD_MAX).contains(&code) {
            let chord = self.active_chords[(code - KEYD_CHORD_1) as usize];
            return (chord.descriptor, chord.layer);
        }
        let mut result = (Descriptor::default(), 0);
        let mut activation = 0;
        for (i, layer) in self.config.layers.iter().enumerate() {
            let state = self.layer_state[i];
            if state.active != 0
                && layer.keymap[code as usize].op != Op::None
                && state.activation_time >= activation
            {
                activation = state.activation_time;
                result = (layer.keymap[code as usize], i);
            }
        }
        let mut max = 0;
        for (i, layer) in self.config.layers.iter().enumerate() {
            if layer.kind == LayerType::Composite
                && layer.constituents.len() > max
                && layer.keymap[code as usize].op != Op::None
                && layer
                    .constituents
                    .iter()
                    .all(|&idx| self.layer_state[idx].active != 0)
            {
                result = (layer.keymap[code as usize], i);
                max = layer.constituents.len();
            }
        }
        if result.0.op == Op::None {
            result = (
                Descriptor {
                    op: Op::KeySequence,
                    args: [code as u16, 0, 0],
                },
                0,
            );
        }
        result
    }

    fn notify_layer(&mut self, index: usize, active: bool) {
        if self.output.failed() {
            return;
        }
        self.output.on_layer_change(
            &self.config,
            &self.layer_state[..self.config.layers.len()],
            index,
            active,
        );
    }

    fn deactivate_layer(&mut self, index: usize) {
        assert!(self.layer_state[index].active > 0);
        let _ = log::debug_stdout(
            1,
            "src/keyboard.rs",
            line!(),
            "Deactivating layer %s",
            &[self.config.layers[index].name.as_encoded_bytes()],
        );
        self.layer_state[index].active -= 1;
        self.notify_layer(index, false);
    }

    fn activate_layer(&mut self, code: u8, index: usize) {
        let _ = log::debug_stdout(
            1,
            "src/keyboard.rs",
            line!(),
            "Activating layer %s",
            &[self.config.layers[index].name.as_encoded_bytes()],
        );
        self.layer_state[index].activation_time = ACTIVATION_CLOCK.fetch_add(1, Ordering::Relaxed);
        self.layer_state[index].active = self.layer_state[index].active.wrapping_add(1);
        if let Some(slot) = self.cache_index(code) {
            self.cache[slot].layer = index;
        }
        self.notify_layer(index, true);
    }

    fn clear_oneshot(&mut self) {
        for i in 0..self.config.layers.len() {
            while self.layer_state[i].oneshot_depth != 0 {
                self.deactivate_layer(i);
                self.layer_state[i].oneshot_depth -= 1;
            }
        }
        self.oneshot_latch = false;
        self.oneshot_timeout = 0;
    }

    fn clear(&mut self) {
        self.clear_oneshot();
        for i in 1..self.config.layers.len() {
            if self.config.layers[i].kind != LayerType::Layout && self.layer_state[i].toggled {
                self.layer_state[i].toggled = false;
                self.deactivate_layer(i);
            }
        }
        self.active_macro = None;
        self.reset_keystate();
    }

    fn set_layout(&mut self, index: usize) {
        self.clear();
        for i in 1..self.config.layers.len() {
            if self.config.layers[i].kind == LayerType::Layout {
                self.layer_state[i].active = 0;
            }
        }
        if index != 0 {
            self.layer_state[index].activation_time = 1;
            self.layer_state[index].active = 1;
        }
        self.notify_layer(index, true);
    }

    fn schedule_timeout(&mut self, timeout: i64) {
        assert!(self.nr_timeouts < self.timeouts.len());
        self.timeouts[self.nr_timeouts] = timeout;
        self.nr_timeouts += 1;
    }

    fn main_loop_timeout(&mut self, time: i64) -> i64 {
        if self.output.failed() {
            return 0;
        }
        let mut timeout = 0;
        let mut n = 0;
        for i in 0..self.nr_timeouts {
            let deadline = self.timeouts[i];
            if deadline > time {
                if timeout == 0 || deadline < timeout {
                    timeout = deadline;
                }
                self.timeouts[n] = deadline;
                n += 1;
            }
        }
        self.nr_timeouts = n;
        for deadline in
            self.pending_tap
                .iter()
                .map(|tap| tap.expiration)
                .chain(self.cache.iter().filter_map(|entry| {
                    entry
                        .tap
                        .and_then(|tap| tap.first)
                        .map(|tap| tap.expiration)
                }))
        {
            if deadline > time && (timeout == 0 || deadline < timeout) {
                timeout = deadline;
            }
        }
        if timeout == 0 { 0 } else { timeout - time }
    }

    fn process_descriptor(
        &mut self,
        code: u8,
        descriptor: Descriptor,
        layer: usize,
        pressed: bool,
        time: i64,
    ) {
        if self.output.failed() {
            return;
        }
        let d = descriptor;
        let index = d.args[0] as usize;
        if pressed && matches!(d.op, Op::LayerM | Op::OneShotM | Op::ToggleM) {
            self.execute_macro(layer, d.args[1] as usize);
            if self.output.failed() {
                return;
            }
        }
        match d.op {
            Op::None => {}
            Op::KeySequence => {
                let new_code = d.args[0] as u8;
                let mods = d.args[1] as u8;
                if pressed {
                    if self.keystate[new_code as usize] != 0 {
                        self.send_key(new_code, 0);
                    }
                    let active_mods = self.update_mods(Some(layer), mods);
                    self.last_repeatable_action = d;
                    self.last_repeatable_action.args[1] = active_mods as u16;
                    self.send_key(new_code, 1);
                    self.clear_oneshot();
                } else {
                    self.send_key(new_code, 0);
                    self.update_mods(None, 0);
                }
                if mods == 0 || mods == MOD_SHIFT {
                    self.last_simple_key_time = time;
                }
            }
            Op::Scroll => {
                self.scroll.sensitivity = d.args[0] as i16 as i32;
                self.scroll.active = pressed;
            }
            Op::ScrollToggleOn => {
                self.scroll.sensitivity = d.args[0] as i16 as i32;
                self.scroll.active = true;
            }
            Op::ScrollToggleOff => {
                if pressed {
                    self.scroll.active = false;
                }
            }
            Op::ScrollToggle => {
                self.scroll.sensitivity = d.args[0] as i16 as i32;
                if pressed {
                    self.scroll.active = !self.scroll.active;
                }
            }
            Op::OverloadIdleTimeout => {
                if pressed {
                    let action = self.auxiliary_descriptor(
                        d.args[if time - self.last_simple_key_time >= d.args[2] as i64 {
                            1
                        } else {
                            0
                        }],
                    );
                    self.process_descriptor(code, action, layer, true, time);
                    if let Some(slot) = self.cache_index(code) {
                        self.cache[slot].descriptor = action;
                    }
                }
            }
            Op::OverloadTimeout | Op::OverloadTimeoutTap => {
                if pressed {
                    self.pending_overload.code = code;
                    self.pending_overload.resolve_on_interrupt = d.op == Op::OverloadTimeoutTap;
                    self.pending_overload.layer = layer;
                    self.pending_overload.action1 = self.auxiliary_descriptor(d.args[1]);
                    self.pending_overload.action2 = Descriptor {
                        op: Op::Layer,
                        args: [d.args[0], 0, 0],
                    };
                    self.pending_overload.expiration = time + d.args[2] as i64;
                    self.schedule_timeout(self.pending_overload.expiration);
                }
            }
            Op::Layout => {
                if pressed {
                    self.set_layout(index);
                }
            }
            Op::LayerM | Op::Layer => {
                if pressed {
                    self.activate_layer(code, index);
                } else {
                    self.deactivate_layer(index);
                }
                self.inhibit_modifier_guard = self.last_pressed_code == code;
                self.update_mods(None, 0);
                self.inhibit_modifier_guard = false;
            }
            Op::ClearM => {
                if pressed {
                    self.clear();
                    self.execute_macro(layer, index);
                }
            }
            Op::Repeat => {
                if pressed {
                    self.process_descriptor(code, self.last_repeatable_action, layer, true, time);
                    if let Some(slot) = self.cache_index(code) {
                        self.cache[slot].descriptor = self.last_repeatable_action;
                    }
                }
            }
            Op::Clear => {
                if pressed {
                    self.clear();
                }
            }
            Op::Overload => {
                let action = self.auxiliary_descriptor(d.args[1]);
                if pressed {
                    self.overload_start_time = time;
                    self.activate_layer(code, index);
                    self.update_mods(None, 0);
                } else {
                    self.deactivate_layer(index);
                    self.update_mods(None, 0);
                    if self.last_pressed_code == code
                        && (self.config.overload_tap_timeout == 0
                            || time - self.overload_start_time < self.config.overload_tap_timeout)
                    {
                        if action.op == Op::Macro {
                            self.execute_macro(layer, action.args[0] as usize);
                        } else {
                            self.process_descriptor(code, action, layer, true, time);
                            self.process_descriptor(code, action, layer, false, time);
                        }
                    }
                }
            }
            Op::OverloadDoubleTap => {
                if pressed {
                    if self.cache_index(code).is_none()
                        && !self.cache_set(
                            code,
                            CacheEntry {
                                descriptor: d,
                                descriptor_layer: layer,
                                ..CacheEntry::default()
                            },
                        )
                    {
                        return;
                    }
                    let pair = self.auxiliary_descriptor(d.args[1]);
                    let first = self.pending_tap.take();
                    let first = first.filter(|tap| {
                        if tap.descriptor == d && tap.layer == layer {
                            true
                        } else {
                            self.emit_tap(tap.single, tap.mods, time);
                            false
                        }
                    });
                    if let Some(slot) = self.cache_index(code) {
                        self.cache[slot].tap = Some(TapPress {
                            start: time,
                            interrupted: false,
                            single: self.auxiliary_descriptor(pair.args[0]),
                            double: self.auxiliary_descriptor(pair.args[2]),
                            window: pair.args[1] as i64,
                            first,
                        });
                    }
                    self.activate_layer(code, index);
                    self.update_mods(None, 0);
                } else if let Some(slot) = self.cache_index(code) {
                    let entry = self.cache[slot];
                    self.cache[slot] = CacheEntry::default();
                    if entry.tap.is_some() {
                        self.finish_tap(code, entry, time);
                    }
                }
            }
            Op::OneShotM | Op::OneShotK | Op::OneShot => {
                if pressed {
                    if d.op == Op::OneShotK {
                        self.process_descriptor(
                            code,
                            self.auxiliary_descriptor(d.args[1]),
                            layer,
                            true,
                            time,
                        );
                    }
                    self.activate_layer(code, index);
                    self.update_mods(Some(layer), 0);
                    self.oneshot_latch = true;
                } else {
                    if d.op == Op::OneShotK {
                        self.process_descriptor(
                            code,
                            self.auxiliary_descriptor(d.args[1]),
                            layer,
                            false,
                            time,
                        );
                    }
                    if self.oneshot_latch {
                        self.layer_state[index].oneshot_depth =
                            self.layer_state[index].oneshot_depth.wrapping_add(1);
                        if self.config.oneshot_timeout != 0 {
                            self.oneshot_timeout = time + self.config.oneshot_timeout;
                            self.schedule_timeout(self.oneshot_timeout);
                        }
                    } else {
                        self.deactivate_layer(index);
                        self.update_mods(None, 0);
                    }
                }
            }
            Op::Macro2 | Op::Macro => {
                if pressed {
                    let (macro_index, timeout) = if d.op == Op::Macro2 {
                        self.macro_repeat_interval = d.args[1] as i64;
                        (d.args[2] as usize, d.args[0] as i64)
                    } else {
                        self.macro_repeat_interval = self.config.macro_repeat_timeout;
                        (index, self.config.macro_timeout)
                    };
                    self.clear_oneshot();
                    let execution = self.execute_macro(layer, macro_index);
                    if self.output.failed() {
                        return;
                    }
                    self.active_macro = Some(macro_index);
                    self.active_macro_layer = layer;
                    self.macro_timeout = execution + time + timeout;
                    self.schedule_timeout(self.macro_timeout);
                    self.last_repeatable_action = d;
                }
            }
            Op::ToggleM | Op::Toggle => {
                if pressed {
                    self.layer_state[index].toggled = !self.layer_state[index].toggled;
                    if self.layer_state[index].toggled {
                        self.activate_layer(code, index);
                    } else {
                        self.deactivate_layer(index);
                    }
                    self.update_mods(None, 0);
                    self.clear_oneshot();
                }
            }
            Op::Timeout => {
                if pressed {
                    self.pending_timeout = PendingTimeout {
                        code,
                        layer,
                        action1: self.auxiliary_descriptor(d.args[0]),
                        action2: self.auxiliary_descriptor(d.args[2]),
                        expiration: time + d.args[1] as i64,
                        activation_time: time,
                        spontaneous: false,
                    };
                    self.schedule_timeout(self.pending_timeout.expiration);
                } else if time == self.pending_timeout.activation_time {
                    self.pending_timeout.spontaneous = true;
                }
            }
            Op::Command => {
                if pressed {
                    let command = self
                        .config
                        .commands
                        .get(index)
                        .map_or(OsStr::new(""), |command| command.as_os_str());
                    let _ = log::debug_stdout(
                        1,
                        "src/keyboard.rs",
                        line!(),
                        "executing command: %s",
                        &[command.as_encoded_bytes()],
                    );
                    self.output.execute_command(command);
                    self.clear_oneshot();
                    self.update_mods(None, 0);
                }
            }
            Op::Swap | Op::SwapM => {
                let macro_index = (d.op == Op::SwapM).then_some(d.args[1] as usize);
                if pressed {
                    if self.layer_state[layer].toggled {
                        self.deactivate_layer(layer);
                        self.layer_state[layer].toggled = false;
                        self.activate_layer(0, index);
                        self.layer_state[index].toggled = true;
                        self.update_mods(None, 0);
                    } else if self.layer_state[layer].oneshot_depth != 0 {
                        self.deactivate_layer(layer);
                        self.layer_state[layer].oneshot_depth -= 1;
                        self.activate_layer(0, index);
                        self.layer_state[index].oneshot_depth =
                            self.layer_state[index].oneshot_depth.wrapping_add(1);
                        self.update_mods(None, 0);
                    } else if let Some(slot) = self.cache.iter().position(|entry| {
                        entry.code != 0
                            && entry.layer == layer
                            && layer != 0
                            && self.config.layers[entry.layer].kind == LayerType::Normal
                    }) {
                        self.cache[slot].descriptor.op = Op::Layer;
                        self.cache[slot].descriptor.args[0] = d.args[0];
                        self.deactivate_layer(layer);
                        self.activate_layer(self.cache[slot].code, index);
                        self.update_mods(None, 0);
                    }
                    if let Some(index) = macro_index {
                        self.execute_macro(layer, index);
                    }
                } else if let Some(index) = macro_index
                    && let Some(macro_) = self.config.macros.get(index)
                {
                    let entries = &macro_.entries;
                    if entries.len() == 1 && entries[0].kind == MacroKind::KeySequence {
                        self.send_key(entries[0].data as u8, 0);
                        self.update_mods(None, 0);
                    }
                }
            }
        }
        if pressed {
            self.last_pressed_code = code;
        }
    }

    fn enqueue_chord(&mut self, code: u8, pressed: bool, time: i64) {
        if code != 0 {
            assert!(self.chord.len < self.chord.queue.len());
            self.chord.queue[self.chord.len] = KeyEvent {
                code,
                pressed: pressed as u8,
                timestamp: time as i32,
            };
            self.chord.len += 1;
        }
    }

    fn check_chord_match(&mut self) -> u8 {
        let mut full = false;
        let mut partial = false;
        let mut max_time = -1;
        for (layer_index, layer) in self.config.layers.iter().enumerate() {
            if self.layer_state[layer_index].active == 0 {
                continue;
            }
            for (index, chord) in layer.chords.iter().enumerate() {
                let mut n = 0;
                let mut valid = true;
                for event in &self.chord.queue[..self.chord.len] {
                    if event.pressed != 0 {
                        n += 1;
                        if !chord.keys.contains(&event.code) {
                            valid = false;
                            break;
                        }
                    }
                }
                if !valid || n == 0 {
                    continue;
                }
                if n == chord.keys.len() {
                    if max_time <= self.layer_state[layer_index].activation_time {
                        self.chord.matched = Some((layer_index, index));
                        full = true;
                        max_time = self.layer_state[layer_index].activation_time;
                    }
                } else {
                    partial = true;
                }
            }
        }
        if full {
            if partial { 3 } else { 2 }
        } else if partial {
            1
        } else {
            0
        }
    }

    fn resolve_chord(&mut self) -> bool {
        self.chord.state = ChordState::Resolving;
        let mut offset = 0;
        if let Some((layer, index)) = self.chord.matched {
            let slot = self
                .active_chords
                .iter()
                .position(|chord| !chord.active)
                .expect("active chord limit exceeded");
            let mut active = ActiveChord {
                active: true,
                layer,
                ..ActiveChord::default()
            };
            // Reset leaves the C match pointer addressing an empty, fixed chord slot.
            if let Some(chord) = self.config.layers[layer].chords.get(index) {
                active.len = chord.keys.len();
                active.descriptor = chord.descriptor;
                active.keys[..active.len].copy_from_slice(&chord.keys);
            }
            offset = active.len;
            self.active_chords[slot] = active;
            self.process_event(KEYD_CHORD_1 + slot as u8, true, self.chord.last_code_time);
        }
        let queue = self.chord.queue;
        let len = self.chord.len;
        self.process_events(&queue[offset..len]);
        self.chord.state = ChordState::Inactive;
        true
    }

    fn abort_chord(&mut self) -> bool {
        self.chord.matched = None;
        self.resolve_chord()
    }

    fn await_chord_match(&mut self, time: i64) -> bool {
        match self.check_chord_match() {
            0 => self.abort_chord(),
            1 | 3 => {
                self.chord.state = ChordState::PendingDisambiguation;
                self.chord.last_code_time = time;
                self.schedule_timeout(time + self.config.chord_interkey_timeout);
                true
            }
            _ => {
                self.chord.last_code_time = time;
                if self.config.chord_hold_timeout != 0 {
                    self.chord.state = ChordState::PendingHoldTimeout;
                    self.schedule_timeout(time + self.config.chord_hold_timeout);
                    true
                } else {
                    self.resolve_chord()
                }
            }
        }
    }

    fn handle_chord(&mut self, code: u8, pressed: bool, time: i64) -> bool {
        if code != 0 && !pressed {
            for slot in 0..self.active_chords.len() {
                let chord = &mut self.active_chords[slot];
                if !chord.active {
                    continue;
                }
                let mut found = false;
                for key in &mut chord.keys[..chord.len] {
                    if *key == code {
                        *key = 0;
                        found = true;
                    }
                }
                if found {
                    if chord.keys[..chord.len].iter().all(|&key| key == 0) {
                        chord.active = false;
                        self.process_event(KEYD_CHORD_1 + slot as u8, false, time);
                    }
                    return true;
                }
            }
        }
        match self.chord.state {
            ChordState::Resolving => false,
            ChordState::Inactive => {
                self.chord.len = 0;
                self.chord.matched = None;
                self.enqueue_chord(code, pressed, time);
                if self.check_chord_match() == 0 {
                    false
                } else {
                    self.await_chord_match(time)
                }
            }
            ChordState::PendingDisambiguation => {
                if code == 0 {
                    if time - self.chord.last_code_time >= self.config.chord_interkey_timeout {
                        if self.chord.matched.is_none() {
                            return self.abort_chord();
                        }
                        let left =
                            self.config.chord_hold_timeout - self.config.chord_interkey_timeout;
                        if left <= 0 {
                            return self.resolve_chord();
                        }
                        self.schedule_timeout(time + left);
                        self.chord.state = ChordState::PendingHoldTimeout;
                        true
                    } else {
                        false
                    }
                } else {
                    self.enqueue_chord(code, pressed, time);
                    if !pressed {
                        self.abort_chord()
                    } else {
                        self.await_chord_match(time)
                    }
                }
            }
            ChordState::PendingHoldTimeout => {
                if code == 0 {
                    if time - self.chord.last_code_time >= self.config.chord_hold_timeout {
                        self.resolve_chord()
                    } else {
                        false
                    }
                } else {
                    self.enqueue_chord(code, pressed, time);
                    if !pressed
                        && self.chord.matched.is_some_and(|(layer, index)| {
                            self.config.layers[layer]
                                .chords
                                .get(index)
                                .is_some_and(|chord| chord.keys.contains(&code))
                        })
                    {
                        self.abort_chord()
                    } else {
                        true
                    }
                }
            }
        }
    }

    fn handle_pending_timeout(&mut self, code: u8, pressed: bool, time: i64) {
        let pending = self.pending_timeout;
        if pending.code == 0
            || (!pressed && pending.code == code && time == pending.activation_time)
        {
            return;
        }
        if pending.spontaneous {
            if time >= pending.expiration || code != 0 {
                let action = if time >= pending.expiration {
                    pending.action2
                } else {
                    pending.action1
                };
                self.pending_timeout.code = 0;
                self.process_descriptor(pending.code, action, pending.layer, true, time);
                self.process_descriptor(pending.code, action, pending.layer, false, time);
            }
        } else if time >= pending.expiration || (code != 0 && (pressed || code == pending.code)) {
            let action = if time >= pending.expiration {
                pending.action2
            } else {
                pending.action1
            };
            self.pending_timeout.code = 0;
            self.cache_set(
                pending.code,
                CacheEntry {
                    descriptor_layer: pending.layer,
                    descriptor: action,
                    ..CacheEntry::default()
                },
            );
            self.process_descriptor(pending.code, action, pending.layer, true, time);
        }
    }

    fn handle_pending_overload(&mut self, code: u8, pressed: bool, time: i64) -> bool {
        let pending = &mut self.pending_overload;
        if pending.code == 0 {
            return false;
        }
        if code != 0 {
            assert!(pending.len < pending.queue.len());
            if !pressed
                && code != pending.code
                && !pending.queue[..pending.len]
                    .iter()
                    .any(|event| event.code == code)
            {
                return false;
            }
            pending.queue[pending.len] = KeyEvent {
                code,
                pressed: pressed as u8,
                timestamp: time as i32,
            };
            pending.len += 1;
        }
        let action = if time >= pending.expiration {
            Some(pending.action2)
        } else if code == pending.code {
            Some(pending.action1)
        } else if pending.resolve_on_interrupt && !pressed {
            Some(pending.action2)
        } else {
            None
        };
        if let Some(action) = action.filter(|action| action.op != Op::None) {
            let queue = pending.queue;
            let len = pending.len;
            let code = pending.code;
            let layer = pending.layer;
            pending.code = 0;
            pending.len = 0;
            self.cache_set(
                code,
                CacheEntry {
                    descriptor: action,
                    descriptor_layer: layer,
                    ..CacheEntry::default()
                },
            );
            self.process_descriptor(code, action, layer, true, time);
            self.process_events(&queue[..len]);
        }
        true
    }

    fn process_event(&mut self, code: u8, pressed: bool, time: i64) -> i64 {
        if self.output.failed() {
            return 0;
        }
        self.last_event_time = time;
        self.resolve_taps(code, pressed, time, false);
        if self.handle_chord(code, pressed, time) {
            self.last_event_time = time;
            self.resolve_taps(0, false, time, false);
            return self.main_loop_timeout(time);
        }
        self.handle_pending_timeout(code, pressed, time);
        self.resolve_taps(code, pressed, time, false);
        if self.handle_pending_overload(code, pressed, time) {
            self.last_event_time = time;
            self.resolve_taps(0, false, time, false);
            return self.main_loop_timeout(time);
        }
        if self.oneshot_timeout != 0 && time >= self.oneshot_timeout {
            self.clear_oneshot();
            self.update_mods(None, 0);
        }
        if let Some(index) = self.active_macro {
            if code != 0 {
                self.active_macro = None;
                self.update_mods(None, 0);
            } else if time >= self.macro_timeout {
                let execution = self.execute_macro(self.active_macro_layer, index);
                if self.output.failed() {
                    return 0;
                }
                self.macro_timeout = execution + time + self.macro_repeat_interval;
                self.schedule_timeout(self.macro_timeout);
            }
        }
        if code != 0 {
            let (descriptor, layer);
            if pressed {
                if self.cache_index(code).is_some() {
                    return self.main_loop_timeout(time);
                }
                (descriptor, layer) = self.lookup_descriptor(code);
                if !self.cache_set(
                    code,
                    CacheEntry {
                        descriptor,
                        descriptor_layer: layer,
                        ..CacheEntry::default()
                    },
                ) {
                    return self.main_loop_timeout(time);
                }
            } else {
                let Some(slot) = self.cache_index(code) else {
                    return self.main_loop_timeout(time);
                };
                let entry = self.cache[slot];
                self.cache[slot] = CacheEntry::default();
                if entry.tap.is_some() {
                    self.finish_tap(code, entry, time);
                    return self.main_loop_timeout(time);
                }
                (descriptor, layer) = (entry.descriptor, entry.descriptor_layer);
            }
            self.process_descriptor(code, descriptor, layer, pressed, time);
        }
        self.main_loop_timeout(time)
    }

    pub fn process_events(&mut self, events: &[KeyEvent]) -> i64 {
        let mut i = 0;
        let mut timeout = 0i32;
        let mut timeout_ts = 0i32;
        while i != events.len() {
            if self.output.failed() {
                return 0;
            }
            let event = events[i];
            if timeout > 0 && timeout_ts <= event.timestamp {
                timeout = self.process_event(0, false, timeout_ts as i64) as i32;
                timeout_ts = timeout_ts.wrapping_add(timeout);
            } else {
                timeout = self.process_event(event.code, event.pressed != 0, event.timestamp as i64)
                    as i32;
                timeout_ts = event.timestamp.wrapping_add(timeout);
                i += 1;
            }
        }
        timeout as i64
    }

    pub fn eval(&mut self, expression: impl AsRef<OsStr>) -> Result<(), crate::config::ParseError> {
        let expression = expression.as_ref();
        self.resolve_taps(0, false, self.last_event_time, true);
        if expression.as_encoded_bytes() == b"reset" {
            self.config.clone_from(&self.original_config);
            Ok(())
        } else {
            config_add_entry(&mut self.config, expression)
        }
    }
}
