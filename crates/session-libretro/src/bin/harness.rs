//! The developer harness: run one manifest's game in a window and watch RAM.
//!
//! This is not the client. It is the tool a manifest is *authored* with — it
//! prints every address it is watching and where that address came from, lets
//! any of them be retargeted without a rebuild, and carries the two RAM
//! discovery tools for finding one nobody has written down. It judges nothing
//! itself: given `--challenge`, it hands what the runner reports to
//! `challenge-core`, prints the verdict that comes back, and re-enters a time
//! trial when told to. What a run was worth is never decided here.
//!
//! ```sh
//! cargo run --release -p session-libretro -- --game dkc --stage jungle-hijinx
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use minifb::{Key, Scale, ScaleMode, Window, WindowOptions};

use challenge_core::{Next, Run};
use session_api::{ChallengeSpec, Presentable, SessionEvent, StageRunner, StageSpec};
use session_libretro::libretro::{self, RETRO_MEMORY_IDS};
use session_libretro::manifest::{Challenge, Controller, Manifest};
use session_libretro::memory::{AddressSpace, Width};
use session_libretro::prep::{self, RomOverrides};
use session_libretro::ramsearch::{Filter, Probe, Search};
use session_libretro::runner::LibretroRunner;
use session_libretro::{audio, input};

/// Where a bare `--game dkc` looks.
const MANIFEST_DIR: &str = "manifests";

/// Candidates listed by one keypress. A search that has not narrowed yet holds
/// every address in RAM, and a console is not the place for 128 K lines.
const LIST_LIMIT: usize = 40;

const HELP: &str = "\
session-libretro harness - run a game from its manifest

USAGE:
    harness --game <NAME|PATH> [OPTIONS]

GAME:
    --game <NAME>    Manifest to run: a name (looked up as manifests/<NAME>.toml)
                     or a path to a .toml
    --stage <ID>     Stage to enter at startup. Default: the first one declared
    --challenge <ID> Challenge to run: enters its stage and judges the run with
                     challenge-core. A time trial re-enters on death; the
                     verdict is printed when the run ends. F2 starts a new run
    --limit <SECS>   End a --challenge run as timed out after this long, across
                     every attempt. Converted to frames at the game's rate, so
                     it is the same number of frames on every machine
    --list-stages    Print the manifest's stages and exit
    --list-challenges
                     Print every challenge and what completes it, and exit
    --list-addrs     Print the address table with its sources, and exit

OPTIONS:
    --core <PATH>    libretro core DLL. Default: the single *_libretro.dll in
                     the manifest's core_dir
    --rom <ID=PATH>  Run one of the manifest's [[rom]] entries from this file
                     instead of the declared one. The id is required: a manifest
                     may declare a base ROM and hacks of it, and a bare path no
                     longer says which one it means. Overriding the base means
                     the dump lives elsewhere; overriding a hack means the patch
                     was already applied by hand
    --seed <N>       Roll the challenge's patch slots from this seed and run the
                     ROM that comes out (decimal, or 0x hex). Only means anything
                     with --challenge, and only for a challenge that declares
                     patch slots. Without it a generated challenge runs unrolled,
                     which is what capturing its entry state wants
    --state <PATH>   Override the entry state, and the target for F1/F2
    --rebind-state   Let the entry state restore onto a ROM it was not captured
                     on, by rewriting the ROM hash in a copy of it (ADR 0019).
                     The file on disk is never written to. Not needed with
                     --seed: a roll is derived from the ROM the stage names, so
                     the pairing is already known to be sound and rebinding is
                     turned on for it. Pass this when you derived the ROM
                     yourself and handed it over with --rom. Without either, the
                     core refuses a state that names a different ROM - which is
                     what you want everywhere except a value-substituted one
    --scale <1-4>    Window scale. Default: 3
    --watch <ADDRS>  Comma-separated addresses to report on change, e.g.
                     --watch 0x7E0577,0x057B. An entry may be a range lo-hi,
                     e.g. --watch 0x0570-0x05a0
    --addr <N=A[:W]> Retarget a watched quantity without rebuilding, e.g.
                     --addr lives=0x7E08A6 --addr in_stage=0x0527:u8
                     W is u8, u16 or u32. --addr <name>=off turns it off
    --dump-memory-map
                     Print every memory region this core publishes - the four
                     retro_get_memory_data ids and every SET_MEMORY_MAPS
                     descriptor - and exit. Run it once per core: what a core
                     offers beyond system RAM is the difference between a
                     detector being possible and not
    --dump-options   Print every core option this core publishes, its legal
                     values and its default, plus any environment call the core
                     asked for and did not get - and exit. Run it once per core:
                     an option left at its default is a machine configured
                     differently from everyone else\'s
    --search-width <u8|u16|u32>
                     Width the F3-F8 RAM search compares at. Default: u16.
                     A DOS game wants u32: its globals are C ints
    --probe-recent <N>
                     F11 reports bytes that changed within N frames. Default: 20
    --probe-stable <N>
                     ...and that held still for N frames before that. Default: 180
    --no-probe       Skip the per-frame change history (disables F11)
    --frames <N>     Run N frames headless - no window, no sound, no pacing -
                     and print what fired. The frame numbers are the same as
                     they would be on screen
    --capture <PATH> Where a captured state goes. With --frames, write one when
                     the run ends and exit - with a stage whose entry is
                     `named`, that is the whole authoring loop in one command:
                     boot the game to where it starts itself, then capture the
                     state a `state` entry restores. In the window, F1 writes
                     here instead of back over the state the stage was entered
                     from. Pass it whenever you are authoring a new entry point
                     rather than re-taking the one you started at
    --no-audio       Run without sound, paced off the wall clock instead
    -h, --help       This text

KEYS (pad games - SMB3, DKC):
    Arrows  d-pad
    Z  B      X  A      A  Y      S  X      Q  L      W  R
    Enter  Start        RShift  Select
    Esc  quit

KEYS (keyboard games - DOOM):
    Every key goes straight to the game, Esc included, because the game maps
    its own. Close the window to quit; F1-F11 below still belong to the harness.
    The mouse goes to the game too: click the window to capture it (that click
    is not passed on), Alt-Tab to release it.

KEYS (both):
    F1  save state      F2  re-enter stage  F5  dump watched RAM
    F9  reset core      F12 quit

RAM SEARCH (find a value that persists - lives, bananas, a counter).
Works over the system-RAM slice, or over [runner.libretro.memory.scan] for a
core that publishes no slice at all:
    F3  snapshot / restart      F4  keep unchanged   F6  keep changed
    F7  keep increased          F8  keep decreased   F10 list candidates

RAM PROBE (find a one-shot event - a hit, a pickup, a stage clear):
    F11 report what changed just now
";

struct Args {
    game: Option<String>,
    stage: Option<String>,
    challenge: Option<String>,
    core: Option<PathBuf>,
    roms: RomOverrides,
    seed: Option<u64>,
    limit: Option<f64>,
    state: Option<PathBuf>,
    rebind_state: bool,
    scale: usize,
    watch: Vec<usize>,
    addrs: Vec<String>,
    list_addrs: bool,
    list_stages: bool,
    list_challenges: bool,
    dump_memory_map: bool,
    dump_options: bool,
    search_width: Width,
    probe_recent: u64,
    probe_stable: u64,
    probe: bool,
    audio: bool,
    frames: Option<u64>,
    capture: Option<PathBuf>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args = match parse_args()? {
        Some(a) => a,
        None => {
            print!("{HELP}");
            return Ok(());
        }
    };

    let game = args.game.as_deref().ok_or("--game is required; try --help")?;
    let path = manifest_path(game);
    let mut manifest = Manifest::load(&path)?;
    println!("[game] {} ({}, {})", manifest.title, manifest.console, path.display());

    for spec in &args.addrs {
        apply_addr_override(spec, &mut manifest)?;
    }

    if args.list_stages {
        print_stages(&manifest);
        return Ok(());
    }
    if args.list_challenges {
        print_challenges(&manifest);
        return Ok(());
    }
    if args.list_addrs {
        print_addrs(&manifest);
        return Ok(());
    }

    if args.stage.is_some() && args.challenge.is_some() {
        return Err("--stage and --challenge both choose where to start; pass one".into());
    }

    // The entry point, chosen before the core is loaded so a bad --stage costs
    // nothing.
    //
    // A challenge is resolved to its stage here and judged by `challenge-core`
    // below. Nothing in this crate decides a run was won: an adapter that did
    // would be the exact mistake ADR 0017 exists to prevent.
    let (stage, challenge): (_, Option<Challenge>) = match (&args.stage, &args.challenge) {
        (Some(id), _) => (
            manifest
                .stage(id)
                .ok_or_else(|| format!("{game} declares no stage `{id}`; try --list-stages"))?
                .clone(),
            None,
        ),
        (None, Some(id)) => {
            let c = manifest
                .challenge(id)
                .ok_or_else(|| {
                    format!("{game} declares no challenge `{id}`; try --list-challenges")
                })?
                .clone();
            println!(
                "[challenge] {} ({}) — completed by: {}",
                c.id.0,
                c.kind,
                describe(&c.complete_when)
            );
            if !c.ends_attempt_on.is_empty() {
                let facts: Vec<&str> = c.ends_attempt_on.iter().map(String::as_str).collect();
                println!("[challenge] an attempt also ends on: {}", facts.join(", "));
            }
            let stage =
                manifest.stage(&c.stage.0).expect("validation proved the stage exists").clone();
            (stage, Some(c))
        }
        (None, None) => (
            manifest
                .stages
                .first()
                .ok_or_else(|| format!("{game} declares no stages at all"))?
                .clone(),
            None,
        ),
    };

    // Everything between what a manifest declares and bytes a core can run: the
    // player's own dump, a contributor's hack applied to it, and a seeded roll
    // of whatever the challenge declares (ADR 0013, ADR 0015, ADR 0020).
    let content = prep::content_for(&manifest, &stage, challenge.as_ref(), args.seed, &args.roms)?;
    let mut spec = manifest.stage_spec(&stage, content.key.clone());
    if let Some(p) = &args.state {
        spec.entry = session_api::EntryPoint::State(session_api::AssetKey(
            p.to_string_lossy().into_owned(),
        ));
    }
    let state_path = match &spec.entry {
        session_api::EntryPoint::State(k) => PathBuf::from(&k.0),
        _ => PathBuf::from("state.bin"),
    };

    if args.limit.is_some() && challenge.is_none() {
        return Err("--limit bounds a run, and only --challenge starts one".into());
    }
    // What `challenge-core` reads: the challenge in shared vocabulary, and the
    // limit already in frames. Taken before the runner owns the manifest.
    let judged = challenge.as_ref().map(|c| {
        let limit = args.limit.map(|s| manifest.framerate.frames(s));
        if let Some(l) = limit {
            println!("[run] limited to {l} frames across every attempt");
        }
        (manifest.challenge_spec(c), limit)
    });
    let mut run = judged.as_ref().map(start_run).transpose()?;

    print_addrs(&manifest);
    let space = manifest.libretro.space;
    let controller = manifest.libretro.controller;
    let mut runner =
        LibretroRunner::start(manifest, args.core.clone(), content, args.watch.clone())?;
    // Stage prep already turns rebinding on for the one pairing ADR 0019
    // sanctions. The flag stays for the case it was written for: a ROM derived
    // outside the harness and passed in with --rom.
    if args.rebind_state {
        runner.set_state_rebinding(true);
    }

    if args.dump_memory_map {
        print_memory_map(&runner);
        return Ok(());
    }
    if args.dump_options {
        print_options();
        return Ok(());
    }

    // How much of a declared scan window no route reaches. Said once, because a
    // window aimed at unmapped space looks exactly like a search that keeps
    // finding zeros.
    if let Some((missing, size)) = runner.scan_window_gap() {
        if missing > 0 {
            eprintln!(
                "[scan] {missing} of {size} bytes in the scan window are unmapped and \
                 read as zero"
            );
        }
    }

    let av = runner.av_info();
    let (width, height) =
        (av.geometry.base_width as usize, av.geometry.base_height as usize);

    // Headless: enter the stage, run a fixed number of frames as fast as the
    // core will go, and print what fired. No window, no sound, no pacing — the
    // frame numbers are identical either way, which is the point of counting
    // emulated frames rather than time (ADR 0005). This is how a manifest's
    // rules get exercised against a real ROM in one command.
    if let Some(n) = args.frames {
        runner.enter(&spec).map_err(|e| e.to_string())?;
        if let Some(r) = run.as_mut() {
            r.enter();
        }
        for _ in 0..n {
            if runner.shutdown_requested() {
                println!("[core] the game asked to shut down at frame {}", runner.frame());
                break;
            }
            let events = runner.advance().map_err(|e| e.to_string())?.to_vec();
            for event in &events {
                if let SessionEvent::StageCleared { .. } | SessionEvent::StageExited { .. } = event
                {
                    println!("{event}");
                }
            }
            if let Some(r) = run.as_mut() {
                if judge(r, &mut runner, &spec, &events)? == Judged::Finished {
                    break;
                }
            }
        }
        if let Some(r) = run.as_ref().filter(|r| r.result().is_none()) {
            println!(
                "[run] still going when --frames ran out: {} frames over {} attempt(s)",
                r.frames(),
                r.attempts()
            );
        }
        println!("[done] {} frames", runner.frame());
        if let Some(path) = &args.capture {
            write_state(&runner, path)?;
        }
        return Ok(());
    }
    // Windowed, `--capture` retargets F1. Without it F1 writes back over the
    // state the stage was entered from, which is fine while authoring that
    // state and destroys a shipped asset the moment you are authoring a
    // different one from it. Authoring a *new* entry point is the whole reason
    // to be here, so it gets somewhere of its own to go (ADR 0019 and
    // `docs/games/smb3.md`: capture on the base ROM).
    let state_path = match &args.capture {
        Some(p) => {
            println!("[state] F1 writes to {} - the entry state is read-only", p.display());
            p.clone()
        }
        None => state_path,
    };

    // Audio doubles as the frame clock, so set it up before the loop.
    let audio = if args.audio {
        match audio::start(av.timing.sample_rate) {
            Ok(a) => {
                println!("[audio] {} Hz, {} channels", a.device_rate, a.channels);
                Some(a)
            }
            Err(e) => {
                eprintln!("[audio] disabled: {e}");
                None
            }
        }
    } else {
        None
    };

    let mut window = Window::new(
        &format!("{} harness", runner.manifest().title),
        width.max(1),
        height.max(1),
        WindowOptions {
            scale: match args.scale {
                2 => Scale::X2,
                4 => Scale::X4,
                _ => Scale::X1,
            },
            scale_mode: ScaleMode::AspectRatioStretch,
            resize: true,
            ..WindowOptions::default()
        },
    )
    .map_err(|e| format!("cannot open window: {e}"))?;
    // minifb defaults to 250 fps and sleeps out the remainder of 4 ms inside
    // every `update`, which is its own idea of pacing on top of ours. This loop
    // is paced by the audio queue (or by the wall clock without one), so that
    // sleep is dead time between the last key we read and the frame it drives.
    // 0 is minifb's spelling of "do no waiting".
    window.set_target_fps(0);

    let mut pads = match controller {
        Controller::Pad(layout) => Some(input::Pads::new(layout)),
        // A DOS game has no pad to map. Skipping gilrs here is not only tidy:
        // it is what stops a connected controller quietly pressing keys.
        Controller::Keyboard => None,
    };
    // ...and a PC has a mouse beside its keyboard. A console has neither, and
    // capturing the cursor for a game that cannot read it would only be in
    // the way.
    let mut mouse = matches!(controller, Controller::Keyboard).then(|| {
        println!("[input] click the window to give the game the mouse; Alt-Tab takes it back");
        input::Mouse::new()
    });
    let mut search = Search::new(args.search_width);
    let mut probe = args.probe.then(Probe::new);
    let mut hotkeys = Hotkeys::new();

    runner.enter(&spec).map_err(|e| e.to_string())?;
    if let Some(r) = run.as_mut() {
        r.enter();
    }
    if let Some(p) = probe.as_mut() {
        p.reset();
    }

    let frame_time = Duration::from_secs_f64(1.0 / av.timing.fps.max(1.0));
    let audio_target = audio
        .as_ref()
        .map(|a| (audio::TARGET_QUEUED_FRAMES as f64 * a.device_rate / av.timing.fps) as usize);
    let mut next_deadline = Instant::now();
    let mut last_seq = 0u64;

    // Escape quits a pad game and belongs to the game in a keyboard one, where
    // it is how Doom opens its menu. F12 quits either.
    let quits_on_escape = matches!(controller, Controller::Pad(_));
    while window.is_open()
        && !window.is_key_down(Key::F12)
        && !(quits_on_escape && window.is_key_down(Key::Escape))
    {
        if runner.shutdown_requested() {
            println!("[core] the game asked to shut down; stopping");
            break;
        }

        // Wait for this frame's turn *before* reading anything, so that the
        // keys below are the ones held when the frame runs rather than the
        // ones held when the previous frame finished. The order used to be the
        // other way round, which put the whole of this wait between a
        // keypress and the frame it reached.
        //
        // Pace on the audio queue when we have one: it follows the sound card
        // clock, which is what playback actually drains at. Wall-clock pacing
        // drifts against it and underruns every few seconds.
        match audio_target {
            Some(target) => {
                // Bounded: if the audio device stops draining (unplugged, or
                // the stream died), fall through and keep running rather than
                // spinning here forever.
                let give_up_at = Instant::now() + frame_time * 4;
                while audio::queued_frames() > target && Instant::now() < give_up_at {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            None => {
                let now = Instant::now();
                if next_deadline > now {
                    std::thread::sleep(next_deadline - now);
                } else {
                    // Behind schedule; give up the lost time rather than
                    // sprinting to catch up.
                    next_deadline = now;
                }
                next_deadline += frame_time;
            }
        }

        // Waiting does not deliver keystrokes; pumping the window is what
        // does. Without this the keys read below would still be whatever the
        // presenting `update` at the bottom of the previous iteration saw, and
        // moving the wait above would have bought nothing.
        window.update();

        for key in hotkeys.pressed(&window) {
            match key {
                Key::F1 => {
                    if let Err(e) = write_state(&runner, &state_path) {
                        eprintln!("[state] {e}");
                    }
                }
                Key::F2 => {
                    if let Err(e) = runner.enter(&spec) {
                        eprintln!("[state] {e}");
                    }
                    // Neither kind allows a manual re-entry mid-run, so under
                    // a challenge F2 starts a fresh run rather than an attempt
                    // the judge would have to explain.
                    if let Some(j) = judged.as_ref() {
                        if let Some(old) = run.as_mut().filter(|r| r.result().is_none()) {
                            println!("[run] abandoned: {}", old.forfeit());
                        }
                        let mut fresh = start_run(j)?;
                        fresh.enter();
                        run = Some(fresh);
                    }
                    if let Some(p) = probe.as_mut() {
                        // RAM has just been replaced wholesale. Without
                        // re-arming, every byte that differs from before the
                        // load looks like it changed this instant.
                        p.reset();
                    }
                }
                Key::F5 => runner.dump_watches(),
                Key::F9 => {
                    runner.reset();
                    if let Some(p) = probe.as_mut() {
                        p.reset();
                    }
                    println!("[core] reset; frame counter restarted");
                }
                Key::F3 => {
                    if let Some((_, ram)) = runner.scan_window() {
                        search.snapshot(ram);
                    }
                }
                Key::F4 | Key::F6 | Key::F7 | Key::F8 => {
                    let f = match key {
                        Key::F4 => Filter::Unchanged,
                        Key::F6 => Filter::Changed,
                        Key::F7 => Filter::Increased,
                        _ => Filter::Decreased,
                    };
                    if let Some((_, ram)) = runner.scan_window() {
                        search.filter(ram, f);
                    }
                }
                Key::F10 => {
                    if let Some((base, ram)) = runner.scan_window() {
                        search.list(&window_space(&space, base, ram.len()), ram, LIST_LIMIT);
                    }
                }
                Key::F11 => {
                    let frame = runner.frame();
                    if probe.is_none() {
                        println!("[probe] disabled by --no-probe");
                    } else if let Some((base, ram)) = runner.scan_window() {
                        if let Some(p) = probe.as_ref() {
                            p.report(
                                &window_space(&space, base, ram.len()),
                                ram,
                                frame,
                                args.probe_recent,
                                args.probe_stable,
                                LIST_LIMIT,
                            );
                        }
                    }
                }
                _ => {}
            }
        }

        match pads.as_mut() {
            Some(p) => {
                p.pump();
                libretro::JOYPAD.store(input::keyboard(&window) | p.state(), Ordering::Relaxed);
            }
            None => input::keyboard_passthrough(&window),
        }
        if let Some(m) = mouse.as_mut() {
            m.pump(&mut window);
        }

        if probe.is_some() {
            let frame = runner.frame() + 1;
            if let Some((_, ram)) = runner.scan_window() {
                // `scan_window` borrows the runner mutably to fill its buffer,
                // so the probe is reached through a raw split rather than held
                // across the call.
                if let Some(p) = probe.as_mut() {
                    p.tick(ram, frame);
                }
            }
        }

        let events = runner.advance().map_err(|e| e.to_string())?.to_vec();
        for event in &events {
            // The watcher has already printed a line for each of these; what is
            // worth repeating is how the stage ended, because it is the one
            // thing a playtester is looking for in the scroll.
            match event {
                SessionEvent::StageCleared { frame } => println!("STAGE CLEARED at frame {frame}"),
                SessionEvent::StageExited { frame, reason } => {
                    println!("STAGE EXITED at frame {frame} ({reason})")
                }
                _ => {}
            }
        }
        // A finished run stays on screen, playable, until F2 starts another.
        if let Some(r) = run.as_mut().filter(|r| r.result().is_none()) {
            match judge(r, &mut runner, &spec, &events)? {
                Judged::Reentered => {
                    if let Some(p) = probe.as_mut() {
                        p.reset();
                    }
                }
                Judged::Finished => println!("[run] F2 starts another run"),
                Judged::Continue => {}
            }
        }

        let mut painted = false;
        let mut paint_error = None;
        last_seq = runner.present(last_seq, &mut |f| {
            painted = true;
            if let Err(e) = window.update_with_buffer(f.pixels, f.width, f.height) {
                paint_error = Some(e.to_string());
            }
        });
        if let Some(e) = paint_error {
            return Err(format!("cannot present frame: {e}"));
        }
        // A duped frame still needs an update() to pump window messages, but
        // repainting an identical buffer is wasted work.
        if !painted {
            window.update();
        }
    }

    Ok(())
}

/// The harness's own keys — the ones [`input::keyboard_passthrough`] withholds
/// from the game — and whether each was pressed since the last look.
///
/// minifb has `get_keys_pressed` for this, but it derives its edges from
/// whenever the window was last pumped, and this loop pumps twice per frame:
/// once after pacing, so the keys driving a frame are current, and once to
/// present. An edge landing between the two would be erased by the next pump
/// before anything read it, which is a save-state keypress silently doing
/// nothing every other frame. Tracking held-state ourselves is independent of
/// how often the window is pumped.
struct Hotkeys {
    held: [bool; HOTKEYS.len()],
}

/// F12 is not here: it quits, and the loop condition tests it as a level.
const HOTKEYS: [Key; 11] = [
    Key::F1,
    Key::F2,
    Key::F3,
    Key::F4,
    Key::F5,
    Key::F6,
    Key::F7,
    Key::F8,
    Key::F9,
    Key::F10,
    Key::F11,
];

impl Hotkeys {
    fn new() -> Hotkeys {
        Hotkeys { held: [false; HOTKEYS.len()] }
    }

    fn pressed(&mut self, window: &Window) -> Vec<Key> {
        let mut edges = Vec::new();
        for (key, held) in HOTKEYS.iter().zip(self.held.iter_mut()) {
            let down = window.is_key_down(*key);
            if down && !*held {
                edges.push(*key);
            }
            *held = down;
        }
        edges
    }
}

/// A fresh run of the `--challenge`, limited if `--limit` was given.
fn start_run((spec, limit): &(ChallengeSpec, Option<u64>)) -> Result<Run, String> {
    let run = Run::new(spec.clone()).map_err(|e| e.to_string())?;
    Ok(match limit {
        Some(frames) => run.with_limit(*frames),
        None => run,
    })
}

#[derive(PartialEq)]
enum Judged {
    Continue,
    Reentered,
    Finished,
}

/// Hands one frame to `challenge-core` and does what it says. The harness
/// carries out the verdict and never forms one.
fn judge(
    run: &mut Run,
    runner: &mut LibretroRunner,
    spec: &StageSpec,
    events: &[SessionEvent],
) -> Result<Judged, String> {
    let frame = runner.frame();
    match run.observe(frame, events) {
        Next::Continue => Ok(Judged::Continue),
        Next::Reenter => {
            println!(
                "[run] attempt {} ended at frame {frame}; re-entering with {} frames on the clock",
                run.attempts(),
                run.frames()
            );
            runner.enter(spec).map_err(|e| e.to_string())?;
            run.enter();
            Ok(Judged::Reentered)
        }
        Next::Finished(result) => {
            println!("[run] {result}");
            Ok(Judged::Finished)
        }
    }
}

/// `--game dkc` and `--game manifests/dkc.toml` both work; a name without a
/// separator or an extension is looked up in `manifests/`.
fn manifest_path(game: &str) -> PathBuf {
    let p = Path::new(game);
    if p.extension().is_some() || game.contains('/') || game.contains('\\') {
        p.to_path_buf()
    } else {
        Path::new(MANIFEST_DIR).join(format!("{game}.toml"))
    }
}

/// The address table as it will actually be used, with where each default came
/// from. Printed at startup because a tool whose whole job is checking
/// unverified addresses should say which ones it is checking.
fn print_addrs(m: &Manifest) {
    println!("[addr] watched quantities (override with --addr name=ADDR[:u8|u16|u32]):");
    for w in &m.watches {
        let shown = match w.at {
            Some(a) => format!("{} {}", m.libretro.space.show(a), w.width),
            None => "unset".to_string(),
        };
        println!("        {:<14} {shown:<18} {}", w.name, w.note);
    }
}

fn print_stages(m: &Manifest) {
    println!("[stage] content declared by {}:", m.id);
    for r in &m.roms {
        let how = match (&r.from, &r.patch) {
            (Some(from), Some(patch)) => format!("{} + {}", from, patch.display()),
            _ => r
                .path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "no path".to_string()),
        };
        println!(
            "        {:<12} {:<38} {how}",
            r.id,
            r.name.as_deref().unwrap_or("")
        );
    }
    println!("[stage] stages declared by {}:", m.id);
    for s in &m.stages {
        let n = m.challenges_on(&s.id).count();
        let carries = match n {
            0 => "no challenges — nothing can be dealt on it".to_string(),
            1 => "1 challenge".to_string(),
            n => format!("{n} challenges"),
        };
        println!("        {:<22} {:<26} on {:<10} {carries}", s.id.0, s.name, s.rom);
        if let Some(note) = &s.note {
            println!("        {:<22} {note}", "");
        }
    }
}

/// Every challenge the game offers, and what completes each. The harness scores
/// none of them — it prints them so a contributor can read back what the file
/// they just wrote actually says.
fn print_challenges(m: &Manifest) {
    println!("[challenge] challenges declared by {}:", m.id);
    for c in &m.challenges {
        println!(
            "        {:<34} {:<11} on {:<22} {}",
            c.id.0,
            c.kind.to_string(),
            c.stage.0,
            describe(&c.complete_when)
        );
        if !c.ends_attempt_on.is_empty() {
            // Worth its own line: it changes what the run costs, not what
            // completes it (ADR 0025).
            let facts: Vec<&str> = c.ends_attempt_on.iter().map(String::as_str).collect();
            println!("        {:<34} an attempt ends on: {}", "", facts.join(", "));
        }
        if let Some(t) = m.patch_table(&c.id) {
            // A generated challenge is a challenge that additionally says how
            // its ROM is derived (ADR 0015). Worth printing: it is the
            // difference between playing a level and playing a roll of one.
            println!(
                "        {:<34} generated: {} slots, generator v{} — needs --seed",
                "",
                t.slots.len(),
                t.generator_version
            );
        }
        if let Some(note) = &c.note {
            println!("        {:<34} {note}", "");
        }
    }
}

/// `complete_when` as one line. Every criterion listed has to hold, so they are
/// joined with "and" rather than commas — the conjunction is the whole meaning.
fn describe(c: &session_api::Completion) -> String {
    let mut parts: Vec<String> = Vec::new();
    if c.cleared {
        parts.push("cleared".to_string());
    }
    if let Some(f) = c.survive_frames {
        parts.push(format!("survive {f} frames"));
    }
    for (name, n) in &c.counts {
        parts.push(match name.as_str() {
            session_api::ENEMY_DEFEATED => format!("defeat {n} enemies"),
            session_api::DAMAGED => format!("take {n} hits"),
            _ => format!("{n} x {name}"),
        });
    }
    for o in &c.objectives {
        parts.push(format!("reach {o}"));
    }
    parts.join(" and ")
}

/// What a region of this size is likely to be. A guess from the size alone —
/// enough to read a dump without a reference table open, not enough to point a
/// detector at. Confirm anything load-bearing against the core's source or the
/// `addrspace` name it published.
fn region_shape(len: usize) -> Option<&'static str> {
    match len {
        0x20000 => Some("SNES work RAM"),
        // Both are 64 KB, and only the VIDEO_RAM flag or the addrspace name
        // tells them apart. APU RAM is the one worth having: the sound
        // driver's state lives there.
        0x10000 => Some("VRAM or APU RAM"),
        0x800 => Some("NES internal RAM, or a small cartridge SRAM"),
        544 => Some("OAM — the sprite table"),
        512 => Some("CGRAM, or OAM's low table"),
        32 => Some("OAM's high table"),
        0x2000 | 0x8000 => Some("cartridge SRAM"),
        _ => None,
    }
}

fn shape_suffix(len: usize) -> String {
    match region_shape(len) {
        Some(what) => format!("  — looks like {what}"),
        None => String::new(),
    }
}

/// Everything this core is willing to hand out, both routes. The point of it is
/// what lies *outside* system RAM: a core that publishes OAM makes enemy kills
/// and damage a sprite-table read rather than a hunt for a counter that may not
/// exist, and one that publishes APU RAM makes "which music is playing" a plain
/// RAM watch.
/// Capture the machine's whole state and put it on disk.
///
/// The other half of a `named` entry point: a manifest names a place the game
/// can reach by itself, the harness boots there, and this is what turns that
/// into the `state` entry a session actually uses (ADR 0002).
fn write_state(runner: &LibretroRunner, path: &Path) -> Result<(), String> {
    let data = runner.capture_state()?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)
            .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    fs::write(path, &data).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    println!("[state] saved {} bytes to {}", data.len(), path.display());
    Ok(())
}

/// The address space the discovery tools print through.
///
/// They work in offsets into whatever slice they were handed, and print each
/// one as `space.show(off + space.base)`. For a console that slice is system
/// RAM and the manifest's own space already describes it. For a scan window it
/// is a slice of somewhere else entirely, so the base and size are the window's
/// — otherwise a hit at window offset `0x9E98` would print as `$009E98` and
/// paste back into `--addr` as an address that means nothing.
fn window_space(space: &AddressSpace, base: usize, len: usize) -> AddressSpace {
    AddressSpace { base, size: len, ..*space }
}

/// Every option this core publishes, and every environment call it asked for
/// and did not get.
///
/// Both halves are for the same job: finding out what a core actually wants
/// before writing a manifest against it. An option left at its default is a
/// machine configured differently from the next player's, and an unanswered
/// environment call is the core silently doing something else — which is the
/// failure that costs a day rather than a minute.
fn print_options() {
    let defs = libretro::option_defs();
    println!();
    if defs.is_empty() {
        println!("core options: none published.");
    } else {
        println!("core options ({}):", defs.len());
        for d in &defs {
            println!("  {}", d.key);
            if !d.desc.is_empty() {
                println!("      {}", d.desc);
            }
            println!("      default: {}", d.default_value);
            if !d.values.is_empty() {
                println!("      values:  {}", d.values.join(" | "));
            }
        }
    }

    let unhandled = libretro::unhandled_env();
    println!();
    if unhandled.is_empty() {
        println!("unanswered environment calls: none.");
    } else {
        println!("unanswered environment calls ({}):", unhandled.len());
        println!("    cmd  asked  (RETRO_ENVIRONMENT_* number; EXPERIMENTAL is | 0x10000)");
        for (cmd, count) in &unhandled {
            println!("  {cmd:>5}  {count:>5}");
        }
        println!();
        println!("Each of these is a capability this frontend does not offer. That is");
        println!("often fine — but a core that asked and was refused has worked around");
        println!("it somehow, and it is worth knowing how before trusting its frames.");
    }
}

fn print_memory_map(runner: &LibretroRunner) {
    println!();
    println!("retro_get_memory_data ids:");
    for (id, name) in RETRO_MEMORY_IDS {
        match runner.memory(id) {
            Some(m) => {
                println!("  {id}  {name:<10} {:>7} bytes{}", m.len(), shape_suffix(m.len()))
            }
            None => println!("  {id}  {name:<10}       — not published"),
        }
    }

    let map = libretro::memory_map();
    println!();
    if map.is_empty() {
        println!("SET_MEMORY_MAPS: no descriptors published.");
        println!();
        println!("Nothing beyond the ids above is reachable from this core.");
        return;
    }

    println!("SET_MEMORY_MAPS descriptors ({}):", map.len());
    println!("    #  start       length      read  flags / addrspace");
    for (i, r) in map.iter().enumerate() {
        let mut tags: Vec<String> = Vec::new();
        let flags = r.flag_names();
        if !flags.is_empty() {
            tags.push(flags);
        }
        if let Some(space) = &r.addrspace {
            tags.push(format!("addrspace={space:?}"));
        }
        println!(
            "  {i:>3}  0x{:08X}  0x{:08X}  {:<4}  {}",
            r.start,
            r.len,
            if r.readable() { "yes" } else { "no" },
            tags.join(" ")
        );
        // The fields that explain a `no`, and the offset, which silently
        // shifts where a descriptor's bytes actually start. Printed only when
        // they carry information.
        if r.select != 0 || r.disconnect != 0 || r.offset != 0 || r.ptr == 0 {
            let mut why: Vec<String> = Vec::new();
            if r.ptr == 0 {
                why.push("no backing buffer".into());
            }
            if r.select != 0 {
                why.push(format!("select=0x{:X}", r.select));
            }
            if r.disconnect != 0 {
                why.push(format!("disconnect=0x{:X}", r.disconnect));
            }
            if r.offset != 0 {
                why.push(format!("offset=0x{:X}", r.offset));
            }
            println!("       {}", why.join(", "));
        }
        if let Some(what) = region_shape(r.len) {
            println!("       looks like {what}");
        }
    }

    // The summary is the part worth reading. `read_addr` resolves a plain
    // start..start+len window only, so a bit-matched descriptor is published
    // but not reachable — see the note on mirroring in `libretro::read_addr`.
    let system_ram = runner.system_ram().map(|m| m.len()).unwrap_or(0);
    let bit_matched = map
        .iter()
        .filter(|r| r.ptr != 0 && (r.select != 0 || r.disconnect != 0))
        .count();
    let beyond: Vec<&libretro::MappedRegion> =
        map.iter().filter(|r| r.readable() && r.len != system_ram).collect();

    println!();
    if beyond.is_empty() {
        println!("Readable beyond system RAM: nothing.");
    } else {
        println!("Readable beyond system RAM:");
        for r in beyond {
            println!("  0x{:08X}  {:>7} bytes{}", r.start, r.len, shape_suffix(r.len));
        }
    }
    if bit_matched > 0 {
        println!("{bit_matched} descriptor(s) use select/disconnect bit-matching, which");
        println!("read_addr does not resolve: published by the core, unreachable here.");
    }
}

/// Parses one hex address, with or without an `0x` or `$` prefix.
fn parse_addr(s: &str) -> Result<usize, String> {
    let hex = s.trim().trim_start_matches("0x").trim_start_matches('$');
    usize::from_str_radix(hex, 16).map_err(|_| format!("wanted a hex address, got `{s}`"))
}

/// `name=ADDR[:u8|u16|u32]`, or `name=off` to turn a detector off.
fn apply_addr_override(spec: &str, m: &mut Manifest) -> Result<(), String> {
    let (name, rest) = spec
        .split_once('=')
        .ok_or_else(|| format!("--addr wants name=ADDR, got `{spec}`"))?;
    let (name, rest) = (name.trim(), rest.trim());
    if rest.eq_ignore_ascii_case("off") || rest.eq_ignore_ascii_case("none") {
        return m.set_addr(name, None, None);
    }
    let (addr, width) = match rest.split_once(':') {
        Some((a, w)) => (parse_addr(a)?, Some(Width::parse(w.trim())?)),
        None => (parse_addr(rest)?, None),
    };
    m.set_addr(name, Some(addr), width)
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        game: None,
        stage: None,
        challenge: None,
        core: None,
        roms: RomOverrides::new(),
        seed: None,
        limit: None,
        state: None,
        rebind_state: false,
        scale: 3,
        watch: Vec::new(),
        addrs: Vec::new(),
        list_addrs: false,
        list_stages: false,
        list_challenges: false,
        dump_memory_map: false,
        dump_options: false,
        search_width: Width::U16,
        // ~a third of a second either side of the keypress: long enough to
        // cover a human's reaction time, short enough that the frame the game
        // wrote is still nearby.
        probe_recent: 20,
        // Three seconds of holding still. Anything busier than that is a
        // timer, a position, or the RNG, and is not what a hit wrote.
        probe_stable: 180,
        probe: true,
        audio: true,
        frames: None,
        capture: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value =
            |flag: &str| -> Result<String, String> { it.next().ok_or_else(|| format!("{flag} needs a value")) };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--game" => args.game = Some(value("--game")?),
            "--stage" => args.stage = Some(value("--stage")?),
            "--challenge" => args.challenge = Some(value("--challenge")?),
            "--core" => args.core = Some(PathBuf::from(value("--core")?)),
            "--rom" => {
                // ADR 0020 made a bare path ambiguous: a manifest declares more
                // than one ROM now, so an override says which one it replaces.
                let v = value("--rom")?;
                let (id, path) = v.split_once('=').ok_or_else(|| {
                    format!(
                        "--rom takes <ID>=<PATH>, e.g. --rom base=my-dump.nes; got `{v}`. \
                         The id names a [[rom]] this manifest declares"
                    )
                })?;
                args.roms.insert(id.to_string(), PathBuf::from(path));
            }
            "--seed" => {
                let v = value("--seed")?;
                let parsed = match v.strip_prefix("0x") {
                    Some(hex) => u64::from_str_radix(hex, 16),
                    None => v.parse(),
                };
                args.seed =
                    Some(parsed.map_err(|_| format!("--seed wants a number, got {v}"))?);
            }
            "--limit" => {
                let v = value("--limit")?;
                let secs: f64 =
                    v.parse().map_err(|_| format!("--limit wants seconds, got {v}"))?;
                if !(secs > 0.0) {
                    return Err(format!("--limit must be more than zero seconds, got {v}"));
                }
                args.limit = Some(secs);
            }
            "--state" => args.state = Some(PathBuf::from(value("--state")?)),
            "--rebind-state" => args.rebind_state = true,
            "--no-audio" => args.audio = false,
            "--frames" => {
                let v = value("--frames")?;
                args.frames =
                    Some(v.parse().map_err(|_| format!("--frames wants a count, got {v}"))?);
            }
            "--capture" => args.capture = Some(PathBuf::from(value("--capture")?)),
            "--no-probe" => args.probe = false,
            "--list-addrs" => args.list_addrs = true,
            "--list-stages" => args.list_stages = true,
            "--list-challenges" => args.list_challenges = true,
            "--dump-memory-map" => args.dump_memory_map = true,
            "--dump-options" => args.dump_options = true,
            "--addr" => args.addrs.push(value("--addr")?),
            "--search-width" => args.search_width = Width::parse(value("--search-width")?.trim())?,
            "--probe-recent" => {
                let v = value("--probe-recent")?;
                args.probe_recent =
                    v.parse().map_err(|_| format!("--probe-recent wants a frame count, got {v}"))?;
            }
            "--probe-stable" => {
                let v = value("--probe-stable")?;
                args.probe_stable =
                    v.parse().map_err(|_| format!("--probe-stable wants a frame count, got {v}"))?;
            }
            "--scale" => {
                let v = value("--scale")?;
                args.scale = v.parse().map_err(|_| format!("--scale wants a number 1-4, got {v}"))?;
            }
            "--watch" => {
                let list = value("--watch")?;
                for part in list.split(',') {
                    let part = part.trim();
                    match part.split_once('-') {
                        Some((lo, hi)) => {
                            let lo = parse_addr(lo)?;
                            let hi = parse_addr(hi)?;
                            if hi < lo {
                                return Err(format!("--watch range {part} has hi < lo"));
                            }
                            args.watch.extend(lo..=hi);
                        }
                        None => args.watch.push(parse_addr(part)?),
                    }
                }
            }
            other => return Err(format!("unknown argument {other}; try --help")),
        }
    }
    Ok(Some(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_parse_in_every_spelling() {
        assert_eq!(parse_addr("0x7E0577"), Ok(0x7E_0577));
        assert_eq!(parse_addr("$0577"), Ok(0x0577));
        assert_eq!(parse_addr("577"), Ok(0x577));
        assert!(parse_addr("nope").is_err());
    }

    #[test]
    fn a_game_is_named_or_pointed_at() {
        assert_eq!(manifest_path("dkc"), Path::new("manifests/dkc.toml"));
        assert_eq!(manifest_path("other/smb3.toml"), Path::new("other/smb3.toml"));
    }

    #[test]
    fn an_override_retargets_a_watch_or_silences_it() {
        let mut m =
            Manifest::load(Path::new("../../manifests/dkc.toml")).expect("the shipped manifest");
        apply_addr_override("lives=0x7E08A6", &mut m).unwrap();
        assert_eq!(m.watch("lives").unwrap().at, Some(0x08A6));
        // Width is preserved when the spec does not mention one.
        assert_eq!(m.watch("lives").unwrap().width, Width::U16);

        apply_addr_override("hit_timer=0x08A5:u8", &mut m).unwrap();
        assert_eq!(m.watch("hit_timer").unwrap().width, Width::U8);

        apply_addr_override("lives=off", &mut m).unwrap();
        assert_eq!(m.watch("lives").unwrap().at, None);
    }

    #[test]
    fn a_malformed_override_is_refused_rather_than_ignored() {
        let mut m =
            Manifest::load(Path::new("../../manifests/dkc.toml")).expect("the shipped manifest");
        assert!(apply_addr_override("lives", &mut m).is_err());
        assert!(apply_addr_override("lives=zzz", &mut m).is_err());
        assert!(apply_addr_override("lives=0x0577:u64", &mut m).is_err());
        assert!(apply_addr_override("nonsense=0x0577", &mut m).is_err());
        // Past the end of the machine the manifest declared. The bound is the
        // declared space, not a constant: `u32` used to be refused here as a
        // bad width and is now a real one (ADR 0018), so what is left to catch
        // is an address this console cannot reach.
        assert!(apply_addr_override("lives=0x900000", &mut m).is_err());
        // ...and one it can, spelled either way, still works.
        assert!(apply_addr_override("lives=0x7F0000", &mut m).is_ok());
    }
}
