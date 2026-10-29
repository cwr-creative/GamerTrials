//! The transparent webview over the game window.
//!
//! An owned top-level window, not a child: Windows keeps an owned window above
//! its owner and minimises it with it, and WebView2 composes a transparent
//! page over whatever is behind a top-level window, which it does not do
//! behind a sibling child. The overlay tracks the game window's client area
//! from a thread, and ignores the mouse except while a menu is up.

use std::sync::Arc;
use std::time::Duration;

use tauri::{AppHandle, Emitter, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use winapi::shared::windef::{HWND, POINT, RECT};
use winapi::um::winuser::{
    ClientToScreen, GetClientRect, GetWindowLongW, GetWindowRect, IsIconic, SetWindowLongPtrW,
    SetWindowPos, GWLP_HWNDPARENT, GWL_EXSTYLE, SWP_NOACTIVATE, SWP_NOZORDER, WS_EX_LAYERED,
    WS_EX_TRANSPARENT,
};

#[derive(Clone)]
pub struct Overlay {
    window: WebviewWindow,
    game: isize,
    hwnd: isize,
    inner: Arc<()>,
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
            .resizable(false)
            .focused(false)
            .position(x as f64, y as f64)
            .inner_size(w as f64, h as f64)
            .build()?;
        let hwnd = window.hwnd()?.0 as isize;
        unsafe {
            // Owned by the game window: above it in z-order, minimised with it.
            SetWindowLongPtrW(hwnd as HWND, GWLP_HWNDPARENT, game as _);
        }
        let ov = Overlay { window, game, hwnd, inner: Arc::new(()) };
        ov.place((x, y, w, h))?;
        println!("[overlay] {hwnd:#x} over {game:#x} at {x},{y} {w}x{h}");
        Ok(ov)
    }

    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Whether the mouse goes through the overlay to the game window.
    pub fn set_passthrough(&self, on: bool) -> tauri::Result<()> {
        self.window.set_ignore_cursor_events(on)
    }

    /// Brings the overlay to the front, for when it has buttons to click.
    pub fn focus(&self) {
        unsafe {
            winapi::um::winuser::SetForegroundWindow(self.hwnd as HWND);
        }
        let _ = self.window.set_focus();
    }

    pub fn emit(&self, payload: serde_json::Value) {
        if let Err(e) = self.window.emit("overlay", payload) {
            eprintln!("[overlay] emit: {e}");
        }
    }

    fn place(&self, (x, y, w, h): (i32, i32, u32, u32)) -> tauri::Result<()> {
        unsafe {
            // One call, no activation, no z-order change: the owner relation
            // already keeps it on top of the game window.
            SetWindowPos(self.hwnd as HWND, std::ptr::null_mut(), x, y, w as i32, h as i32, SWP_NOZORDER | SWP_NOACTIVATE);
        }
        let _ = &self.inner;
        Ok(())
    }

    /// Follow the game window until it goes away. Runs on its own thread.
    pub fn track(&self) {
        let mut last = None;
        let mut hidden = false;
        loop {
            std::thread::sleep(Duration::from_millis(8));
            let iconic = unsafe { IsIconic(self.game as HWND) } != 0;
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

    /// Whether the window's extended style says the mouse goes through it.
    pub fn is_passthrough(&self) -> bool {
        let ex = unsafe { GetWindowLongW(self.hwnd as HWND, GWL_EXSTYLE) } as u32;
        ex & WS_EX_TRANSPARENT != 0 && ex & WS_EX_LAYERED != 0
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
    use winapi::um::winuser::{keybd_event, SetForegroundWindow, KEYEVENTF_KEYUP};

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
    println!("[spike] passthrough style: {}", if ov.is_passthrough() { "WS_EX_TRANSPARENT|LAYERED set" } else { "NOT set" });

    let (x, y, w, h) = window_rect(game).unwrap_or((100, 100, 800, 600));
    unsafe {
        SetWindowPos(game as HWND, std::ptr::null_mut(), x + 160, y + 90, w as i32, h as i32, SWP_NOZORDER);
    }
    all &= check("after move");
    unsafe {
        SetWindowPos(game as HWND, std::ptr::null_mut(), x + 160, y + 90, w as i32 + 240, h as i32 + 180, SWP_NOZORDER);
    }
    all &= check("after resize");

    // Focus the overlay — the state a player is in whenever a menu was up —
    // and hold Right for half a second. The game thread logs whether the
    // bits reached the core and who was in front.
    unsafe {
        SetForegroundWindow(ov.hwnd() as HWND);
    }
    std::thread::sleep(Duration::from_millis(300));
    let fg = unsafe { winapi::um::winuser::GetForegroundWindow() } as isize;
    println!("[spike] foreground is {} ({fg:#x})", if fg == ov.hwnd() { "the overlay" } else if fg == game { "the game window" } else { "something else" });
    unsafe {
        keybd_event(0x27, 0, 0, 0);
    }
    std::thread::sleep(Duration::from_millis(500));
    unsafe {
        keybd_event(0x27, 0, KEYEVENTF_KEYUP, 0);
    }
    std::thread::sleep(Duration::from_millis(300));

    // Put the window back where it was.
    unsafe {
        SetWindowPos(game as HWND, std::ptr::null_mut(), x, y, w as i32, h as i32, SWP_NOZORDER);
    }
    all &= check("after restore");
    println!("[spike] overlay size {:?} position {:?}", ov.size(), ov.position());
    println!("[spike] RESULT: {}", if all { "aligned throughout" } else { "misaligned at least once" });
    std::thread::sleep(Duration::from_secs(2));
}
