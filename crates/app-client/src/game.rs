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
use session_api::{ChallengeKind, Presentable, SessionEvent, StageRunner, StageSpec};
use session_libretro::audio::{self, Audio};
use session_libretro::input::PadState;
use session_libretro::libretro;
use session_libretro::manifest::{Controller, Manifest, PadLayout};
use session_libretro::runner::LibretroRunner;
use session_libretro::{input, prep};

use crate::keys;
use crate::stream::{Picture, Sound, Streamer};
use crate::settings::{self, Display, ResolvedInput, VideoPrefs};
use crate::video::{self, Canvas};

/// One round's content: what to derive and run, and the bound on the run.
pub struct Load {
    pub round: u64,
    pub manifest: PathBuf,
    pub stage: String,
    pub challenge: String,
    /// Rolls the challenge's patch slots; ignored for a challenge without any.
    pub seed: u64,
    pub limit: u64,
    /// The player's bindings for this game, resolved through every layer.
    pub input: ResolvedInput,
    /// The player's video settings for this game's core.
    pub video: VideoPrefs,
    pub display: Display,
}

/// What the client tells the game thread.
pub enum Command {
    /// Derive the content, start the runner, enter the stage, and hold.
    Load(Box<Load>),
    /// The countdown is over: start the run and advance.
    Begin,
    /// Advance again after a pause.
    Resume,
    /// The player gave up from the pause menu.
    Forfeit,
    /// Drop the runner; keep the window.
    Unload,
    /// The overlay's window handle, so keys typed into it count as ours.
    Overlay(isize),
    /// How many are watching this run (ADR 0032). Above zero, the picture
    /// and sound are encoded and reported; a rise starts on a key frame.
    Stream(u32),
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
    /// A time trial's clock: frames across every attempt, and the core's
    /// rate to read them in seconds by. Sent whenever the tenth of a second
    /// it reads changes, and once more, `stopped`, when the run is over.
    Clock { round: u64, frames: u64, fps: f64, stopped: bool },
    /// The provisional verdict. `forfeited_at` is the frame the player gave
    /// up on, when they did; the server is told that first.
    Finished { round: u64, result: RunResult, forfeited_at: Option<u64> },
    /// The player pressed the pause key; the game is held.
    Paused,
    /// One encoded picture for whoever is watching (ADR 0032): the bytes
    /// `ToServer::Video` carries.
    Video { round: u64, frame: u64, key: bool, width: u32, height: u32, aspect: f32, data: Vec<u8> },
    /// The sound under the pictures, likewise.
    Audio { round: u64, frame: u64, rate: u32, channels: u8, data: Vec<u8> },
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
    pads: input::Pads,
    /// Present for a keyboard game.
    mouse: Option<input::Mouse>,
    input: ResolvedInput,
    video: VideoPrefs,
    display: Display,
    /// The picture's shape as the core reports it, for `Aspect::Core`.
    core_aspect: f32,
    _audio: Option<Audio>,
    frame_time: Duration,
    fps: f64,
    /// A time trial, which shows its clock.
    timed: bool,
    /// The tenth of a second the clock last read.
    clock_tenth: Option<u64>,
    audio_target: Option<usize>,
    next_deadline: Instant,
    last_seq: u64,
    /// Who is watching, by count; the picture is encoded while above zero.
    watchers: u32,
    /// The next picture sent starts a group: a watcher just arrived.
    want_key: bool,
}

impl Loaded {
    /// Sending to a spectator right now.
    fn streaming(&self) -> bool {
        self.watchers > 0 && self.live && !self.done
    }
}

pub fn run(cmds: Receiver<Command>, reports: Sender<Report>) -> Result<(), String> {
    // Opened at the NES's size three times over; the first round resizes it
    // to its own core's picture (`video::window_size`), and every frame is
    // placed in it by `video::Canvas`, so minifb is only ever asked to stretch.
    let (width, height) = (768usize, 720usize);
    let mut window = Window::new(
        "GamerTrials",
        width,
        height,
        WindowOptions {
            scale: Scale::X1,
            scale_mode: ScaleMode::Stretch,
            resize: true,
            ..WindowOptions::default()
        },
    )
    .map_err(|e| format!("cannot open window: {e}"))?;
    window.set_target_fps(0);
    // An unpainted window is white, and the overlay is transparent.
    clear(&mut window, 16, 16)?;
    let hwnd = window.get_window_handle() as isize;
    let _ = reports.send(Report::Window { hwnd });

    let mut loaded: Option<Loaded> = None;
    let mut held = false;
    let mut ours = vec![hwnd];
    let streamer = Streamer::spawn(reports.clone());
    // Watchers can arrive before the round has loaded.
    let mut watchers = 0u32;
    let mut pause_edge = settings::Edge::default();
    let mut give_up_edge = settings::Edge::default();
    let mut canvas = Canvas::default();
    let mut borderless = false;
    // The windowed size last asked for, so a player's own resize survives
    // every round that would ask for the same size again.
    let mut sized_for: Option<(u32, u32)> = None;

    while window.is_open() && !keys::quit_pressed() {
        loop {
            match cmds.try_recv() {
                Ok(Command::Load(l)) => {
                    let round = l.round;
                    // Drop the old core before loading the next: the frontend
                    // statics are per core.
                    loaded = None;
                    match load(*l, &mut window) {
                        Ok(mut x) => {
                            place_window(hwnd, &x.0, &mut borderless, &mut sized_for);
                            let _ = reports.send(Report::Loaded { round, artifact: x.1 });
                            x.0.watchers = watchers;
                            x.0.want_key = watchers > 0;
                            x.0.runner.tap_audio(watchers > 0);
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
                Ok(Command::Forfeit) => {
                    if let Some(l) = loaded.as_mut() {
                        forfeit(l, &reports);
                    }
                }
                Ok(Command::Unload) => {
                    if let Some(l) = loaded.as_ref() {
                        l.runner.tap_audio(false);
                    }
                    loaded = None;
                }
                Ok(Command::Overlay(h)) => ours = vec![hwnd, h],
                Ok(Command::Stream(n)) => {
                    if let Some(l) = loaded.as_mut() {
                        if n > l.watchers {
                            l.want_key = true;
                        }
                        l.watchers = n;
                        l.runner.tap_audio(n > 0);
                    }
                    watchers = n;
                }
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

        // The two actions the client owns during play, as edges of whatever
        // the player bound them to. Polled every pass, so an edge is never
        // left over from before a pause.
        let in_front = keys::ours_in_front(&ours);
        let pad = match loaded.as_mut() {
            Some(l) => {
                l.pads.pump();
                l.pads.raw()
            }
            None => PadState::default(),
        };
        if let Some(l) = loaded.as_mut() {
            let dz = l.input.deadzone;
            let down = |i: &settings::Input| in_front && keys::held_or_tapped(i, &pad, dz);
            let give_up = give_up_edge.poll(&l.input.actions.give_up, down);
            let pause = pause_edge.poll(&l.input.actions.pause, down);
            if l.live && !l.done && !held {
                if give_up {
                    forfeit(l, &reports);
                } else if pause {
                    held = true;
                    let _ = reports.send(Report::Paused);
                }
            }
        }

        match loaded.as_mut() {
            None => {
                window.update();
                std::thread::sleep(Duration::from_millis(16));
            }
            Some(l) if held || !l.live || l.done => {
                // Present whatever is there, pump the window, wait a frame.
                if !l.live {
                    // The entry frame, if the core drew one on entering.
                    present(l, &mut window, &mut canvas, &streamer)?;
                } else {
                    window.update();
                }
                std::thread::sleep(l.frame_time);
            }
            Some(l) => {
                step(l, &mut window, &mut canvas, in_front, &pad, &reports, &streamer)?;
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
    let layout = match controller {
        Controller::Pad(layout) => layout,
        Controller::Keyboard => PadLayout::Snes,
    };
    let mut runner = LibretroRunner::start(manifest, None, content, Vec::new())?;
    let av = runner.av_info();
    let audio = match audio::start(av.timing.sample_rate) {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("[audio] disabled: {e}");
            None
        }
    };
    let fps = av.timing.fps.max(1.0);
    let frame_time = Duration::from_secs_f64(1.0 / fps);
    let audio_target = audio
        .as_ref()
        .map(|a| (audio::TARGET_QUEUED_FRAMES as f64 * a.device_rate / av.timing.fps) as usize);
    window.set_title(&format!("{title} \u{2014} GamerTrials"));
    runner.enter(&spec).map_err(|e| e.to_string())?;
    let timed = matches!(judged.kind, ChallengeKind::TimeTrial);
    let run = Run::new(judged).map_err(|e| e.to_string())?.with_limit(l.limit);
    // A keyboard game reads the pad too: its RetroPad buttons press keys.
    let pads = input::Pads::new(layout);
    let mouse = l
        .input
        .keyboard
        .as_ref()
        .map(|kb| input::Mouse::new().with_sensitivity(kb.mouse_sensitivity));
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
            input: l.input,
            video: l.video,
            display: l.display,
            core_aspect: av.geometry.aspect_ratio,
            _audio: audio,
            frame_time,
            fps,
            timed,
            clock_tenth: None,
            audio_target,
            next_deadline: Instant::now(),
            last_seq: 0,
            watchers: 0,
            want_key: false,
        },
        artifact,
    ))
}

/// One frame of play: pace, read input, advance, judge, present.
fn step(
    l: &mut Loaded,
    window: &mut Window,
    canvas: &mut Canvas,
    in_front: bool,
    pad: &PadState,
    reports: &Sender<Report>,
    streamer: &Streamer,
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

    let bits = if in_front { keys::joypad(&l.input, pad) } else { 0 };
    match l.input.keyboard.as_ref() {
        None => libretro::JOYPAD.store(bits, Ordering::Relaxed),
        Some(kb) => {
            if in_front {
                keys::keyboard_game(kb, bits, pad, l.input.deadzone);
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
    // The sound this frame made, for a spectator. Taken whenever the tap is
    // on so it never piles up; sent only while the run is theirs to watch.
    if l.watchers > 0 {
        let (round, send) = (l.round, l.streaming());
        l.runner.take_audio(&mut |samples, rate| {
            if send {
                streamer.sound(Sound { round, frame, rate, samples: samples.to_vec() });
            }
        });
    }
    let next = l.run.observe(frame, &events);
    if !events.is_empty() || next != Next::Continue {
        let _ = reports.send(Report::Frame { round: l.round, frame, events });
    }
    l.done = matches!(next, Next::Finished(_));
    // Before `Finished`, so the last reading reaches the overlay ahead of
    // the round card.
    clock(l, reports);
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
            let _ = reports.send(Report::Finished { round: l.round, result, forfeited_at: None });
        }
    }
    present(l, window, canvas, streamer)
}

/// Shown to the player only; the run is scored in frames, never in these.
fn clock(l: &mut Loaded, reports: &Sender<Report>) {
    if !l.timed {
        return;
    }
    let frames = l.run.frames();
    let tenth = (frames as f64 * 10.0 / l.fps) as u64;
    if l.done || l.clock_tenth != Some(tenth) {
        l.clock_tenth = Some(tenth);
        let _ = reports.send(Report::Clock { round: l.round, frames, fps: l.fps, stopped: l.done });
    }
}

fn forfeit(l: &mut Loaded, reports: &Sender<Report>) {
    if !l.live || l.done {
        return;
    }
    let frame = l.runner.frame();
    let result = l.run.forfeit();
    l.done = true;
    clock(l, reports);
    let _ = reports.send(Report::Finished { round: l.round, result, forfeited_at: Some(frame) });
}

fn present(l: &mut Loaded, window: &mut Window, canvas: &mut Canvas, streamer: &Streamer) -> Result<(), String> {
    let mut painted = false;
    let mut paint_error = None;
    let client = window.get_size();
    let (video, aspect) = (l.video, l.core_aspect);
    // For a spectator: the core's own picture, before the player's settings
    // shape it (ADR 0032). One copy here; the encoder runs on its own thread
    // and a picture it has no time for is simply not offered.
    let (streaming, want_key, round, frame) = (l.streaming(), l.want_key, l.round, l.runner.frame());
    let mut keyed = false;
    l.last_seq = l.runner.present(l.last_seq, &mut |f| {
        painted = true;
        if streaming {
            keyed |= streamer.offer(|| Picture {
                round,
                frame,
                width: f.width,
                height: f.height,
                aspect,
                pixels: f.pixels.to_vec(),
                key: want_key,
            });
        }
        let (buf, w, h) = canvas.draw(&f, client, aspect, &video);
        if let Err(e) = window.update_with_buffer(buf, w, h) {
            paint_error = Some(e.to_string());
        }
    });
    if keyed {
        l.want_key = false;
    }
    if let Some(e) = paint_error {
        return Err(format!("cannot present frame: {e}"));
    }
    if !painted {
        window.update();
    }
    Ok(())
}

/// Fullscreen or not, and in a window, the size this core's picture wants at
/// the player's scale.
fn place_window(hwnd: isize, l: &Loaded, borderless: &mut bool, sized_for: &mut Option<(u32, u32)>) {
    let want_borderless = l.display == Display::Borderless;
    if want_borderless != *borderless {
        video::window::set_borderless(hwnd, want_borderless);
        *borderless = want_borderless;
        *sized_for = None;
    }
    if *borderless {
        return;
    }
    let av = l.runner.av_info();
    let frame = (av.geometry.base_width as usize, av.geometry.base_height as usize);
    if frame.0 == 0 || frame.1 == 0 {
        return;
    }
    let size = video::window_size(frame, l.core_aspect, &l.video);
    if *sized_for != Some(size) {
        video::window::set_client_size(hwnd, size.0, size.1);
        *sized_for = Some(size);
    }
}

fn clear(window: &mut Window, width: usize, height: usize) -> Result<(), String> {
    window
        .update_with_buffer(&vec![0u32; width * height], width, height)
        .map_err(|e| format!("cannot clear window: {e}"))
}
