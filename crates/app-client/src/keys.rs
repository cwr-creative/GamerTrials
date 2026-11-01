//! Host input that does not depend on which window has focus, read through the
//! player's bindings (ADR 0028).
//!
//! The harness reads keys from its minifb window's message queue, which only
//! sees them while that window is focused. Under the overlay (ADR 0027) the
//! focused window is usually the webview, so the game window would go deaf.
//! `host` reads the key's physical state instead, from any thread and whatever
//! has focus — `GetAsyncKeyState` on Windows, the X server's keymap on Linux
//! (ADR 0035); the one guard needed is that *one of our windows* is in front,
//! so a game does not keep playing from keystrokes typed into another
//! program. Mouse buttons are virtual keys too, and read the same way.
//!
//! A key is named by its Windows virtual-key code on every platform: that is
//! what a settings file stores, so one file binds the same keys on both.

use session_libretro::input::PadState;
use session_libretro::keymap::GUEST_KEYS;
use session_libretro::libretro;

use crate::settings::{Dir, Input, KeyboardGame, ResolvedInput, Stick};

pub use host::{down, tapped};

const VK_F12: i32 = 0x7B;

/// Whether one of `ours` is the foreground window.
pub fn ours_in_front(ours: &[isize]) -> bool {
    host::foreground().is_some_and(|fg| ours.contains(&fg))
}

#[cfg(windows)]
mod host {
    use winapi::um::winuser::{GetAsyncKeyState, GetForegroundWindow};

    pub fn down(vk: i32) -> bool {
        // High bit set: the key is down now.
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
    }

    /// Down now, or pressed and released since the last poll: for a key read
    /// as an edge once per frame, where a tap shorter than a frame would
    /// otherwise fall between two polls. The low bit is documented as
    /// unreliable across processes and is only ever an extra chance here,
    /// never the only one.
    pub fn tapped(vk: i32) -> bool {
        unsafe { (GetAsyncKeyState(vk) as u16 & 0x8001) != 0 }
    }

    pub fn foreground() -> Option<isize> {
        Some(unsafe { GetForegroundWindow() } as isize)
    }
}

#[cfg(target_os = "linux")]
mod host {
    use std::os::raw::c_ulong;

    use session_libretro::x11;

    const BUTTON1: u32 = 1 << 8;
    const BUTTON2: u32 = 1 << 9;
    const BUTTON3: u32 = 1 << 10;

    pub fn down(vk: i32) -> bool {
        match vk {
            0x01 => x11::pointer_buttons() & BUTTON1 != 0,
            0x02 => x11::pointer_buttons() & BUTTON3 != 0,
            0x04 => x11::pointer_buttons() & BUTTON2 != 0,
            // X11's pointer state has no bits for buttons 8 and 9, the side
            // buttons, so they cannot be bound here.
            0x05 | 0x06 => false,
            _ => keysym(vk).is_some_and(x11::keysym_down),
        }
    }

    /// The X server keeps no "pressed since" bit, so this is only [`down`]:
    /// a tap shorter than a frame can fall between two polls.
    pub fn tapped(vk: i32) -> bool {
        down(vk)
    }

    pub fn foreground() -> Option<isize> {
        x11::active_window()
    }

    /// The X11 keysym a virtual key types. Letters and digits are their own
    /// ASCII in both; the rest is a table.
    fn keysym(vk: i32) -> Option<c_ulong> {
        let sym = match vk {
            0x30..=0x39 => vk as c_ulong,
            0x41..=0x5A => (vk + 0x20) as c_ulong,
            // F1..F24.
            0x70..=0x87 => 0xFFBE + (vk - 0x70) as c_ulong,
            // The numpad's digits.
            0x60..=0x69 => 0xFFB0 + (vk - 0x60) as c_ulong,
            0x08 => 0xFF08,  // backspace
            0x09 => 0xFF09,  // tab
            0x0D => 0xFF0D,  // enter
            0x10 => 0xFFE1,  // shift
            0x11 => 0xFFE3,  // ctrl
            0x12 => 0xFFE9,  // alt
            0x13 => 0xFF13,  // pause
            0x14 => 0xFFE5,  // caps lock
            0x1B => 0xFF1B,  // escape
            0x20 => 0x0020,  // space
            0x21 => 0xFF55,  // page up
            0x22 => 0xFF56,  // page down
            0x23 => 0xFF57,  // end
            0x24 => 0xFF50,  // home
            0x25 => 0xFF51,  // left
            0x26 => 0xFF52,  // up
            0x27 => 0xFF53,  // right
            0x28 => 0xFF54,  // down
            0x2C => 0xFF61,  // print screen
            0x2D => 0xFF63,  // insert
            0x2E => 0xFFFF,  // delete
            0x5B => 0xFFEB,  // left super
            0x5C => 0xFFEC,  // right super
            0x5D => 0xFF67,  // menu
            0x6A => 0xFFAA,  // numpad *
            0x6B => 0xFFAB,  // numpad +
            0x6D => 0xFFAD,  // numpad -
            0x6E => 0xFFAE,  // numpad .
            0x6F => 0xFFAF,  // numpad /
            0x90 => 0xFF7F,  // num lock
            0x91 => 0xFF14,  // scroll lock
            0xA0 => 0xFFE1,  // left shift
            0xA1 => 0xFFE2,  // right shift
            0xA2 => 0xFFE3,  // left ctrl
            0xA3 => 0xFFE4,  // right ctrl
            0xA4 => 0xFFE9,  // left alt
            0xA5 => 0xFFEA,  // right alt
            0xBA => 0x003B,  // ;
            0xBB => 0x003D,  // =
            0xBC => 0x002C,  // ,
            0xBD => 0x002D,  // -
            0xBE => 0x002E,  // .
            0xBF => 0x002F,  // /
            0xC0 => 0x0060,  // `
            0xDB => 0x005B,  // [
            0xDC => 0x005C,  //             0xDD => 0x005D,  // ]
            0xDE => 0x0027,  // '
            _ => return None,
        };
        Some(sym)
    }
}

/// The quit key, read as a level like the harness does.
pub fn quit_pressed() -> bool {
    down(VK_F12)
}

/// Whether one bound input is held now.
pub fn held(input: &Input, pad: &PadState, deadzone: f32) -> bool {
    match input {
        Input::Key(vk) => down(*vk),
        Input::Mouse(b) => down(b.vk()),
        Input::Pad(b) => pad.buttons & (1 << b.index()) != 0,
        Input::Stick(s, d) => stick(pad, *s, *d) > deadzone,
    }
}

/// [`held`], with a key's tap since the last poll counted too: for the
/// client's own actions, read as edges.
pub fn held_or_tapped(input: &Input, pad: &PadState, deadzone: f32) -> bool {
    match input {
        Input::Key(vk) => tapped(*vk),
        Input::Mouse(b) => tapped(b.vk()),
        _ => held(input, pad, deadzone),
    }
}

/// How far a stick is pushed one way, 0..1.
pub fn stick(pad: &PadState, s: Stick, d: Dir) -> f32 {
    let (x, y) = match s {
        Stick::Left => (pad.sticks[0], pad.sticks[1]),
        Stick::Right => (pad.sticks[2], pad.sticks[3]),
    };
    match d {
        Dir::Up => y,
        Dir::Down => -y,
        Dir::Left => -x,
        Dir::Right => x,
    }
}

/// The RetroPad as the bindings say it is held: every bound input of a
/// button, OR'd, so a key and a pad button on the same button both work.
pub fn joypad(input: &ResolvedInput, pad: &PadState) -> u32 {
    let mut bits = 0u32;
    for (id, inputs) in input.binds.iter().enumerate() {
        if inputs.iter().any(|i| held(i, pad, input.deadzone)) {
            bits |= 1 << id;
        }
    }
    bits
}

/// Push this frame's keys into a keyboard game, as edges, the way
/// `session_libretro::input::keyboard_passthrough` does — each host key as
/// the player's remap says, plus whatever a gamepad's RetroPad buttons and
/// stick directions press. A guest key two sources hold is one press.
pub fn keyboard_game(kb: &KeyboardGame, pad_bits: u32, pad: &PadState, deadzone: f32) {
    let mut held_codes = Vec::new();
    for (vk, sent) in &kb.keys {
        if let Some(code) = sent {
            if down(*vk) {
                held_codes.push(*code);
            }
        }
    }
    for (id, code) in &kb.pad_keys {
        if pad_bits & (1 << id) != 0 {
            held_codes.push(*code);
        }
    }
    for (input, code) in &kb.stick_keys {
        if held(input, pad, deadzone) {
            held_codes.push(*code);
        }
    }
    for g in GUEST_KEYS {
        if held_codes.contains(&g.retrok) {
            libretro::key_down(g.retrok);
        } else {
            libretro::key_up(g.retrok);
        }
    }
}

/// Release every passthrough key, for when input is withheld from the game.
pub fn keyboard_release_all() {
    for g in GUEST_KEYS {
        libretro::key_up(g.retrok);
    }
}

/// The first input pressed now that was not held in `before`, for the
/// settings page's capture. Every virtual key is scanned, so a key outside
/// the guest table (F-keys, the numpad) can still bind a pad game's button.
pub fn first_new_press(pad: &PadState, deadzone: f32, before: &[Input]) -> Option<Input> {
    use crate::settings::{MouseButton, PadButton};
    let mut now = Vec::new();
    for b in [MouseButton::Left, MouseButton::Right, MouseButton::Middle, MouseButton::X1, MouseButton::X2] {
        now.push(Input::Mouse(b));
    }
    for vk in 0x08..=0xFE {
        // The generic Shift/Ctrl/Alt codes shadow their left and right ones.
        if (0x10..=0x12).contains(&vk) {
            continue;
        }
        now.push(Input::Key(vk));
    }
    for b in PadButton::ALL {
        now.push(Input::Pad(b));
    }
    for s in [Stick::Left, Stick::Right] {
        for d in [Dir::Up, Dir::Down, Dir::Left, Dir::Right] {
            now.push(Input::Stick(s, d));
        }
    }
    now.into_iter().find(|i| !before.contains(i) && held(i, pad, deadzone))
}

/// Everything held now, as [`first_new_press`] counts it.
pub fn all_held(pad: &PadState, deadzone: f32) -> Vec<Input> {
    let mut out = Vec::new();
    let mut before = Vec::new();
    while let Some(i) = first_new_press(pad, deadzone, &before) {
        before.push(i);
        out.push(i);
    }
    out
}
