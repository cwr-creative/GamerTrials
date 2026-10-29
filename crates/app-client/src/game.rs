//! The game thread: one native window for the whole match, and in it one
//! runner after another.
//!
//! It is the driver loop that `session-fake`'s tests were running as test
//! code — enter, advance, observe, re-enter — owned by the client at last
//! (ADR 0009, ADR 0027). It judges only provisionally: its `Run` decides
//! whether to re-enter or stop on the frame it happens, and everything it
//! observed goes to the server to be judged for real. It never sends a
//! result anywhere as a result.
//!
//! It is told what to load and when to begin; it says what the game reported.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use challenge_core::{Next, Run, RunResult};
use minifb::{Scale, ScaleMode, Window, WindowOptions};
use session_api::{Presentable, SessionEvent, StageRunner, StageSpec};
use session_libretro::audio::{self, Audio};
use session_libretro::libretro;
use session_libretro::manifest::{Controller, Manifest};
use session_libretro::runner::LibretroRunner;
use session_libretro::{input, prep};

use crate::keys;

/// One round's content: what to derive and run, and the bound on the run.
pub struct Load {
    pub round: u64,
    pub manifest: PathBuf,
    pub stage: String,
    pub challenge: String,
    /// Rolls the challenge's patch slots; ignored for a challenge without any.
    pub seed: u64,
    pub limit: u64,
}

/// What the client tells the game thread.
pub enum Command {
    /// Derive the content, start the runner, enter the stage, and hold.
    Load(Load),
    /// The countdown is over: start the run and advance.
    Begin,
    /// Advance again after a pause.
    Resume,
    /// Drop the runner; keep the window.
    Unload,
    /// The overlay's window handle, so keys typed into it count as ours.
    Overlay(isize),
    Quit,
}

/// What the game thread tells the client.
#[derive(Debug)]
pub enum Report {
    /// The window is up. Sent once, first.
    Window { hwnd: isize },
    /// The round's content is derived and the stage entered; the run has not
    /// started. `artifact` is what the server compares across players.
    Loaded { round: u64, artifact: String },
    LoadFailed { round: u64, error: String },
    /// One advanced frame worth telling the server about: it carried an
    /// event, or the provisional judge settled something on it.
    Frame { round: u64, frame: u64, events: Vec<SessionEvent> },
    /// A time trial re-entered the stage after an attempt ended.
    Reentered { round: u64, attempt: u32, banked: u64 },
    /// The provisional verdict. `forfeited_at` is the frame the player gave
    /// up on, when they did; the server is told that first.
    Finished { round: u64, result: RunResult, forfeited_at: Option<u64> },
    /// The player pressed the pause key; the game is held.
    Paused,
    Closed,
}

/// The game being run right now, and the run being judged on it.
struct Loaded {
    round: u64,
    runner: LibretroRunner,
    spec: StageSpec,
    run: Run,
    /// `Begin` has been given.
    live: bool,
    done: bool,
    pads: Option<input::Pads>,
    mouse: Option<input::Mouse>,
    _audio: Option<Audio>,
    frame_time: Duration,
    audio_target: Option<usize>,
    next_deadline: Instant,
    last_seq: u64,
}

const VK_F9: i32 = 0x78;
const VK_F10: i32 = 0x79;

pub fn run(scale: usize, cmds: Receiver<Command>, reports: Sender<Report>) -> Result<(), String> {
    // Sized for the NES; every other console is scaled into it, which the
    // aspect-preserving scale mode does per frame.
    let (width, height) = (256usize, 240usize);
    let mut window = Window::new(
        "GamerTrials",
        width,
        height,
        WindowOptions {
            scale: match scale {
                4 => Scale::X4,
                1 => Scale::X1,
                3 => Scale::X2,
                _ => Scale::X2,
            },
            scale_mode: ScaleMode::AspectRatioStretch,
            resize: true,
            ..WindowOptions::default()
        },
    )
    .map_err(|e| format!("cannot open window: {e}"))?;
    window.set_target_fps(0);
    // An unpainted window is white, and the overlay is transparent.
    clear(&mut window, width, height)?;
    let hwnd = window.get_window_handle() as isize;
    let _ = reports.send(Report::Window { hwnd });

    let mut loaded: Option<Loaded> = None;
    let mut held = false;
    let mut ours = vec![hwnd];
    let mut f9_was_down = false;
    let mut f10_was_down = false;

    while window.is_open() && !keys::quit_pressed() {
        loop {
            match cmds.try_recv() {
                Ok(Command::Load(l)) => {
                    let round = l.round;
                    // Drop the old core before loading the next: the frontend
                    // statics are per core.
                    loaded = None;
                    match load(l, &mut window) {
                        Ok(x) => {
                            let _ = reports.send(Report::Loaded { round, artifact: x.1 });
                            loaded = Some(x.0);
                            held = false;
                        }
                        Err(error) => {
                            let _ = reports.send(Report::LoadFailed { round, error });
                        }
                    }
                }
                Ok(Command::Begin) => {
                    if let Some(l) = loaded.as_mut() {
                        if !l.live && !l.done {
                            l.run.enter();
                            l.live = true;
                            l.next_deadline = Instant::now();
                            held = false;
                        }
                    }
                }
                Ok(Command::Resume) => {
                    held = false;
                    if let Some(l) = loaded.as_mut() {
                        l.next_deadline = Instant::now();
                    }
                }
                Ok(Command::Unload) => loaded = None,
                Ok(Command::Overlay(h)) => ours = vec![hwnd, h],
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => {
                    let _ = reports.send(Report::Closed);
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        if loaded.as_ref().is_some_and(|l| l.runner.shutdown_requested()) {
            // The guest exited the game — Doom's own quit. Nothing to run.
            if let Some(l) = loaded.as_mut() {
                forfeit(l, &reports);
            }
        }

        // The two keys the client owns during play, as edges.
        let in_front = keys::ours_in_front(&ours);
        let f9 = in_front && keys::tapped(VK_F9);
        let f10 = in_front && keys::tapped(VK_F10);
        if let Some(l) = loaded.as_mut() {
            if l.live && !l.done && !held {
                if f9 && !f9_was_down {
                    forfeit(l, &reports);
                }
                if f10 && !f10_was_down {
                    held = true;
                    let _ = reports.send(Report::Paused);
                }
            }
        }
        f9_was_down = f9;
        f10_was_down = f10;

        match loaded.as_mut() {
            None => {
                window.update();
                std::thread::sleep(Duration::from_millis(16));
            }
            Some(l) if held || !l.live || l.done => {
                // Present whatever is there, pump the window, wait a frame.
                if !l.live {
                    // The entry frame, if the core drew one on entering.
                    present(l, &mut window)?;
                } else {
                    window.update();
                }
                std::thread::sleep(l.frame_time);
            }
            Some(l) => {
                step(l, &mut window, in_front, &reports)?;
            }
        }
    }
    let _ = reports.send(Report::Closed);
    Ok(())
}

fn load(l: Load, window: &mut Window) -> Result<(Loaded, String), String> {
    let manifest = Manifest::load(&l.manifest)?;
    let stage = manifest
        .stage(&l.stage)
        .ok_or_else(|| format!("{} declares no stage `{}`", manifest.title, l.stage))?
        .clone();
    let challenge = manifest
        .challenge(&l.challenge)
        .ok_or_else(|| format!("{} declares no challenge `{}`", manifest.title, l.challenge))?
        .clone();
    let seed = challenge.is_generated().then_some(l.seed);
    let content =
        prep::content_for(&manifest, &stage, Some(&challenge), seed, &Default::default())?;
    let artifact = content.key.0.clone();
    let title = content.title.clone();
    let spec = manifest.stage_spec(&stage, content.key.clone());
    let judged = manifest.challenge_spec(&challenge);
    let controller = manifest.libretro.controller;
    let mut runner = LibretroRunner::start(manifest, None, content, Vec::new())?;
    let av = runner.av_info();
    let audio = match audio::start(av.timing.sample_rate) {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("[audio] disabled: {e}");
            None
        }
    };
    let frame_time = Duration::from_secs_f64(1.0 / av.timing.fps.max(1.0));
    let audio_target = audio
        .as_ref()
        .map(|a| (audio::TARGET_QUEUED_FRAMES as f64 * a.device_rate / av.timing.fps) as usize);
    window.set_title(&format!("{title} \u{2014} GamerTrials"));
    runner.enter(&spec).map_err(|e| e.to_string())?;
    let run = Run::new(judged).map_err(|e| e.to_string())?.with_limit(l.limit);
    let (pads, mouse) = match controller {
        Controller::Pad(layout) => (Some(input::Pads::new(layout)), None),
        Controller::Keyboard => (None, Some(input::Mouse::new())),
    };
    Ok((
        Loaded {
            round: l.round,
            runner,
            spec,
            run,
            live: false,
            done: false,
            pads,
            mouse,
            _audio: audio,
            frame_time,
            audio_target,
            next_deadline: Instant::now(),
            last_seq: 0,
        },
        artifact,
    ))
}

/// One frame of play: pace, read input, advance, judge, present.
fn step(
    l: &mut Loaded,
    window: &mut Window,
    in_front: bool,
    reports: &Sender<Report>,
) -> Result<(), String> {
    match l.audio_target {
        Some(target) => {
            let give_up_at = Instant::now() + l.frame_time * 4;
            while audio::queued_frames() > target && Instant::now() < give_up_at {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        None => {
            let now = Instant::now();
            if l.next_deadline > now {
                std::thread::sleep(l.next_deadline - now);
            } else {
                l.next_deadline = now;
            }
            l.next_deadline += l.frame_time;
        }
    }
    window.update();

    match l.pads.as_mut() {
        Some(p) => {
            p.pump();
            let bits = if in_front { p.state() | keys::joypad() } else { 0 };
            libretro::JOYPAD.store(bits, Ordering::Relaxed);
        }
        None => {
            if in_front {
                keys::keyboard_passthrough();
                if let Some(m) = l.mouse.as_mut() {
                    m.pump(window);
                }
            } else {
                keys::keyboard_release_all();
            }
        }
    }

    let events = l.runner.advance().map_err(|e| e.to_string())?.to_vec();
    let frame = l.runner.frame();
    let next = l.run.observe(frame, &events);
    if !events.is_empty() || next != Next::Continue {
        let _ = reports.send(Report::Frame { round: l.round, frame, events });
    }
    match next {
        Next::Continue => {}
        Next::Reenter => {
            l.runner.enter(&l.spec).map_err(|e| e.to_string())?;
            l.run.enter();
            let _ = reports.send(Report::Reentered {
                round: l.round,
                attempt: l.run.attempts(),
                banked: l.run.frames(),
            });
        }
        Next::Finished(result) => {
            l.done = true;
            let _ = reports.send(Report::Finished { round: l.round, result, forfeited_at: None });
        }
    }
    present(l, window)
}

fn forfeit(l: &mut Loaded, reports: &Sender<Report>) {
    if !l.live || l.done {
        return;
    }
    let frame = l.runner.frame();
    let result = l.run.forfeit();
    l.done = true;
    let _ = reports.send(Report::Finished { round: l.round, result, forfeited_at: Some(frame) });
}

fn present(l: &mut Loaded, window: &mut Window) -> Result<(), String> {
    let mut painted = false;
    let mut paint_error = None;
    l.last_seq = l.runner.present(l.last_seq, &mut |f| {
        painted = true;
        if let Err(e) = window.update_with_buffer(f.pixels, f.width, f.height) {
            paint_error = Some(e.to_string());
        }
    });
    if let Some(e) = paint_error {
        return Err(format!("cannot present frame: {e}"));
    }
    if !painted {
        window.update();
    }
    Ok(())
}

fn clear(window: &mut Window, width: usize, height: usize) -> Result<(), String> {
    window
        .update_with_buffer(&vec![0u32; width * height], width, height)
        .map_err(|e| format!("cannot clear window: {e}"))
}
