//! The transparent webview over the game window.
//!
//! An owned top-level window, not a child: Windows keeps an owned window above
//! its owner and minimises it with it, and WebView2 composes a transparent
//! page over whatever is behind a top-level window, which it does not do
//! behind a sibling child. The overlay tracks the game window's client area
//! from a thread, and ignores the mouse except while a menu is up.
//!
//! On Linux (ADR 0035) both are X11 windows and the overlay is the game
//! window's transient, which is X11's owned window; `native` holds what
//! differs.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub use native::{client_rect, handle, window_rect};

#[derive(Clone)]
pub struct Overlay {
    window: WebviewWindow,
    game: isize,
    hwnd: isize,
    /// What `set_passthrough` last asked for, for a platform that cannot be
    /// asked back.
    passthrough: Arc<AtomicBool>,
}

impl Overlay {
    pub fn create(app: &AppHandle, game: isize) -> Result<Overlay, Box<dyn std::error::Error>> {
        let (x, y, w, h) = client_rect(game).ok_or("cannot read the game window's rect")?;
        let window = WebviewWindowBuilder::new(app, "overlay", WebviewUrl::App("overlay.html".into()))
            .title("GamerTrials")
            .transparent(true)
            .decorations(false)
            .shadow(false)
            .always_on_top(true)
            .skip_taskbar(true)
            // GTK holds a window it may not resize at its first size, even
            // against the program's own `set_size`; there is no frame to drag.
            .resizable(cfg!(not(target_os = "linux")))
            .focused(false)
            .position(x as f64, y as f64)
            .inner_size(w as f64, h as f64)
            // No `additional_browser_args` here: WebView2 runs one browser
            // process per app, with the arguments of the first webview made
            // (the main window's), and refuses a second webview whose
            // arguments differ (0x8007139F). wry's defaults already include
            // `--autoplay-policy=no-user-gesture-required`, which is what a
            // spectator's sound needs to start without a click (ADR 0032).
            .build()?;
        let hwnd = handle(&window).ok_or("cannot read the overlay's native window")?;
        // Owned by the game window: above it in z-order, minimised with it.
        native::set_owner(hwnd, game);
        let ov = Overlay { window, game, hwnd, passthrough: Arc::new(AtomicBool::new(false)) };
        ov.place((x, y, w, h))?;
        println!("[overlay] {hwnd:#x} over {game:#x} at {x},{y} {w}x{h}");
        Ok(ov)
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Whether the mouse goes through the overlay to the game window.
    pub fn set_passthrough(&self, on: bool) -> tauri::Result<()> {
        self.passthrough.store(on, Ordering::Relaxed);
        self.window.set_ignore_cursor_events(on)
    }

    /// Brings the overlay to the front, for when it has buttons to click.
    pub fn focus(&self) {
        native::bring_to_front(self.hwnd);
        let _ = self.window.set_focus();
    }

    pub fn emit(&self, payload: serde_json::Value) {
        if let Err(e) = self.window.emit("overlay", payload) {
            eprintln!("[overlay] emit: {e}");
        }
    }

    fn place(&self, rect: (i32, i32, u32, u32)) -> tauri::Result<()> {
        native::place(&self.window, self.hwnd, rect)
    }

    /// Follow the game window until it goes away. Runs on its own thread.
    pub fn track(&self) {
        let mut last = None;
        let mut hidden = false;
        loop {
            std::thread::sleep(Duration::from_millis(8));
            let iconic = native::is_minimised(self.game);
            if iconic != hidden {
                hidden = iconic;
                let _ = if hidden { self.window.hide() } else { self.window.show() };
            }
            if hidden {
                continue;
            }
            let Some(rect) = client_rect(self.game) else {
                // The game window is gone; so is the reason to be here.
                let _ = self.window.close();
                return;
            };
            if last != Some(rect) {
                last = Some(rect);
                if self.place(rect).is_err() {
                    return;
                }
            }
        }
    }

    /// Whether the mouse goes through it, as the window itself says where
    /// it can be asked.
    pub fn is_passthrough(&self) -> bool {
        native::is_passthrough(self.hwnd).unwrap_or_else(|| self.passthrough.load(Ordering::Relaxed))
    }

    pub fn size(&self) -> Option<PhysicalSize<u32>> {
        self.window.inner_size().ok()
    }

    pub fn position(&self) -> Option<PhysicalPosition<i32>> {
        self.window.outer_position().ok()
    }
}

/// The scripted self-test: move and resize the game window and check the
/// overlay followed; make the overlay the foreground window and press a key;
/// report the extended style. Every result is one `[spike]` line, so the
/// log is the measurement.
pub fn spike(ov: &Overlay, game: isize) {
    let check = |what: &str| {
        std::thread::sleep(Duration::from_millis(250));
        let g = client_rect(game);
        let o = window_rect(ov.hwnd());
        let ok = g.is_some() && g == o;
        println!("[spike] {what}: game client {g:?}, overlay {o:?} -> {}", if ok { "aligned" } else { "MISALIGNED" });
        ok
    };

    // After the intro and countdown, so the key below reaches a running game.
    std::thread::sleep(Duration::from_millis(7500));
    println!("[spike] starting checks");
    let mut all = check("initial");
    println!("[spike] passthrough: {}", if ov.is_passthrough() { "set" } else { "NOT set" });

    let (x, y, w, h) = window_rect(game).unwrap_or((100, 100, 800, 600));
    native::move_resize(game, x + 160, y + 90, w, h);
    all &= check("after move");
    native::move_resize(game, x + 160, y + 90, w + 240, h + 180);
    all &= check("after resize");

    // Focus the overlay — the state a player is in whenever a menu was up —
    // and hold Right for half a second. The game thread logs whether the
    // bits reached the core and who was in front.
    native::bring_to_front(ov.hwnd());
    std::thread::sleep(Duration::from_millis(300));
    let fg = native::foreground();
    println!(
        "[spike] foreground is {} ({fg:#x})",
        if fg == ov.hwnd() { "the overlay" } else if fg == game { "the game window" } else { "something else" }
    );
    native::hold_right(Duration::from_millis(500));
    std::thread::sleep(Duration::from_millis(300));

    // Put the window back where it was.
    native::move_resize(game, x, y, w, h);
    all &= check("after restore");
    println!("[spike] overlay size {:?} position {:?}", ov.size(), ov.position());
    println!("[spike] RESULT: {}", if all { "aligned throughout" } else { "misaligned at least once" });
    std::thread::sleep(Duration::from_secs(2));
}

#[cfg(windows)]
mod native {
    use std::time::Duration;

    use tauri::WebviewWindow;
    use winapi::shared::windef::{HWND, POINT, RECT};
    use winapi::um::winuser::{
        keybd_event, ClientToScreen, GetClientRect, GetForegroundWindow, GetWindowLongW, GetWindowRect, IsIconic,
        SetForegroundWindow, SetWindowLongPtrW, SetWindowPos, GWLP_HWNDPARENT, GWL_EXSTYLE, KEYEVENTF_KEYUP,
        SWP_NOACTIVATE, SWP_NOZORDER, WS_EX_LAYERED, WS_EX_TRANSPARENT,
    };

    pub fn handle(window: &WebviewWindow) -> Option<isize> {
        window.hwnd().ok().map(|h| h.0 as isize)
    }

    /// The game window's client area in screen coordinates: `(x, y, w, h)`.
    pub fn client_rect(hwnd: isize) -> Option<(i32, i32, u32, u32)> {
        unsafe {
            let h = hwnd as HWND;
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if GetClientRect(h, &mut rc) == 0 {
                return None;
            }
            let mut origin = POINT { x: 0, y: 0 };
            if ClientToScreen(h, &mut origin) == 0 {
                return None;
            }
            Some((origin.x, origin.y, (rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32))
        }
    }

    pub fn window_rect(hwnd: isize) -> Option<(i32, i32, u32, u32)> {
        unsafe {
            let mut rc = RECT { left: 0, top: 0, right: 0, bottom: 0 };
            if GetWindowRect(hwnd as HWND, &mut rc) == 0 {
                return None;
            }
            Some((rc.left, rc.top, (rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32))
        }
    }

    pub fn set_owner(hwnd: isize, owner: isize) {
        unsafe {
            SetWindowLongPtrW(hwnd as HWND, GWLP_HWNDPARENT, owner as _);
        }
    }

    pub fn place(_: &WebviewWindow, hwnd: isize, (x, y, w, h): (i32, i32, u32, u32)) -> tauri::Result<()> {
        unsafe {
            // One call, no activation, no z-order change: the owner relation
            // already keeps it on top of the game window.
            SetWindowPos(hwnd as HWND, std::ptr::null_mut(), x, y, w as i32, h as i32, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        Ok(())
    }

    pub fn is_minimised(hwnd: isize) -> bool {
        unsafe { IsIconic(hwnd as HWND) != 0 }
    }

    pub fn bring_to_front(hwnd: isize) {
        unsafe {
            SetForegroundWindow(hwnd as HWND);
        }
    }

    pub fn foreground() -> isize {
        unsafe { GetForegroundWindow() as isize }
    }

    /// Whether the window's extended style says the mouse goes through it.
    pub fn is_passthrough(hwnd: isize) -> Option<bool> {
        let ex = unsafe { GetWindowLongW(hwnd as HWND, GWL_EXSTYLE) } as u32;
        Some(ex & WS_EX_TRANSPARENT != 0 && ex & WS_EX_LAYERED != 0)
    }

    pub fn move_resize(hwnd: isize, x: i32, y: i32, w: u32, h: u32) {
        unsafe {
            SetWindowPos(hwnd as HWND, std::ptr::null_mut(), x, y, w as i32, h as i32, SWP_NOZORDER);
        }
    }

    pub fn hold_right(for_: Duration) {
        unsafe {
            keybd_event(0x27, 0, 0, 0);
        }
        std::thread::sleep(for_);
        unsafe {
            keybd_event(0x27, 0, KEYEVENTF_KEYUP, 0);
        }
    }
}

#[cfg(target_os = "linux")]
mod native {
    use std::time::Duration;

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use tauri::{PhysicalPosition, PhysicalSize, WebviewWindow};

    use session_libretro::x11;

    // The window manager's frame is a window of its own, so a window's rect
    // is its client rect.
    pub use session_libretro::x11::{client_rect, client_rect as window_rect, move_resize};

    /// The webview's X11 window. Anything else means GTK chose Wayland,
    /// where the overlay cannot be placed at all.
    pub fn handle(window: &WebviewWindow) -> Option<isize> {
        match window.window_handle().ok()?.as_raw() {
            RawWindowHandle::Xlib(h) => Some(h.window as isize),
            other => {
                eprintln!("[overlay] not an X11 window ({other:?}); is GDK_BACKEND=x11 set?");
                None
            }
        }
    }

    pub fn set_owner(hwnd: isize, owner: isize) {
        x11::set_transient_for(hwnd, owner);
    }

    /// Through GTK rather than behind its back, so its idea of the window's
    /// size stays the X server's.
    pub fn place(window: &WebviewWindow, _: isize, (x, y, w, h): (i32, i32, u32, u32)) -> tauri::Result<()> {
        window.set_position(PhysicalPosition::new(x, y))?;
        window.set_size(PhysicalSize::new(w, h))
    }

    pub fn is_minimised(hwnd: isize) -> bool {
        x11::is_minimised(hwnd)
    }

    pub fn bring_to_front(hwnd: isize) {
        x11::activate(hwnd);
    }

    pub fn foreground() -> isize {
        x11::active_window().unwrap_or(0)
    }

    /// GTK passes the mouse through with an empty input shape, which X11
    /// answers for only through the Shape extension; the overlay's own record
    /// answers instead.
    pub fn is_passthrough(_: isize) -> Option<bool> {
        None
    }

    /// The scripted key needs XTest, which this client does not load: the
    /// spike measures the rest and says so.
    pub fn hold_right(for_: Duration) {
        println!("[spike] no key injection on Linux; hold Right by hand within {for_:?}");
        std::thread::sleep(for_);
    }
}
