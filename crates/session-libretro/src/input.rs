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
//!   already made. A PC has a mouse beside its keyboard, so this shape carries
//!   one too ([`Mouse`]), and a game that ignores it is not bothered by it.
//!
//! The libretro joypad layout *is* the SNES pad, so all twelve ids are always
//! mapped and a NES core simply ignores the four it does not have. The one
//! thing a manifest picks is how a modern controller's face diamond lands on
//! it — see [`PadLayout`] — because that is a genuine difference between the
//! two consoles rather than a preference: a two-button game wants both the "A
//! is on the right" and "A is on the bottom" habits to work, and a four-button
//! game cannot have that, because every position is already taken.

use gilrs::{Axis, Button, Gilrs};
use minifb::{Key, MouseButton, MouseMode, Window};

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

/// The host mouse, as the relative motion a DOS game reads.
///
/// A mouse that turns a player has to be captured: an absolute cursor stops
/// producing motion at the edge of the screen, which is a player who can no
/// longer turn left. So a click in the window captures it — the cursor is
/// hidden, held to the window, and put back in the middle after every frame so
/// that motion is measured from there — and losing focus (Alt-Tab) releases
/// it. The click that captures is swallowed rather than passed on, because in
/// Doom it would be a shot.
///
/// Motion is in host pixels after Windows' own pointer acceleration, which is
/// what a cursor-warping frontend gets; raw input would need the window's
/// message loop, and minifb owns that.
pub struct Mouse {
    captured: bool,
    /// Held from the capturing click until it is released, so the game never
    /// sees the second half of a press it did not see start.
    swallow_left: bool,
}

impl Mouse {
    pub fn new() -> Self {
        Mouse { captured: false, swallow_left: false }
    }

    /// Latch this frame's motion and buttons into the core. Once per frame,
    /// after the window has been pumped and before the frame runs.
    pub fn pump(&mut self, window: &mut Window) {
        if !window.is_active() {
            self.release(window);
            libretro::set_mouse(0, 0, 0);
            return;
        }
        let left = window.get_mouse_down(MouseButton::Left);
        if !self.captured {
            if left && window.get_mouse_pos(MouseMode::Discard).is_some() && cursor::capture(window) {
                window.set_cursor_visibility(false);
                self.captured = true;
                self.swallow_left = true;
            }
            libretro::set_mouse(0, 0, 0);
            return;
        }
        let Some((dx, dy)) = cursor::recentre(window) else {
            self.release(window);
            libretro::set_mouse(0, 0, 0);
            return;
        };
        if !left {
            self.swallow_left = false;
        }
        let mut buttons = 0u32;
        set(&mut buttons, libretro::MOUSE_LEFT, left && !self.swallow_left);
        set(&mut buttons, libretro::MOUSE_RIGHT, window.get_mouse_down(MouseButton::Right));
        set(&mut buttons, libretro::MOUSE_MIDDLE, window.get_mouse_down(MouseButton::Middle));
        // Best effort: minifb clears the wheel at the start of every pump and
        // this loop pumps twice a frame, so a notch that lands during the
        // presenting pump is lost. Nothing in the library uses the wheel yet.
        if let Some((_, wheel)) = window.get_scroll_wheel() {
            set(&mut buttons, libretro::MOUSE_WHEELUP, wheel > 0.0);
            set(&mut buttons, libretro::MOUSE_WHEELDOWN, wheel < 0.0);
        }
        libretro::set_mouse(dx, dy, buttons);
    }

    fn release(&mut self, window: &mut Window) {
        if self.captured {
            cursor::release();
            // `set_cursor_visibility` is a counter on Windows, not a flag, so
            // it is called once per change and never per frame.
            window.set_cursor_visibility(true);
            self.captured = false;
        }
        self.swallow_left = false;
    }
}

impl Default for Mouse {
    fn default() -> Self {
        Mouse::new()
    }
}

/// Holding the cursor in the window and measuring it from the centre.
#[cfg(windows)]
mod cursor {
    use minifb::Window;
    use winapi::shared::windef::{HWND, POINT, RECT};
    use winapi::um::winuser::{ClientToScreen, ClipCursor, GetClientRect, GetCursorPos, SetCursorPos};

    /// The window's client area and its centre, in screen coordinates.
    fn client(window: &Window) -> Option<(RECT, POINT)> {
        let hwnd = window.get_window_handle() as HWND;
        let mut rect = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        unsafe {
            if GetClientRect(hwnd, &mut rect) == 0 || rect.right <= 0 || rect.bottom <= 0 {
                return None;
            }
            let mut tl = POINT { x: 0, y: 0 };
            let mut br = POINT { x: rect.right, y: rect.bottom };
            if ClientToScreen(hwnd, &mut tl) == 0 || ClientToScreen(hwnd, &mut br) == 0 {
                return None;
            }
            let screen = RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y };
            let centre = POINT { x: (tl.x + br.x) / 2, y: (tl.y + br.y) / 2 };
            Some((screen, centre))
        }
    }

    pub fn capture(window: &Window) -> bool {
        let Some((rect, centre)) = client(window) else {
            return false;
        };
        unsafe {
            ClipCursor(&rect);
            SetCursorPos(centre.x, centre.y);
        }
        true
    }

    /// Motion since the last call, then the cursor back to the centre. The
    /// clip is re-applied every frame because the window may have moved, and
    /// Windows drops a clip on its own whenever another window takes focus.
    pub fn recentre(window: &Window) -> Option<(i32, i32)> {
        let (rect, centre) = client(window)?;
        let mut at = POINT { x: 0, y: 0 };
        unsafe {
            if GetCursorPos(&mut at) == 0 {
                return None;
            }
            ClipCursor(&rect);
            SetCursorPos(centre.x, centre.y);
        }
        Some((at.x - centre.x, at.y - centre.y))
    }

    pub fn release() {
        unsafe {
            ClipCursor(std::ptr::null());
        }
    }
}

/// Elsewhere there is no capture, and so no motion: the buttons still work.
/// The project targets Windows; this is only here so the crate still builds.
#[cfg(not(windows))]
mod cursor {
    use minifb::Window;

    pub fn capture(_: &Window) -> bool {
        true
    }

    pub fn recentre(_: &Window) -> Option<(i32, i32)> {
        Some((0, 0))
    }

    pub fn release() {}
}
