//! Host keyboard and gamepad, turned into whatever port 0 is driven by.
//!
//! Port 0 only; one local player.
//!
//! Two shapes, and a manifest picks which (`[runner.libretro] controller`):
//!
//! * **A pad.** Keyboard and gamepad collapse into one libretro joypad
//!   bitmask, and the manifest's [`PadLayout`] decides how a modern
//!   controller's face diamond lands on the console's.
//! * **A keyboard.** Every key passes straight through as its `RETROK_*` code
//!   ([`keyboard_passthrough`]). There is nothing to lay out, because a DOS
//!   game maps its own keys — Doom's controls are configured inside Doom, and a
//!   manifest that remapped them would be overriding a choice the player has
//!   already made.
//!
//! The libretro joypad layout *is* the SNES pad, so all twelve ids are always
//! mapped and a NES core simply ignores the four it does not have. The one
//! thing a manifest picks is how a modern controller's face diamond lands on
//! it — see [`PadLayout`] — because that is a genuine difference between the
//! two consoles rather than a preference: a two-button game wants both the "A
//! is on the right" and "A is on the bottom" habits to work, and a four-button
//! game cannot have that, because every position is already taken.

use gilrs::{Axis, Button, Gilrs};
use minifb::{Key, Window};

use crate::libretro;

use crate::libretro::{
    JOY_A, JOY_B, JOY_DOWN, JOY_L, JOY_LEFT, JOY_R, JOY_RIGHT, JOY_SELECT, JOY_START, JOY_UP,
    JOY_X, JOY_Y,
};
use crate::manifest::PadLayout;

/// Past this, a stick counts as a d-pad press.
const STICK_DEADZONE: f32 = 0.5;

pub struct Pads {
    gilrs: Option<Gilrs>,
    layout: PadLayout,
}

impl Pads {
    pub fn new(layout: PadLayout) -> Self {
        match Gilrs::new() {
            Ok(g) => {
                for (_id, pad) in g.gamepads() {
                    println!("[input] gamepad: {}", pad.name());
                }
                Pads { gilrs: Some(g), layout }
            }
            Err(e) => {
                eprintln!("[input] no gamepad support ({e}); keyboard only");
                Pads { gilrs: None, layout }
            }
        }
    }

    /// Drain pending gamepad events so the state we read below is current.
    pub fn pump(&mut self) {
        if let Some(g) = self.gilrs.as_mut() {
            while let Some(event) = g.next_event() {
                g.update(&event);
            }
        }
    }

    pub fn state(&self) -> u32 {
        let mut bits = 0u32;
        let Some(g) = self.gilrs.as_ref() else {
            return bits;
        };
        for (_id, pad) in g.gamepads() {
            if !pad.is_connected() {
                continue;
            }
            match self.layout {
                // Two face buttons, so South *or* East is A and West *or*
                // North is B, and neither habit needs remapping.
                PadLayout::Nes => {
                    set(&mut bits, JOY_A, pad.is_pressed(Button::South) || pad.is_pressed(Button::East));
                    set(&mut bits, JOY_B, pad.is_pressed(Button::West) || pad.is_pressed(Button::North));
                }
                // The SNES pad has the same diamond every modern pad has, so
                // the mapping is positional and every button is where the
                // original hardware had it.
                PadLayout::Snes => {
                    set(&mut bits, JOY_B, pad.is_pressed(Button::South));
                    set(&mut bits, JOY_A, pad.is_pressed(Button::East));
                    set(&mut bits, JOY_Y, pad.is_pressed(Button::West));
                    set(&mut bits, JOY_X, pad.is_pressed(Button::North));
                    set(&mut bits, JOY_L, pad.is_pressed(Button::LeftTrigger));
                    set(&mut bits, JOY_R, pad.is_pressed(Button::RightTrigger));
                }
            }
            set(&mut bits, JOY_START, pad.is_pressed(Button::Start));
            set(&mut bits, JOY_SELECT, pad.is_pressed(Button::Select));

            let sx = pad.value(Axis::LeftStickX);
            let sy = pad.value(Axis::LeftStickY);
            set(&mut bits, JOY_UP, pad.is_pressed(Button::DPadUp) || sy > STICK_DEADZONE);
            set(&mut bits, JOY_DOWN, pad.is_pressed(Button::DPadDown) || sy < -STICK_DEADZONE);
            set(&mut bits, JOY_LEFT, pad.is_pressed(Button::DPadLeft) || sx < -STICK_DEADZONE);
            set(&mut bits, JOY_RIGHT, pad.is_pressed(Button::DPadRight) || sx > STICK_DEADZONE);
        }
        bits
    }
}

/// The keyboard mapping does not vary by layout: Z/X are B/A on both consoles,
/// and A/S/Q/W simply do nothing on a NES core.
pub fn keyboard(window: &Window) -> u32 {
    let mut bits = 0u32;
    set(&mut bits, JOY_UP, window.is_key_down(Key::Up));
    set(&mut bits, JOY_DOWN, window.is_key_down(Key::Down));
    set(&mut bits, JOY_LEFT, window.is_key_down(Key::Left));
    set(&mut bits, JOY_RIGHT, window.is_key_down(Key::Right));
    set(&mut bits, JOY_B, window.is_key_down(Key::Z));
    set(&mut bits, JOY_A, window.is_key_down(Key::X));
    set(&mut bits, JOY_Y, window.is_key_down(Key::A));
    set(&mut bits, JOY_X, window.is_key_down(Key::S));
    set(&mut bits, JOY_L, window.is_key_down(Key::Q));
    set(&mut bits, JOY_R, window.is_key_down(Key::W));
    set(&mut bits, JOY_START, window.is_key_down(Key::Enter));
    set(&mut bits, JOY_SELECT, window.is_key_down(Key::RightShift));
    bits
}

fn set(bits: &mut u32, id: u32, pressed: bool) {
    if pressed {
        *bits |= 1 << id;
    }
}

/// Host key to `RETROK_*`, which is SDL 1.2's keysym set — so the letters are
/// their lowercase ASCII codes and the rest is a short mechanical table.
///
/// The function keys are deliberately absent. F1-F11 are the harness's own
/// controls (save state, RAM search, probe), and a game that also received them
/// would be typing into the thing being measured.
const KEYMAP: &[(Key, u32)] = &[
    (Key::Up, 273),
    (Key::Down, 274),
    (Key::Right, 275),
    (Key::Left, 276),
    (Key::LeftCtrl, 306),
    (Key::RightCtrl, 305),
    (Key::LeftShift, 304),
    (Key::RightShift, 303),
    (Key::LeftAlt, 308),
    (Key::RightAlt, 307),
    (Key::Space, 32),
    (Key::Enter, 13),
    (Key::Escape, 27),
    (Key::Tab, 9),
    (Key::Backspace, 8),
    (Key::Comma, 44),
    (Key::Period, 46),
    (Key::Minus, 45),
    (Key::Equal, 61),
    (Key::Semicolon, 59),
    (Key::Apostrophe, 39),
    (Key::Slash, 47),
    (Key::Backslash, 92),
    (Key::LeftBracket, 91),
    (Key::RightBracket, 93),
    (Key::Key0, 48),
    (Key::Key1, 49),
    (Key::Key2, 50),
    (Key::Key3, 51),
    (Key::Key4, 52),
    (Key::Key5, 53),
    (Key::Key6, 54),
    (Key::Key7, 55),
    (Key::Key8, 56),
    (Key::Key9, 57),
    (Key::A, 97),
    (Key::B, 98),
    (Key::C, 99),
    (Key::D, 100),
    (Key::E, 101),
    (Key::F, 102),
    (Key::G, 103),
    (Key::H, 104),
    (Key::I, 105),
    (Key::J, 106),
    (Key::K, 107),
    (Key::L, 108),
    (Key::M, 109),
    (Key::N, 110),
    (Key::O, 111),
    (Key::P, 112),
    (Key::Q, 113),
    (Key::R, 114),
    (Key::S, 115),
    (Key::T, 116),
    (Key::U, 117),
    (Key::V, 118),
    (Key::W, 119),
    (Key::X, 120),
    (Key::Y, 121),
    (Key::Z, 122),
];

/// Push this frame's held keys into the core, as edges.
///
/// Edges rather than a bitmask because a core may read its keyboard either
/// way: through `input_state`, which wants the held set, or through the
/// callback it registered with `SET_KEYBOARD_CALLBACK`, which wants press and
/// release. Both are fed from here, so which one a core prefers is its own
/// business.
pub fn keyboard_passthrough(window: &Window) {
    for (key, code) in KEYMAP {
        if window.is_key_down(*key) {
            libretro::key_down(*code);
        } else {
            libretro::key_up(*code);
        }
    }
}
