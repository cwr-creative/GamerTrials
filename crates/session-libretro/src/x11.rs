//! The X11 calls made on Linux (ADR 0035) where Windows would make Win32
//! ones: holding the mouse here (`input`), and the client's key reading,
//! window sizing and overlay. Not the adapter's own business, but the game
//! window is minifb's, and this is the crate that opens it.
//!
//! On Linux every window is an X11 window: minifb's game window and the
//! client's Tauri webviews alike, under XWayland on a Wayland desktop (the
//! client sets `GDK_BACKEND`; `Cargo.toml` leaves minifb only its X11
//! backend). Wayland lets no program place its window over another's, which
//! is the whole of the client's overlay (ADR 0027); X11 does, and XWayland
//! brings it to every Ubuntu desktop.
//!
//! Each thread opens its own connection, so nothing here needs
//! `XInitThreads`. An error on one of ours — a window gone between two calls
//! — is ignored rather than handed to Xlib's default handler, which exits the
//! process; an error on anyone else's connection (GDK's, minifb's) goes to
//! whatever handler was there before.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong};
use std::ptr;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use x11_dl::xlib::{self, Display, Xlib};

type Handler = unsafe extern "C" fn(*mut Display, *mut xlib::XErrorEvent) -> c_int;

static INSTALL: Once = Once::new();
static PREVIOUS: Mutex<Option<Handler>> = Mutex::new(None);
static OURS: Mutex<Vec<usize>> = Mutex::new(Vec::new());

unsafe extern "C" fn on_error(d: *mut Display, e: *mut xlib::XErrorEvent) -> c_int {
    if OURS.lock().map(|o| o.contains(&(d as usize))).unwrap_or(false) {
        return 0;
    }
    let previous = PREVIOUS.lock().ok().and_then(|p| *p);
    match previous {
        Some(h) => h(d, e),
        None => 0,
    }
}

/// How long one reading of the keyboard and the mouse buttons stands for
/// every key asked about: one poll of the bindings is one round trip, not one
/// per key.
const SNAPSHOT: Duration = Duration::from_millis(2);

struct Conn {
    x: Xlib,
    d: *mut Display,
    root: c_ulong,
    keycodes: RefCell<HashMap<c_ulong, u8>>,
    snapshot: Cell<Option<(Instant, [u8; 32], c_uint)>>,
}

impl Conn {
    fn open() -> Option<Conn> {
        let x = Xlib::open().ok()?;
        let d = unsafe { (x.XOpenDisplay)(ptr::null()) };
        if d.is_null() {
            return None;
        }
        OURS.lock().unwrap().push(d as usize);
        INSTALL.call_once(|| {
            let previous = unsafe { (x.XSetErrorHandler)(Some(on_error)) };
            *PREVIOUS.lock().unwrap() = previous;
        });
        let root = unsafe { (x.XDefaultRootWindow)(d) };
        Some(Conn { x, d, root, keycodes: RefCell::default(), snapshot: Cell::new(None) })
    }

    fn atom(&self, name: &str) -> c_ulong {
        let name = CString::new(name).expect("atom names have no NUL");
        unsafe { (self.x.XInternAtom)(self.d, name.as_ptr(), 0) }
    }

    /// A format-32 property's values, empty when it is absent.
    unsafe fn longs(&self, win: c_ulong, prop: c_ulong) -> Vec<c_ulong> {
        let (mut ty, mut format, mut n, mut after) = (0, 0, 0, 0);
        let mut data: *mut c_uchar = ptr::null_mut();
        let status = (self.x.XGetWindowProperty)(
            self.d, win, prop, 0, 16, 0, 0, // AnyPropertyType
            &mut ty, &mut format, &mut n, &mut after, &mut data,
        );
        let mut out = Vec::new();
        if status == 0 && !data.is_null() && format == 32 {
            // Xlib hands format-32 data back as longs, whatever their width.
            out.extend_from_slice(std::slice::from_raw_parts(data as *const c_ulong, n as usize));
        }
        if !data.is_null() {
            (self.x.XFree)(data.cast());
        }
        out
    }

    /// A message to the window manager about `win` (EWMH).
    unsafe fn tell_wm(&self, win: c_ulong, what: &str, data: [c_long; 5]) {
        let mut d = xlib::ClientMessageData::new();
        for (i, v) in data.into_iter().enumerate() {
            d.set_long(i, v);
        }
        let mut ev: xlib::XEvent = std::mem::zeroed();
        ev.client_message = xlib::XClientMessageEvent {
            type_: xlib::ClientMessage,
            serial: 0,
            send_event: 1,
            display: self.d,
            window: win,
            message_type: self.atom(what),
            format: 32,
            data: d,
        };
        (self.x.XSendEvent)(self.d, self.root, 0, xlib::SubstructureRedirectMask | xlib::SubstructureNotifyMask, &mut ev);
        (self.x.XFlush)(self.d);
    }

    fn input(&self) -> ([u8; 32], c_uint) {
        if let Some((at, keys, buttons)) = self.snapshot.get() {
            if at.elapsed() < SNAPSHOT {
                return (keys, buttons);
            }
        }
        let mut keys = [0 as c_char; 32];
        let mut buttons = 0;
        unsafe {
            (self.x.XQueryKeymap)(self.d, keys.as_mut_ptr());
            let (mut r, mut c, mut rx, mut ry, mut wx, mut wy) = (0, 0, 0, 0, 0, 0);
            (self.x.XQueryPointer)(self.d, self.root, &mut r, &mut c, &mut rx, &mut ry, &mut wx, &mut wy, &mut buttons);
        }
        let keys = keys.map(|b| b as u8);
        self.snapshot.set(Some((Instant::now(), keys, buttons)));
        (keys, buttons)
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if let Ok(mut o) = OURS.lock() {
            o.retain(|&d| d != self.d as usize);
        }
        unsafe {
            (self.x.XCloseDisplay)(self.d);
        }
    }
}

thread_local! {
    static CONN: Option<Conn> = Conn::open();
}

fn with<R>(f: impl FnOnce(&Conn) -> Option<R>) -> Option<R> {
    CONN.try_with(|c| c.as_ref().and_then(f)).ok().flatten()
}

/// A window's inside in root coordinates: `(x, y, w, h)`. The window
/// manager's frame is a window of its own, so this is the whole window.
pub fn client_rect(win: isize) -> Option<(i32, i32, u32, u32)> {
    with(|c| unsafe {
        let mut a: xlib::XWindowAttributes = std::mem::zeroed();
        if (c.x.XGetWindowAttributes)(c.d, win as c_ulong, &mut a) == 0 {
            return None;
        }
        let (mut x, mut y, mut child) = (0, 0, 0);
        if (c.x.XTranslateCoordinates)(c.d, win as c_ulong, c.root, 0, 0, &mut x, &mut y, &mut child) == 0 {
            return None;
        }
        Some((x, y, a.width.max(0) as u32, a.height.max(0) as u32))
    })
}

/// Whether the window manager has it minimised (ICCCM's `IconicState`).
pub fn is_minimised(win: isize) -> bool {
    with(|c| unsafe { Some(c.longs(win as c_ulong, c.atom("WM_STATE")).first() == Some(&3)) }).unwrap_or(false)
}

/// The window the window manager says is active, or failing that the one
/// with the keyboard.
pub fn active_window() -> Option<isize> {
    with(|c| unsafe {
        let active = c.longs(c.root, c.atom("_NET_ACTIVE_WINDOW")).first().copied().filter(|&w| w != 0);
        active
            .or_else(|| {
                let (mut focus, mut revert) = (0, 0);
                (c.x.XGetInputFocus)(c.d, &mut focus, &mut revert);
                // 0 is None and 1 is PointerRoot: neither is a window.
                (focus > 1).then_some(focus)
            })
            .map(|w| w as isize)
    })
}

/// Keep `win` above `owner` and minimise it with it: Windows' owned window.
pub fn set_transient_for(win: isize, owner: isize) {
    with(|c| unsafe {
        (c.x.XSetTransientForHint)(c.d, win as c_ulong, owner as c_ulong);
        (c.x.XFlush)(c.d);
        Some(())
    });
}

/// Ask the window manager to bring `win` to the front and give it the
/// keyboard. Sent as a pager (source 2), which window managers honour where
/// they would refuse a program asking for itself.
pub fn activate(win: isize) {
    with(|c| unsafe {
        c.tell_wm(win as c_ulong, "_NET_ACTIVE_WINDOW", [2, 0, 0, 0, 0]);
        Some(())
    });
}

pub fn resize(win: isize, w: u32, h: u32) {
    with(|c| unsafe {
        (c.x.XResizeWindow)(c.d, win as c_ulong, w.max(1), h.max(1));
        (c.x.XFlush)(c.d);
        Some(())
    });
}

pub fn move_resize(win: isize, x: i32, y: i32, w: u32, h: u32) {
    with(|c| unsafe {
        (c.x.XMoveResizeWindow)(c.d, win as c_ulong, x, y, w.max(1), h.max(1));
        (c.x.XFlush)(c.d);
        Some(())
    });
}

/// The window manager's fullscreen: no frame, the whole monitor the window
/// is on, and back where it was when it ends.
pub fn set_fullscreen(win: isize, on: bool) {
    with(|c| unsafe {
        let fullscreen = c.atom("_NET_WM_STATE_FULLSCREEN") as c_long;
        c.tell_wm(win as c_ulong, "_NET_WM_STATE", [on as c_long, fullscreen, 0, 1, 0]);
        Some(())
    });
}

/// Whether the key that types `keysym` is held now, whichever window has
/// the keyboard. Under XWayland that is only reported while one of our X11
/// windows has it, which is the only time it is asked (`keys::ours_in_front`).
pub fn keysym_down(keysym: c_ulong) -> bool {
    with(|c| {
        let code = *c
            .keycodes
            .borrow_mut()
            .entry(keysym)
            .or_insert_with(|| unsafe { (c.x.XKeysymToKeycode)(c.d, keysym) });
        if code == 0 {
            // No key on this layout types it.
            return Some(false);
        }
        let (keys, _) = c.input();
        Some(keys[code as usize / 8] & (1 << (code % 8)) != 0)
    })
    .unwrap_or(false)
}

/// The mouse buttons held now, as X11's button masks.
pub fn pointer_buttons() -> c_uint {
    with(|c| Some(c.input().1)).unwrap_or(0)
}

/// Where the pointer is, in `win`'s coordinates.
pub fn pointer_in(win: isize) -> Option<(i32, i32)> {
    with(|c| unsafe {
        let (mut r, mut ch, mut rx, mut ry, mut x, mut y, mut mask) = (0, 0, 0, 0, 0, 0, 0);
        let same_screen =
            (c.x.XQueryPointer)(c.d, win as c_ulong, &mut r, &mut ch, &mut rx, &mut ry, &mut x, &mut y, &mut mask);
        (same_screen != 0).then_some((x, y))
    })
}

/// Put the pointer at `(x, y)` in `win`'s coordinates.
pub fn warp_pointer(win: isize, x: i32, y: i32) {
    with(|c| unsafe {
        (c.x.XWarpPointer)(c.d, 0, win as c_ulong, 0, 0, 0, 0, x, y);
        (c.x.XFlush)(c.d);
        Some(())
    });
}
