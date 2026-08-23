//! Keyboard and gamepad collapsed into one libretro joypad bitmask.
//!
//! Port 0 only. Phase 1 has one local player.

use gilrs::{Axis, Button, Gilrs};
use minifb::{Key, Window};

use crate::libretro::{JOY_A, JOY_B, JOY_DOWN, JOY_LEFT, JOY_RIGHT, JOY_SELECT, JOY_START, JOY_UP};

/// Past this, a stick counts as a d-pad press.
const STICK_DEADZONE: f32 = 0.5;

pub struct Pads {
    gilrs: Option<Gilrs>,
}

impl Pads {
    pub fn new() -> Self {
        match Gilrs::new() {
            Ok(g) => {
                for (_id, pad) in g.gamepads() {
                    println!("[input] gamepad: {}", pad.name());
                }
                Pads { gilrs: Some(g) }
            }
            Err(e) => {
                eprintln!("[input] no gamepad support ({e}); keyboard only");
                Pads { gilrs: None }
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
            // South/East as A and West/North as B, so both the "A is on the
            // right" and "A is on the bottom" habits work without remapping.
            set(&mut bits, JOY_A, pad.is_pressed(Button::South) || pad.is_pressed(Button::East));
            set(&mut bits, JOY_B, pad.is_pressed(Button::West) || pad.is_pressed(Button::North));
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

pub fn keyboard(window: &Window) -> u32 {
    let mut bits = 0u32;
    set(&mut bits, JOY_UP, window.is_key_down(Key::Up));
    set(&mut bits, JOY_DOWN, window.is_key_down(Key::Down));
    set(&mut bits, JOY_LEFT, window.is_key_down(Key::Left));
    set(&mut bits, JOY_RIGHT, window.is_key_down(Key::Right));
    set(&mut bits, JOY_B, window.is_key_down(Key::Z));
    set(&mut bits, JOY_A, window.is_key_down(Key::X));
    set(&mut bits, JOY_START, window.is_key_down(Key::Enter));
    set(&mut bits, JOY_SELECT, window.is_key_down(Key::RightShift));
    bits
}

fn set(bits: &mut u32, id: u32, pressed: bool) {
    if pressed {
        *bits |= 1 << id;
    }
}
