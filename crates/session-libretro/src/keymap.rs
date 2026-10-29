//! The two vocabularies a binding is written in: the RetroPad's buttons, and
//! the guest keyboard's keys.
//!
//! Every console core reads the same RetroPad (`RETRO_DEVICE_JOYPAD`), so a
//! binding written against it holds for every pad game; what differs between
//! consoles is only which of its buttons they have. A keyboard game takes
//! `RETROK_*` codes instead, and a manifest's `pad_keys` and a player's
//! remaps name them by the short names below (ADR 0028).
//!
//! Host keys are Windows virtual-key codes, carried here only as numbers so
//! that the client's `GetAsyncKeyState` reading and the names agree on one
//! table. The harness reads minifb's `Key` instead and keeps its own map in
//! `input`.

use serde::{Deserialize, Serialize};

/// One RetroPad button, in libretro's id order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RetroButton {
    B,
    Y,
    Select,
    Start,
    Up,
    Down,
    Left,
    Right,
    A,
    X,
    L,
    R,
    L2,
    R2,
    L3,
    R3,
}

impl RetroButton {
    pub const ALL: [RetroButton; 16] = [
        RetroButton::B,
        RetroButton::Y,
        RetroButton::Select,
        RetroButton::Start,
        RetroButton::Up,
        RetroButton::Down,
        RetroButton::Left,
        RetroButton::Right,
        RetroButton::A,
        RetroButton::X,
        RetroButton::L,
        RetroButton::R,
        RetroButton::L2,
        RetroButton::R2,
        RetroButton::L3,
        RetroButton::R3,
    ];

    /// The `RETRO_DEVICE_ID_JOYPAD_*` id, which is also its bit in the mask.
    pub fn id(self) -> u32 {
        self as u32
    }

    pub fn bit(self) -> u32 {
        1 << self.id()
    }

    pub fn name(self) -> &'static str {
        match self {
            RetroButton::B => "b",
            RetroButton::Y => "y",
            RetroButton::Select => "select",
            RetroButton::Start => "start",
            RetroButton::Up => "up",
            RetroButton::Down => "down",
            RetroButton::Left => "left",
            RetroButton::Right => "right",
            RetroButton::A => "a",
            RetroButton::X => "x",
            RetroButton::L => "l",
            RetroButton::R => "r",
            RetroButton::L2 => "l2",
            RetroButton::R2 => "r2",
            RetroButton::L3 => "l3",
            RetroButton::R3 => "r3",
        }
    }

    pub fn parse(name: &str) -> Option<RetroButton> {
        RetroButton::ALL.into_iter().find(|b| b.name() == name)
    }
}

/// A key a keyboard game can be sent: its name, its `RETROK_*` code, and the
/// Windows virtual key that types it on the host.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GuestKey {
    pub name: &'static str,
    pub retrok: u32,
    pub vk: i32,
}

const fn k(name: &'static str, retrok: u32, vk: i32) -> GuestKey {
    GuestKey { name, retrok, vk }
}

/// Every key a keyboard game is passed. `RETROK_*` is SDL 1.2's keysym set,
/// so letters are their lowercase ASCII.
///
/// The function keys are deliberately absent: the client and the harness own
/// them, and a DOS game's F-keys are its save and load — a load in a race is
/// a run that skipped its own middle.
pub const GUEST_KEYS: &[GuestKey] = &[
    k("up", 273, 0x26),
    k("down", 274, 0x28),
    k("right", 275, 0x27),
    k("left", 276, 0x25),
    k("lctrl", 306, 0xA2),
    k("rctrl", 305, 0xA3),
    k("lshift", 304, 0xA0),
    k("rshift", 303, 0xA1),
    k("lalt", 308, 0xA4),
    k("ralt", 307, 0xA5),
    k("space", 32, 0x20),
    k("enter", 13, 0x0D),
    k("escape", 27, 0x1B),
    k("tab", 9, 0x09),
    k("backspace", 8, 0x08),
    k("comma", 44, 0xBC),
    k("period", 46, 0xBE),
    k("minus", 45, 0xBD),
    k("equals", 61, 0xBB),
    k("semicolon", 59, 0xBA),
    k("apostrophe", 39, 0xDE),
    k("slash", 47, 0xBF),
    k("backslash", 92, 0xDC),
    k("lbracket", 91, 0xDB),
    k("rbracket", 93, 0xDD),
    k("0", 48, 0x30),
    k("1", 49, 0x31),
    k("2", 50, 0x32),
    k("3", 51, 0x33),
    k("4", 52, 0x34),
    k("5", 53, 0x35),
    k("6", 54, 0x36),
    k("7", 55, 0x37),
    k("8", 56, 0x38),
    k("9", 57, 0x39),
    k("a", 97, 0x41),
    k("b", 98, 0x42),
    k("c", 99, 0x43),
    k("d", 100, 0x44),
    k("e", 101, 0x45),
    k("f", 102, 0x46),
    k("g", 103, 0x47),
    k("h", 104, 0x48),
    k("i", 105, 0x49),
    k("j", 106, 0x4A),
    k("k", 107, 0x4B),
    k("l", 108, 0x4C),
    k("m", 109, 0x4D),
    k("n", 110, 0x4E),
    k("o", 111, 0x4F),
    k("p", 112, 0x50),
    k("q", 113, 0x51),
    k("r", 114, 0x52),
    k("s", 115, 0x53),
    k("t", 116, 0x54),
    k("u", 117, 0x55),
    k("v", 118, 0x56),
    k("w", 119, 0x57),
    k("x", 120, 0x58),
    k("y", 121, 0x59),
    k("z", 122, 0x5A),
];

pub fn guest_key(name: &str) -> Option<&'static GuestKey> {
    GUEST_KEYS.iter().find(|g| g.name == name)
}

pub fn guest_key_for_vk(vk: i32) -> Option<&'static GuestKey> {
    GUEST_KEYS.iter().find(|g| g.vk == vk)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn button_ids_are_libretros() {
        assert_eq!(RetroButton::B.id(), crate::libretro::JOY_B);
        assert_eq!(RetroButton::Start.id(), crate::libretro::JOY_START);
        assert_eq!(RetroButton::A.id(), crate::libretro::JOY_A);
        assert_eq!(RetroButton::R.id(), crate::libretro::JOY_R);
        assert_eq!(RetroButton::R3.id(), 15);
        for b in RetroButton::ALL {
            assert_eq!(RetroButton::parse(b.name()), Some(b));
        }
    }

    #[test]
    fn guest_keys_are_unique() {
        for (i, a) in GUEST_KEYS.iter().enumerate() {
            for b in &GUEST_KEYS[i + 1..] {
                assert_ne!(a.name, b.name);
                assert_ne!(a.retrok, b.retrok);
                assert_ne!(a.vk, b.vk);
            }
        }
    }
}
