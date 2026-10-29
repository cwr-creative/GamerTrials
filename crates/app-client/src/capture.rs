//! The settings page's ears: what is held now, and the next thing pressed.
//!
//! Read in Rust, on the same path play reads it (`keys`), rather than from
//! the page's own key events: a binding captured in the webview's vocabulary
//! would need translating into virtual keys, and a gamepad is not a key event
//! at all (ADR 0028). One thread owns the gamepad reader, because gilrs is not
//! promised to be `Send`, and answers the page's requests in turn.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use session_libretro::input::{PadState, Pads};
use session_libretro::manifest::PadLayout;

use crate::keys;
use crate::settings::{Input, MAX_CHORD};

const VK_ESCAPE: i32 = 0x1B;
const VK_F12: i32 = 0x7B;

enum Req {
    Held { main: Option<isize>, deadzone: f32, reply: Sender<Vec<Input>> },
    Capture { main: Option<isize>, deadzone: f32, chord: bool, reply: Sender<Option<Vec<Input>>> },
}

#[derive(Clone)]
pub struct Capture {
    tx: Sender<Req>,
    cancel: Arc<AtomicBool>,
}

impl Capture {
    pub fn spawn() -> Capture {
        let (tx, rx) = mpsc::channel::<Req>();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        thread::Builder::new()
            .name("settings-input".into())
            .spawn(move || {
                // Made on first use, so a player who never opens Settings
                // never has a second gamepad reader.
                let mut pads: Option<Pads> = None;
                let mut read = move || {
                    let p = pads.get_or_insert_with(|| Pads::new(PadLayout::Snes));
                    p.pump();
                    p.raw()
                };
                for req in rx {
                    match req {
                        Req::Held { main, deadzone, reply } => {
                            let held = if in_front(main) { keys::all_held(&read(), deadzone) } else { Vec::new() };
                            let _ = reply.send(held);
                        }
                        Req::Capture { main, deadzone, chord, reply } => {
                            let _ = reply.send(capture(&mut read, main, deadzone, chord, &stop));
                        }
                    }
                }
            })
            .expect("spawn the settings input thread");
        Capture { tx, cancel }
    }

    /// Everything held now; nothing while another program is in front.
    pub fn held(&self, main: Option<isize>, deadzone: f32) -> Vec<Input> {
        let (reply, rx) = mpsc::channel();
        if self.tx.send(Req::Held { main, deadzone, reply }).is_err() {
            return Vec::new();
        }
        rx.recv().unwrap_or_default()
    }

    /// Wait for the player to press something and say what it was. A `chord`
    /// collects everything pressed until all of it is let go. Escape cancels,
    /// and so does [`Capture::cancel`]; either gives `None`, as do five
    /// seconds of nothing.
    pub fn next(&self, main: Option<isize>, deadzone: f32, chord: bool) -> Option<Vec<Input>> {
        self.cancel.store(false, Ordering::Relaxed);
        let (reply, rx) = mpsc::channel();
        self.tx.send(Req::Capture { main, deadzone, chord, reply }).ok()?;
        rx.recv().ok().flatten()
    }

    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn in_front(main: Option<isize>) -> bool {
    main.is_some_and(|h| keys::ours_in_front(&[h]))
}

fn capture(
    read: &mut impl FnMut() -> PadState,
    main: Option<isize>,
    deadzone: f32,
    chord: bool,
    cancel: &AtomicBool,
) -> Option<Vec<Input>> {
    let deadline = Instant::now() + Duration::from_secs(5);
    // Whatever is held as capture starts — the click that started it — is
    // not the answer until it has been let go and pressed again.
    let mut before = keys::all_held(&read(), deadzone);
    let mut got: Vec<Input> = Vec::new();
    loop {
        if cancel.load(Ordering::Relaxed) || (got.is_empty() && Instant::now() > deadline) {
            return None;
        }
        thread::sleep(Duration::from_millis(5));
        let pad = read();
        before.retain(|i| keys::held(i, &pad, deadzone));
        if in_front(main) {
            if let Some(i) = keys::first_new_press(&pad, deadzone, &before) {
                before.push(i);
                match i {
                    Input::Key(VK_ESCAPE) => return None,
                    // F12 quits the game window; bound to a button, it would
                    // quit mid-run.
                    Input::Key(VK_F12) => continue,
                    _ => {}
                }
                if !chord {
                    return Some(vec![i]);
                }
                if !got.contains(&i) && got.len() < MAX_CHORD {
                    got.push(i);
                }
            }
        }
        if chord && !got.is_empty() && !got.iter().any(|i| keys::held(i, &pad, deadzone)) {
            return Some(got);
        }
    }
}
