//! Keyboard reading that does not depend on which window has focus.
//!
//! The harness reads keys from its minifb window's message queue, which only
//! sees them while that window is focused. Under the overlay (ADR 0027) the
//! focused window is usually the webview, so the game window would go deaf.
//! `GetAsyncKeyState` reads the key's physical state instead, from any thread
//! and whatever has focus; the one guard needed is that *one of our windows*
//! is in front, so a game does not keep playing from keystrokes typed into
//! another program.

use winapi::um::winuser::{GetAsyncKeyState, GetForegroundWindow};

use session_libretro::libretro::{
    self, JOY_A, JOY_B, JOY_DOWN, JOY_L, JOY_LEFT, JOY_R, JOY_RIGHT, JOY_SELECT, JOY_START,
    JOY_UP, JOY_X, JOY_Y,
};

/// Virtual-key codes (winuser.h) for the keys the pad mapping uses.
const VK_BACK: i32 = 0x08;
const VK_TAB: i32 = 0x09;
const VK_RETURN: i32 = 0x0D;
const VK_ESCAPE: i32 = 0x1B;
const VK_SPACE: i32 = 0x20;
const VK_LEFT: i32 = 0x25;
const VK_UP: i32 = 0x26;
const VK_RIGHT: i32 = 0x27;
const VK_DOWN: i32 = 0x28;
const VK_F12: i32 = 0x7B;
const VK_LSHIFT: i32 = 0xA0;
const VK_RSHIFT: i32 = 0xA1;
const VK_LCONTROL: i32 = 0xA2;
const VK_RCONTROL: i32 = 0xA3;
const VK_LMENU: i32 = 0xA4;
const VK_RMENU: i32 = 0xA5;

fn down(vk: i32) -> bool {
    // High bit set: the key is down now.
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

/// Whether one of `ours` is the foreground window.
pub fn ours_in_front(ours: &[isize]) -> bool {
    let fg = unsafe { GetForegroundWindow() } as isize;
    ours.contains(&fg)
}

/// The quit key, read as a level like the harness does.
pub fn quit_pressed() -> bool {
    down(VK_F12)
}

/// The same pad mapping as `session_libretro::input::keyboard`: Z/X are B/A,
/// A/S are Y/X, Q/W the shoulders, Enter and right shift start and select.
pub fn joypad() -> u32 {
    let mut bits = 0u32;
    let mut set = |id: u32, pressed: bool| {
        if pressed {
            bits |= 1 << id;
        }
    };
    set(JOY_UP, down(VK_UP));
    set(JOY_DOWN, down(VK_DOWN));
    set(JOY_LEFT, down(VK_LEFT));
    set(JOY_RIGHT, down(VK_RIGHT));
    set(JOY_B, down(b'Z' as i32));
    set(JOY_A, down(b'X' as i32));
    set(JOY_Y, down(b'A' as i32));
    set(JOY_X, down(b'S' as i32));
    set(JOY_L, down(b'Q' as i32));
    set(JOY_R, down(b'W' as i32));
    set(JOY_START, down(VK_RETURN));
    set(JOY_SELECT, down(VK_RSHIFT));
    bits
}

/// Virtual key to `RETROK_*`, for a keyboard game. Letters and digits are
/// their ASCII; the rest is the same table `session_libretro::input` keeps.
const PASSTHROUGH: &[(i32, u32)] = &[
    (VK_UP, 273),
    (VK_DOWN, 274),
    (VK_RIGHT, 275),
    (VK_LEFT, 276),
    (VK_LCONTROL, 306),
    (VK_RCONTROL, 305),
    (VK_LSHIFT, 304),
    (VK_RSHIFT, 303),
    (VK_LMENU, 308),
    (VK_RMENU, 307),
    (VK_SPACE, 32),
    (VK_RETURN, 13),
    (VK_ESCAPE, 27),
    (VK_TAB, 9),
    (VK_BACK, 8),
    (0xBC, 44), // comma
    (0xBE, 46), // period
    (0xBD, 45), // minus
    (0xBB, 61), // equals
    (0xBA, 59), // semicolon
    (0xDE, 39), // apostrophe
    (0xBF, 47), // slash
    (0xDC, 92), // backslash
    (0xDB, 91), // left bracket
    (0xDD, 93), // right bracket
];

/// Push this frame's held keys into a keyboard game, as edges, the way
/// `session_libretro::input::keyboard_passthrough` does.
pub fn keyboard_passthrough() {
    for (vk, code) in PASSTHROUGH {
        if down(*vk) {
            libretro::key_down(*code);
        } else {
            libretro::key_up(*code);
        }
    }
    for c in b'a'..=b'z' {
        let vk = c.to_ascii_uppercase() as i32;
        if down(vk) {
            libretro::key_down(c as u32);
        } else {
            libretro::key_up(c as u32);
        }
    }
    for d in b'0'..=b'9' {
        if down(d as i32) {
            libretro::key_down(d as u32);
        } else {
            libretro::key_up(d as u32);
        }
    }
}

/// Release every passthrough key, for when input is withheld from the game.
pub fn keyboard_release_all() {
    for (_, code) in PASSTHROUGH {
        libretro::key_up(*code);
    }
    for c in b'a'..=b'z' {
        libretro::key_up(c as u32);
    }
    for d in b'0'..=b'9' {
        libretro::key_up(d as u32);
    }
}

