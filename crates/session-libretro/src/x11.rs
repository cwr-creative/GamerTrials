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
//! whatever handler was there before. Ours are counted, so a setup can tell
//! whether the server refused it (`Presenter`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::ffi::CString;
use std::os::raw::{c_char, c_int, c_long, c_uchar, c_uint, c_ulong};
use std::ptr;
use std::sync::{Mutex, Once};
use std::time::{Duration, Instant};

use x11_dl::xlib::{self, Display, Xlib};
use x11_dl::xrender::{self, XRenderPictFormat, XTransform, Xrender};
use x11_dl::xshm::{XShmSegmentInfo, Xext};

type Handler = unsafe extern "C" fn(*mut Display, *mut xlib::XErrorEvent) -> c_int;

static INSTALL: Once = Once::new();
static PREVIOUS: Mutex<Option<Handler>> = Mutex::new(None);
/// Our connections, and how many errors each has had.
static OURS: Mutex<Vec<(usize, u64)>> = Mutex::new(Vec::new());

unsafe extern "C" fn on_error(d: *mut Display, e: *mut xlib::XErrorEvent) -> c_int {
    if let Ok(mut ours) = OURS.lock() {
        if let Some(o) = ours.iter_mut().find(|o| o.0 == d as usize) {
            o.1 += 1;
            return 0;
        }
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
        OURS.lock().unwrap().push((d as usize, 0));
        INSTALL.call_once(|| {
            let previous = unsafe { (x.XSetErrorHandler)(Some(on_error)) };
            *PREVIOUS.lock().unwrap() = previous;
        });
        let root = unsafe { (x.XDefaultRootWindow)(d) };
        Some(Conn { x, d, root, keycodes: RefCell::default(), snapshot: Cell::new(None) })
    }

    /// How many errors the server has sent this connection so far. Only
    /// what has reached us counts: `XSync` first to count a request's.
    fn errors(&self) -> u64 {
        OURS.lock().ok().and_then(|o| o.iter().find(|o| o.0 == self.d as usize).map(|o| o.1)).unwrap_or(0)
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
        // Hear every error still coming while it is ours to ignore: one
        // read by `XCloseDisplay` goes to the handler before ours, which is
        // Xlib's own when nobody else set one, and that exits the process.
        unsafe {
            (self.x.XSync)(self.d, 0);
        }
        if let Ok(mut o) = OURS.lock() {
            o.retain(|&(d, _)| d != self.d as usize);
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

/// How long `set_fullscreen` waits for the window manager to take a window
/// it has only just been shown.
const MANAGED: Duration = Duration::from_secs(1);

/// The window manager's fullscreen: no frame, the whole monitor the window
/// is on, and back where it was when it ends.
///
/// The request is a message to the window manager, which drops one about a
/// window it does not manage yet (EWMH: only a mapped window's state is
/// changed by message). The game window is mapped when it opens, and a quick
/// first round can ask before the manager has taken it: so this waits, for
/// a moment, for the manager's own mark that it has (ICCCM's `WM_STATE`).
pub fn set_fullscreen(win: isize, on: bool) {
    with(|c| unsafe {
        let wm_state = c.atom("WM_STATE");
        let until = Instant::now() + MANAGED;
        while c.longs(win as c_ulong, wm_state).is_empty() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
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

/// Paints the game window from the client's canvas, scaled by the X server.
///
/// minifb's X11 backend stretches every frame on the CPU to the window's
/// size and sends the result with a plain `XPutImage`, so a frame costs the
/// window's pixels, not the picture's: eight megabytes at 1920×1080, through
/// the X socket, on the thread that steps the core, which slows the game
/// itself once the window is large enough. Here only the canvas crosses —
/// through shared memory (MIT-SHM) when the server is on this machine, else
/// as a small `XPutImage` — into a pixmap, and XRender stretches that pixmap
/// over the window on the server, on the GPU under glamor. That is the twin
/// of `StretchDIBits`, which is what minifb does on Windows. minifb keeps the
/// window and its events; it is no longer asked to paint.
///
/// Its X11 resources are on this thread's connection, so it is made, used
/// and dropped on one thread (the raw pointers keep it there).
pub struct Presenter {
    win: c_ulong,
    render: Xrender,
    /// None when the server cannot reach this process's memory (a display
    /// across the network), or has refused it once.
    shm: Option<Xext>,
    visual: *mut xlib::Visual,
    depth: c_uint,
    format: *mut XRenderPictFormat,
    /// The window as XRender sees it. It holds through every resize.
    dst: c_ulong,
    src: Option<Source>,
}

/// One canvas size's pixmap and the image it is filled from.
struct Source {
    size: (u32, u32),
    pixmap: c_ulong,
    gc: xlib::GC,
    picture: c_ulong,
    image: *mut xlib::XImage,
    /// Boxed: Xlib keeps a pointer to it in the image.
    seg: Option<Box<XShmSegmentInfo>>,
    /// The image's pixels when they are not in shared memory.
    own: Vec<u32>,
    /// The window size the picture's transform was last set for.
    scaled_to: Option<(u32, u32)>,
    /// A put from the segment the server may not have read yet.
    pending: bool,
}

/// What `shmat` returns on failure.
fn shm_failed(addr: *mut libc::c_void) -> bool {
    addr as usize == usize::MAX
}

impl Presenter {
    /// None when the server has no XRender or the window is not a plain
    /// 24-bit one: then minifb paints, as before.
    pub fn new(win: isize) -> Option<Presenter> {
        let win = win as c_ulong;
        with(|c| unsafe {
            let render = Xrender::open().ok()?;
            let (mut event, mut error) = (0, 0);
            if (render.XRenderQueryExtension)(c.d, &mut event, &mut error) == 0 {
                return None;
            }
            let mut a: xlib::XWindowAttributes = std::mem::zeroed();
            if (c.x.XGetWindowAttributes)(c.d, win, &mut a) == 0 || a.depth != 24 {
                return None;
            }
            let format = (render.XRenderFindVisualFormat)(c.d, a.visual);
            if format.is_null() {
                return None;
            }
            let before = c.errors();
            let dst = (render.XRenderCreatePicture)(c.d, win, format, 0, ptr::null());
            (c.x.XSync)(c.d, 0);
            if c.errors() != before {
                return None;
            }
            let shm = Xext::open().ok().filter(|x| (x.XShmQueryExtension)(c.d) != 0);
            Some(Presenter { win, render, shm, visual: a.visual, depth: a.depth as c_uint, format, dst, src: None })
        })
    }

    /// Whether the canvas goes to the server through shared memory.
    pub fn shared(&self) -> bool {
        self.shm.is_some()
    }

    /// Put `canvas`, `w` by `h`, over the whole of a window `client` in
    /// size. False if the server refused what that took; minifb should
    /// paint from then on.
    pub fn paint(&mut self, canvas: &[u32], w: usize, h: usize, client: (u32, u32)) -> bool {
        if w == 0 || h == 0 || canvas.len() < w * h || client.0 == 0 || client.1 == 0 {
            return true;
        }
        with(|c| unsafe {
            let size = (w as u32, h as u32);
            if self.src.as_ref().map(|s| s.size) != Some(size) {
                if let Some(old) = self.src.take() {
                    self.free(c, old);
                }
                self.src = Some(self.source(c, size)?);
            }
            let s = self.src.as_mut()?;
            if s.pending {
                // The last frame's put must be read before the segment is
                // written again. Waited for here rather than after it was
                // sent, so the server scales that frame while this thread
                // emulates the next: on a server that scales on the CPU
                // (no glamor) that is most of a paint at a large size.
                (c.x.XSync)(c.d, 0);
                s.pending = false;
            }
            let img = &*s.image;
            let line = img.bytes_per_line as usize;
            for row in 0..h {
                let to = img.data.add(row * line) as *mut u32;
                ptr::copy_nonoverlapping(canvas[row * w..].as_ptr(), to, w);
            }
            match (&self.shm, &s.seg) {
                (Some(x), Some(_)) => {
                    (x.XShmPutImage)(c.d, s.pixmap, s.gc, s.image, 0, 0, 0, 0, size.0, size.1, 0);
                }
                _ => {
                    (c.x.XPutImage)(c.d, s.pixmap, s.gc, s.image, 0, 0, 0, 0, size.0, size.1);
                }
            }
            if s.scaled_to != Some(client) {
                // From window pixels back to canvas pixels, in 16.16.
                let fixed = |n: u32, d: u32| (n as f64 / d as f64 * 65536.0).round() as xrender::XFixed;
                let mut t = XTransform {
                    matrix: [[fixed(size.0, client.0), 0, 0], [0, fixed(size.1, client.1), 0], [0, 0, 1 << 16]],
                };
                (self.render.XRenderSetPictureTransform)(c.d, s.picture, &mut t);
                s.scaled_to = Some(client);
            }
            (self.render.XRenderComposite)(
                c.d,
                xrender::PictOpSrc,
                s.picture,
                0,
                self.dst,
                0,
                0,
                0,
                0,
                0,
                0,
                client.0,
                client.1,
            );
            (c.x.XFlush)(c.d);
            s.pending = s.seg.is_some();
            Some(true)
        })
        .unwrap_or(false)
    }

    /// The pixmap, picture and image for a canvas of `size`, in shared
    /// memory if the server will attach it.
    unsafe fn source(&mut self, c: &Conn, size: (u32, u32)) -> Option<Source> {
        let before = c.errors();
        let pixmap = (c.x.XCreatePixmap)(c.d, self.win, size.0, size.1, self.depth);
        let gc = (c.x.XCreateGC)(c.d, pixmap, 0, ptr::null_mut());
        let picture = (self.render.XRenderCreatePicture)(c.d, pixmap, self.format, 0, ptr::null());
        // Each canvas pixel a block of window pixels, as minifb drew it and
        // as a pixel-art picture wants.
        let nearest = CString::new("nearest").expect("no NUL");
        (self.render.XRenderSetPictureFilter)(c.d, picture, nearest.as_ptr(), ptr::null_mut(), 0);
        (c.x.XSync)(c.d, 0);
        let mut s = Source {
            size,
            pixmap,
            gc,
            picture,
            image: ptr::null_mut(),
            seg: None,
            own: Vec::new(),
            scaled_to: None,
            pending: false,
        };
        if c.errors() != before {
            self.free(c, s);
            return None;
        }
        if let Some((image, seg)) = self.shared_image(c, size) {
            s.image = image;
            s.seg = Some(seg);
            return Some(s);
        }
        s.own = vec![0u32; size.0 as usize * size.1 as usize];
        s.image = (c.x.XCreateImage)(
            c.d,
            self.visual,
            self.depth,
            xlib::ZPixmap,
            0,
            s.own.as_mut_ptr().cast(),
            size.0,
            size.1,
            32,
            0,
        );
        if s.image.is_null() {
            self.free(c, s);
            return None;
        }
        Some(s)
    }

    /// An image whose pixels are a shared segment the server has attached;
    /// or None, after which shared memory is not tried again.
    unsafe fn shared_image(&mut self, c: &Conn, size: (u32, u32)) -> Option<(*mut xlib::XImage, Box<XShmSegmentInfo>)> {
        let x = self.shm.as_ref()?;
        let mut seg: Box<XShmSegmentInfo> = Box::new(std::mem::zeroed());
        let image =
            (x.XShmCreateImage)(c.d, self.visual, self.depth, xlib::ZPixmap, ptr::null_mut(), &mut *seg, size.0, size.1);
        if image.is_null() {
            self.shm = None;
            return None;
        }
        let bytes = (*image).bytes_per_line as usize * size.1 as usize;
        let id = libc::shmget(libc::IPC_PRIVATE, bytes, libc::IPC_CREAT | 0o600);
        let addr = if id >= 0 { libc::shmat(id, ptr::null(), 0) } else { usize::MAX as *mut libc::c_void };
        let attached = !shm_failed(addr) && {
            seg.shmid = id;
            seg.shmaddr = addr.cast();
            seg.readOnly = 0;
            (*image).data = seg.shmaddr;
            let before = c.errors();
            (x.XShmAttach)(c.d, &mut *seg);
            (c.x.XSync)(c.d, 0);
            c.errors() == before
        };
        if id >= 0 {
            // Gone once both sides have let go of it, whatever happens to
            // this process.
            libc::shmctl(id, libc::IPC_RMID, ptr::null_mut());
        }
        if attached {
            return Some((image, seg));
        }
        if !shm_failed(addr) {
            libc::shmdt(addr);
        }
        (*image).data = ptr::null_mut();
        (c.x.XDestroyImage)(image);
        self.shm = None;
        None
    }

    unsafe fn free(&self, c: &Conn, mut s: Source) {
        if let (Some(x), Some(seg)) = (&self.shm, s.seg.as_mut()) {
            (x.XShmDetach)(c.d, &mut **seg);
            (c.x.XSync)(c.d, 0);
            libc::shmdt(seg.shmaddr.cast());
        }
        if !s.image.is_null() {
            // Its pixels are the segment's or `own`'s, not Xlib's to free.
            (*s.image).data = ptr::null_mut();
            (c.x.XDestroyImage)(s.image);
        }
        (self.render.XRenderFreePicture)(c.d, s.picture);
        (c.x.XFreeGC)(c.d, s.gc);
        (c.x.XFreePixmap)(c.d, s.pixmap);
        (c.x.XFlush)(c.d);
    }
}

impl Drop for Presenter {
    fn drop(&mut self) {
        let src = self.src.take();
        with(|c| unsafe {
            if let Some(s) = src {
                self.free(c, s);
            }
            (self.render.XRenderFreePicture)(c.d, self.dst);
            // Done before minifb destroys the window, which frees its
            // picture and would make this free an error.
            (c.x.XSync)(c.d, 0);
            Some(())
        });
    }
}
