//! Where the core's picture lands in the game window: the player's video
//! settings (ADR 0028), and nothing that reaches the core.
//!
//! The window is created once per match and is never told how to scale; this
//! module does. Each frame it builds a **canvas** with the window's own
//! shape, puts the cropped picture in it at 1:1 where it should appear, and
//! leaves black around it. The canvas is then stretched to the window —
//! by minifb's blit on Windows, by the X server's XRender on Linux (`game::Screen`)
//! — so the enlargement is the system's and no CPU upscale is added between
//! a frame and the screen. Placing the picture is
//! the whole of `scaling` and `aspect`: the canvas is sized so that the
//! stretch takes the picture to exactly the rectangle the setting wants.
//!
//! Spectating (ADR 0003) must encode the core's own frame, not this canvas:
//! a viewer's settings are theirs.
//!
//! In a match of several the right of the window is the sidebar, where the
//! overlay shows the other players (ADR 0038): the picture is placed in
//! what is left, against the left edge.

use session_api::VideoFrame;

use crate::settings::{Aspect, Crop, Scaling, VideoPrefs};

/// Where a picture goes, in canvas pixels: the canvas size, and the top-left
/// of the cropped picture copied into it at 1:1.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    pub canvas: (usize, usize),
    pub at: (usize, usize),
    /// The cropped picture's size, which is what is copied.
    pub picture: (usize, usize),
}

/// The cropped picture's size, never cropped to nothing.
pub fn cropped(frame: (usize, usize), crop: Crop) -> (usize, usize) {
    let keep = |n: usize, a: u32, b: u32| n.saturating_sub(a as usize + b as usize).max(n.min(16));
    (keep(frame.0, crop.left, crop.right), keep(frame.1, crop.top, crop.bottom))
}

/// The cropped picture's display aspect, width over height. `core_aspect` is
/// what the core reported for its whole picture; zero means it reported none.
pub fn display_aspect(frame: (usize, usize), picture: (usize, usize), core_aspect: f32, aspect: Aspect) -> f64 {
    let square = picture.0 as f64 / picture.1 as f64;
    match aspect {
        Aspect::Square => square,
        Aspect::Core => {
            if !(core_aspect.is_finite() && core_aspect > 0.0) || frame.0 == 0 || frame.1 == 0 {
                return square;
            }
            // The core's aspect is for the whole frame; a crop keeps the
            // pixels' shape, not the frame's.
            let par = core_aspect as f64 * frame.1 as f64 / frame.0 as f64;
            square * par
        }
    }
}

/// The rectangle the picture should fill, in client pixels: x, y, w, h.
pub fn target(client: (usize, usize), picture: (usize, usize), da: f64, scaling: Scaling) -> (f64, f64, f64, f64) {
    let (cw, ch) = (client.0 as f64, client.1 as f64);
    let fit = || {
        if cw / ch > da {
            (ch * da, ch)
        } else {
            (cw, cw / da)
        }
    };
    let (w, h) = match scaling {
        Scaling::Stretch => (cw, ch),
        Scaling::Fit => fit(),
        Scaling::Integer => {
            let ph = picture.1 as f64;
            let mut k = (ch / ph).floor();
            while k > 1.0 && k * ph * da > cw {
                k -= 1.0;
            }
            if k < 1.0 || k * ph * da > cw {
                fit()
            } else {
                (k * ph * da, k * ph)
            }
        }
    };
    ((cw - w) / 2.0, (ch - h) / 2.0, w, h)
}

/// The sidebar's width, in client pixels (ADR 0038): three 4:3 thumbnails
/// stacked, so four ninths of the height, and never more than two fifths of
/// the width. The overlay page reckons it the same way.
pub fn sidebar_width(client: (usize, usize)) -> usize {
    (client.1 * 4 / 9).min(client.0 * 2 / 5)
}

/// `sidebar`: leave the sidebar's width free on the right, and put the
/// picture against the left edge of the rest.
pub fn layout(client: (usize, usize), frame: (usize, usize), core_aspect: f32, prefs: &VideoPrefs, sidebar: bool) -> Layout {
    let client = (client.0.max(1), client.1.max(1));
    let picture = cropped(frame, prefs.crop);
    let da = display_aspect(frame, picture, core_aspect, prefs.aspect);
    let side = if sidebar { sidebar_width(client) } else { 0 };
    let area = ((client.0 - side).max(1), client.1);
    let (x, y, w, h) = target(area, picture, da, prefs.scaling);
    let x = if sidebar { 0.0 } else { x };
    // Canvas pixels per client pixel, on each axis, such that the picture
    // copied at 1:1 is stretched onto exactly (w, h).
    let fx = picture.0 as f64 / w;
    let fy = picture.1 as f64 / h;
    let canvas_w = ((client.0 as f64 * fx).round() as usize).max(picture.0);
    let canvas_h = ((client.1 as f64 * fy).round() as usize).max(picture.1);
    let at_x = ((x * fx).round() as usize).min(canvas_w - picture.0);
    let at_y = ((y * fy).round() as usize).min(canvas_h - picture.1);
    Layout { canvas: (canvas_w, canvas_h), at: (at_x, at_y), picture }
}

/// The client size a window wants for a picture at `window_scale`: the
/// picture's height times the scale, and its width at the display aspect,
/// with the sidebar beside it when there is one.
pub fn window_size(frame: (usize, usize), core_aspect: f32, prefs: &VideoPrefs, sidebar: bool) -> (u32, u32) {
    let picture = cropped(frame, prefs.crop);
    let da = display_aspect(frame, picture, core_aspect, prefs.aspect);
    let h = picture.1 as f64 * prefs.window_scale as f64;
    let side = if sidebar { h * 4.0 / 9.0 } else { 0.0 };
    ((h * da + side).round() as u32, h as u32)
}

/// Builds canvases, keeping the buffer between frames so the black around
/// the picture is only cleared when the layout changes.
#[derive(Default)]
pub struct Canvas {
    buf: Vec<u32>,
    last: Option<Layout>,
}

impl Canvas {
    pub fn draw(
        &mut self,
        f: &VideoFrame<'_>,
        client: (usize, usize),
        core_aspect: f32,
        prefs: &VideoPrefs,
        sidebar: bool,
    ) -> (&[u32], usize, usize) {
        let l = layout(client, (f.width, f.height), core_aspect, prefs, sidebar);
        if self.last != Some(l) {
            self.buf.clear();
            self.buf.resize(l.canvas.0 * l.canvas.1, 0);
            self.last = Some(l);
        }
        let left = prefs.crop.left as usize;
        let top = prefs.crop.top as usize;
        let left = left.min(f.width - l.picture.0);
        let top = top.min(f.height - l.picture.1);
        for row in 0..l.picture.1 {
            let src = (top + row) * f.width + left;
            let dst = (l.at.1 + row) * l.canvas.0 + l.at.0;
            self.buf[dst..dst + l.picture.0].copy_from_slice(&f.pixels[src..src + l.picture.0]);
        }
        (&self.buf, l.canvas.0, l.canvas.1)
    }
}

/// Sizing and fullscreen for the game window, which minifb has no API for.
#[cfg(windows)]
pub mod window {
    use winapi::shared::windef::{HWND, RECT};
    use winapi::um::winuser::{
        AdjustWindowRectEx, GetMonitorInfoW, GetWindowLongW, MonitorFromWindow, SetWindowLongW,
        SetWindowPos, GWL_EXSTYLE, GWL_STYLE, MONITORINFO, MONITOR_DEFAULTTONEAREST, SWP_FRAMECHANGED,
        SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER, WS_CAPTION, WS_MAXIMIZEBOX, WS_MINIMIZEBOX,
        WS_POPUP, WS_SYSMENU, WS_THICKFRAME,
    };

    const FRAME: u32 = WS_CAPTION | WS_THICKFRAME | WS_SYSMENU | WS_MINIMIZEBOX | WS_MAXIMIZEBOX;
    /// What borderless keeps of the frame. Neither draws anything without a
    /// caption, but Windows only minimises a window that has them: without
    /// them Win+Down and a click on the taskbar button do nothing, and only
    /// Win+D (which minimises everything) works.
    const KEPT: u32 = WS_SYSMENU | WS_MINIMIZEBOX;

    /// Resize so the client area is `w` by `h`, keeping the top-left.
    pub fn set_client_size(hwnd: isize, w: u32, h: u32) {
        let h_ = hwnd as HWND;
        unsafe {
            let style = GetWindowLongW(h_, GWL_STYLE) as u32;
            let ex = GetWindowLongW(h_, GWL_EXSTYLE) as u32;
            let mut rc = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
            AdjustWindowRectEx(&mut rc, style, 0, ex);
            SetWindowPos(
                h_,
                std::ptr::null_mut(),
                0,
                0,
                rc.right - rc.left,
                rc.bottom - rc.top,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
    }

    /// Borderless at the size of the monitor the window is on, or back to a
    /// framed window. The overlay follows the client rect by itself (ADR 0027).
    pub fn set_borderless(hwnd: isize, on: bool) {
        let h_ = hwnd as HWND;
        unsafe {
            let style = GetWindowLongW(h_, GWL_STYLE) as u32;
            if on {
                SetWindowLongW(h_, GWL_STYLE, ((style & !FRAME) | KEPT | WS_POPUP) as i32);
                let mon = MonitorFromWindow(h_, MONITOR_DEFAULTTONEAREST);
                let mut mi: MONITORINFO = std::mem::zeroed();
                mi.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
                if GetMonitorInfoW(mon, &mut mi) != 0 {
                    let r = mi.rcMonitor;
                    SetWindowPos(
                        h_,
                        std::ptr::null_mut(),
                        r.left,
                        r.top,
                        r.right - r.left,
                        r.bottom - r.top,
                        SWP_NOZORDER | SWP_FRAMECHANGED,
                    );
                }
            } else {
                SetWindowLongW(h_, GWL_STYLE, ((style & !WS_POPUP) | FRAME) as i32);
                SetWindowPos(
                    h_,
                    std::ptr::null_mut(),
                    80,
                    80,
                    0,
                    0,
                    SWP_NOZORDER | SWP_FRAMECHANGED | winapi::um::winuser::SWP_NOSIZE,
                );
            }
        }
    }
}

/// The same on X11 (ADR 0035). The window manager's frame is a window of
/// its own, so the client size is the window's size; borderless is the
/// window manager's fullscreen, which puts the window back where it was.
#[cfg(target_os = "linux")]
pub mod window {
    use session_libretro::x11;

    pub fn set_client_size(hwnd: isize, w: u32, h: u32) {
        x11::resize(hwnd, w, h);
    }

    pub fn set_borderless(hwnd: isize, on: bool) {
        x11::set_fullscreen(hwnd, on);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefs(scaling: Scaling, aspect: Aspect) -> VideoPrefs {
        VideoPrefs { scaling, aspect, ..Default::default() }
    }

    /// The canvas stretched to the client should take the picture to the
    /// target rectangle: check that by mapping the picture's edges back.
    fn on_screen(client: (usize, usize), l: &Layout) -> (f64, f64, f64, f64) {
        let sx = client.0 as f64 / l.canvas.0 as f64;
        let sy = client.1 as f64 / l.canvas.1 as f64;
        (l.at.0 as f64 * sx, l.at.1 as f64 * sy, l.picture.0 as f64 * sx, l.picture.1 as f64 * sy)
    }

    #[test]
    fn stretch_fills_the_window() {
        let l = layout((1000, 500), (256, 240), 4.0 / 3.0, &prefs(Scaling::Stretch, Aspect::Core), false);
        assert_eq!(l, Layout { canvas: (256, 240), at: (0, 0), picture: (256, 240) });
    }

    #[test]
    fn fit_keeps_the_cores_aspect_and_centres() {
        let client = (1920, 1080);
        let l = layout(client, (256, 240), 4.0 / 3.0, &prefs(Scaling::Fit, Aspect::Core), false);
        let (x, y, w, h) = on_screen(client, &l);
        assert!((h - 1080.0).abs() < 1.0 && (w - 1440.0).abs() < 4.0, "{w}x{h}");
        assert!((x - 240.0).abs() < 4.0 && y.abs() < 1.0);
    }

    #[test]
    fn square_pixels_are_square() {
        let client = (1920, 1080);
        let l = layout(client, (256, 240), 4.0 / 3.0, &prefs(Scaling::Fit, Aspect::Square), false);
        let (_, _, w, h) = on_screen(client, &l);
        assert!((w / h - 256.0 / 240.0).abs() < 0.01);
    }

    #[test]
    fn integer_is_a_whole_multiple_of_the_height() {
        let client = (1920, 1080);
        let l = layout(client, (256, 240), 4.0 / 3.0, &prefs(Scaling::Integer, Aspect::Square), false);
        let (x, y, w, h) = on_screen(client, &l);
        assert_eq!((w.round(), h.round()), (1024.0, 960.0), "4x");
        assert_eq!((x.round(), y.round()), (448.0, 60.0));
        // DOS's 320x200 at 4:3 in a window too small for 2x: 1x.
        let l = layout((500, 300), (320, 200), 4.0 / 3.0, &prefs(Scaling::Integer, Aspect::Core), false);
        let (_, _, w, h) = on_screen((500, 300), &l);
        assert!((h - 200.0).abs() < 1.0 && (w - 266.7).abs() < 2.0, "{w}x{h}");
    }

    #[test]
    fn a_crop_drops_the_edges_and_keeps_the_pixels_shape() {
        let mut p = prefs(Scaling::Fit, Aspect::Core);
        p.crop = Crop { top: 8, bottom: 8, left: 0, right: 0 };
        assert_eq!(cropped((256, 240), p.crop), (256, 224));
        let da = display_aspect((256, 240), (256, 224), 4.0 / 3.0, Aspect::Core);
        // The core reports its aspect as an f32.
        assert!((da - 4.0 / 3.0 * 240.0 / 224.0).abs() < 1e-6);
        assert_eq!(cropped((256, 240), Crop { top: 500, ..Default::default() }), (256, 16), "never to nothing");
    }

    #[test]
    fn the_window_is_the_scale_times_the_height() {
        let p = VideoPrefs { window_scale: 3, aspect: Aspect::Square, ..Default::default() };
        assert_eq!(window_size((256, 240), 4.0 / 3.0, &p, false), (768, 720));
        let p = VideoPrefs { window_scale: 2, ..Default::default() };
        assert_eq!(window_size((320, 200), 4.0 / 3.0, &p, false), (533, 400));
        // And a sidebar beside it, four ninths of the height.
        assert_eq!(window_size((320, 200), 4.0 / 3.0, &p, true), (711, 400));
    }

    #[test]
    fn a_sidebar_puts_the_picture_against_the_left_edge() {
        let client = (1920, 1080);
        assert_eq!(sidebar_width(client), 480);
        let l = layout(client, (256, 240), 4.0 / 3.0, &prefs(Scaling::Fit, Aspect::Core), true);
        let (x, y, w, h) = on_screen(client, &l);
        assert!((h - 1080.0).abs() < 1.0 && (w - 1440.0).abs() < 4.0, "{w}x{h}");
        assert!(x.abs() < 1.0 && y.abs() < 1.0, "{x},{y}");
        // A window too narrow for both: the picture shrinks to leave it.
        let client = (1200, 1080);
        assert_eq!(sidebar_width(client), 480);
        let l = layout(client, (256, 240), 4.0 / 3.0, &prefs(Scaling::Fit, Aspect::Core), true);
        let (x, y, w, h) = on_screen(client, &l);
        assert!((w - 720.0).abs() < 4.0 && (h - 540.0).abs() < 4.0, "{w}x{h}");
        assert!(x.abs() < 1.0 && (y - 270.0).abs() < 4.0, "{x},{y}");
        // Stretch fills what the sidebar leaves.
        let l = layout(client, (256, 240), 4.0 / 3.0, &prefs(Scaling::Stretch, Aspect::Core), true);
        let (x, _, w, h) = on_screen(client, &l);
        assert!(x.abs() < 1.0 && (w - 720.0).abs() < 4.0 && (h - 1080.0).abs() < 1.0, "{w}x{h}");
    }

    #[test]
    fn the_canvas_holds_the_cropped_picture() {
        let pixels: Vec<u32> = (0..16 * 20).collect();
        let f = VideoFrame { pixels: &pixels, width: 16, height: 20, seq: 1 };
        let mut p = prefs(Scaling::Fit, Aspect::Square);
        p.crop = Crop { top: 2, bottom: 2, left: 0, right: 0 };
        let mut c = Canvas::default();
        let (buf, w, h) = c.draw(&f, (32, 32), 0.0, &p, false);
        // 16x16 picture in a square window: no border at all.
        assert_eq!((w, h), (16, 16));
        assert_eq!(buf[0], 32, "row 2 of the frame is row 0 of the picture");
    }
}
