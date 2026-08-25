//! The developer harness: run one manifest's game in a window and watch RAM.
//!
//! This is not the client. It is the tool a manifest is *authored* with — it
//! prints every address it is watching and where that address came from, lets
//! any of them be retargeted without a rebuild, and carries the two RAM
//! discovery tools for finding one nobody has written down. What it does not
//! do is score anything: it prints [`SessionEvent`]s, and what a run was worth
//! is `challenge-core`'s question.
//!
//! ```sh
//! cargo run --release -p session-libretro -- --game dkc --stage jungle-hijinx
//! ```

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use minifb::{Key, KeyRepeat, Scale, ScaleMode, Window, WindowOptions};

use session_api::{Presentable, SessionEvent, StageRunner};
use session_libretro::libretro::{self, RETRO_MEMORY_IDS};
use session_libretro::manifest::Manifest;
use session_libretro::memory::Width;
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
    --challenge <ID> Challenge to run: enters its stage and prints its criteria.
                     The harness still scores nothing
    --list-stages    Print the manifest's stages and exit
    --list-challenges
                     Print every challenge and what completes it, and exit
    --list-addrs     Print the address table with its sources, and exit

OPTIONS:
    --core <PATH>    libretro core DLL. Default: the single *_libretro.dll in
                     the manifest's core_dir
    --rom <PATH>     ROM to run. Default: the manifest's rom.path
    --state <PATH>   Override the entry state, and the target for F1/F2
    --scale <1-4>    Window scale. Default: 3
    --watch <ADDRS>  Comma-separated addresses to report on change, e.g.
                     --watch 0x7E0577,0x057B. An entry may be a range lo-hi,
                     e.g. --watch 0x0570-0x05a0
    --addr <N=A[:W]> Retarget a watched quantity without rebuilding, e.g.
                     --addr lives=0x7E08A6 --addr in_stage=0x0527:u8
                     W is u8 or u16. --addr <name>=off turns that detector off
    --dump-memory-map
                     Print every memory region this core publishes - the four
                     retro_get_memory_data ids and every SET_MEMORY_MAPS
                     descriptor - and exit. Run it once per core: what a core
                     offers beyond system RAM is the difference between a
                     detector being possible and not
    --search-width <u8|u16>
                     Width the F3-F8 RAM search compares at. Default: u16
    --probe-recent <N>
                     F11 reports bytes that changed within N frames. Default: 20
    --probe-stable <N>
                     ...and that held still for N frames before that. Default: 180
    --no-probe       Skip the per-frame change history (disables F11)
    --frames <N>     Run N frames headless - no window, no sound, no pacing -
                     and print what fired. The frame numbers are the same as
                     they would be on screen
    --no-audio       Run without sound, paced off the wall clock instead
    -h, --help       This text

KEYS:
    Arrows  d-pad
    Z  B      X  A      A  Y      S  X      Q  L      W  R
    Enter  Start        RShift  Select
    F1  save state      F2  load state      F5  dump watched RAM
    F9  reset core      Esc  quit

RAM SEARCH (find a value that persists - lives, bananas, a counter):
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
    rom: Option<PathBuf>,
    state: Option<PathBuf>,
    scale: usize,
    watch: Vec<usize>,
    addrs: Vec<String>,
    list_addrs: bool,
    list_stages: bool,
    list_challenges: bool,
    dump_memory_map: bool,
    search_width: Width,
    probe_recent: u64,
    probe_stable: u64,
    probe: bool,
    audio: bool,
    frames: Option<u64>,
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
    // A challenge is only ever resolved to its stage here. The harness prints
    // the criteria and then ignores them: judging a run against them needs
    // `challenge-core`, and an adapter that decided a run was won would be the
    // exact mistake ADR 0017 exists to prevent.
    let stage = match (&args.stage, &args.challenge) {
        (Some(id), _) => manifest
            .stage(id)
            .ok_or_else(|| format!("{game} declares no stage `{id}`; try --list-stages"))?,
        (None, Some(id)) => {
            let c = manifest.challenge(id).ok_or_else(|| {
                format!("{game} declares no challenge `{id}`; try --list-challenges")
            })?;
            println!(
                "[challenge] {} ({}) — completed by: {}",
                c.id.0,
                c.kind,
                describe(&c.complete_when)
            );
            println!("[challenge] the harness scores nothing; this is what challenge-core will read");
            manifest.stage(&c.stage.0).expect("validation proved the stage exists")
        }
        (None, None) => manifest
            .stages
            .first()
            .ok_or_else(|| format!("{game} declares no stages at all"))?,
    };
    let mut spec = manifest.stage_spec(stage);
    if let Some(p) = &args.state {
        spec.entry = session_api::EntryPoint::State(session_api::AssetKey(
            p.to_string_lossy().into_owned(),
        ));
    }
    let state_path = match &spec.entry {
        session_api::EntryPoint::State(k) => PathBuf::from(&k.0),
        _ => PathBuf::from("state.bin"),
    };

    print_addrs(&manifest);
    let space = manifest.libretro.space;
    let pad_layout = manifest.libretro.pad_layout;
    let mut runner =
        LibretroRunner::start(manifest, args.core.clone(), args.rom.clone(), args.watch.clone())?;

    if args.dump_memory_map {
        print_memory_map(&runner);
        return Ok(());
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
        for _ in 0..n {
            for event in runner.advance().map_err(|e| e.to_string())? {
                if let SessionEvent::StageCleared { .. } | SessionEvent::StageExited { .. } = event
                {
                    println!("{event}");
                }
            }
        }
        println!("[done] {} frames", runner.frame());
        return Ok(());
    }

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

    let mut pads = input::Pads::new(pad_layout);
    let mut search = Search::new(args.search_width);
    let mut probe = args.probe.then(Probe::new);

    runner.enter(&spec).map_err(|e| e.to_string())?;
    if let Some(p) = probe.as_mut() {
        p.reset();
    }

    let frame_time = Duration::from_secs_f64(1.0 / av.timing.fps.max(1.0));
    let audio_target = audio
        .as_ref()
        .map(|a| (audio::TARGET_QUEUED_FRAMES as f64 * a.device_rate / av.timing.fps) as usize);
    let mut next_deadline = Instant::now();
    let mut last_seq = 0u64;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        for key in window.get_keys_pressed(KeyRepeat::No) {
            match key {
                Key::F1 => match runner.capture_state() {
                    Ok(data) => {
                        if let Some(dir) = state_path.parent() {
                            let _ = fs::create_dir_all(dir);
                        }
                        match fs::write(&state_path, &data) {
                            Ok(()) => println!(
                                "[state] saved {} bytes to {}",
                                data.len(),
                                state_path.display()
                            ),
                            Err(e) => {
                                eprintln!("[state] cannot write {}: {e}", state_path.display())
                            }
                        }
                    }
                    Err(e) => eprintln!("[state] {e}"),
                },
                Key::F2 => {
                    if let Err(e) = runner.enter(&spec) {
                        eprintln!("[state] {e}");
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
                    if let Some(ram) = runner.system_ram() {
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
                    if let Some(ram) = runner.system_ram() {
                        search.filter(ram, f);
                    }
                }
                Key::F10 => {
                    if let Some(ram) = runner.system_ram() {
                        search.list(&space, ram, LIST_LIMIT);
                    }
                }
                Key::F11 => {
                    let frame = runner.frame();
                    match (probe.as_ref(), runner.system_ram()) {
                        (Some(p), Some(ram)) => p.report(
                            &space,
                            ram,
                            frame,
                            args.probe_recent,
                            args.probe_stable,
                            LIST_LIMIT,
                        ),
                        (None, _) => println!("[probe] disabled by --no-probe"),
                        (_, None) => {}
                    }
                }
                _ => {}
            }
        }

        pads.pump();
        libretro::JOYPAD.store(input::keyboard(&window) | pads.state(), Ordering::Relaxed);

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

        if let Some(p) = probe.as_mut() {
            let frame = runner.frame() + 1;
            if let Some(ram) = runner.system_ram() {
                p.tick(ram, frame);
            }
        }

        for event in runner.advance().map_err(|e| e.to_string())? {
            // The watcher has already printed a line for each of these; what is
            // worth repeating is the verdict, because it is the one thing a
            // playtester is looking for in the scroll.
            match event {
                SessionEvent::StageCleared { frame } => println!("STAGE CLEARED at frame {frame}"),
                SessionEvent::StageExited { frame, reason } => {
                    println!("STAGE EXITED at frame {frame} ({reason})")
                }
                _ => {}
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
    println!("[addr] watched quantities (override with --addr name=ADDR[:u8|u16]):");
    for w in &m.watches {
        let shown = match w.at {
            Some(a) => format!("{} {}", m.libretro.space.show(a), w.width),
            None => "unset".to_string(),
        };
        println!("        {:<14} {shown:<18} {}", w.name, w.note);
    }
}

fn print_stages(m: &Manifest) {
    println!("[stage] stages declared by {}:", m.id);
    for s in &m.stages {
        let n = m.challenges_on(&s.id).count();
        let carries = match n {
            0 => "no challenges — nothing can be dealt on it".to_string(),
            1 => "1 challenge".to_string(),
            n => format!("{n} challenges"),
        };
        println!("        {:<22} {:<26} {carries}", s.id.0, s.name);
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
    if let Some(n) = c.enemies_defeated {
        parts.push(format!("defeat {n} enemies"));
    }
    for (item, n) in &c.items_collected {
        parts.push(format!("collect {n} x {item}"));
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

/// `name=ADDR[:u8|u16]`, or `name=off` to turn a detector off.
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
        rom: None,
        state: None,
        scale: 3,
        watch: Vec::new(),
        addrs: Vec::new(),
        list_addrs: false,
        list_stages: false,
        list_challenges: false,
        dump_memory_map: false,
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
            "--rom" => args.rom = Some(PathBuf::from(value("--rom")?)),
            "--state" => args.state = Some(PathBuf::from(value("--state")?)),
            "--no-audio" => args.audio = false,
            "--frames" => {
                let v = value("--frames")?;
                args.frames =
                    Some(v.parse().map_err(|_| format!("--frames wants a count, got {v}"))?);
            }
            "--no-probe" => args.probe = false,
            "--list-addrs" => args.list_addrs = true,
            "--list-stages" => args.list_stages = true,
            "--list-challenges" => args.list_challenges = true,
            "--dump-memory-map" => args.dump_memory_map = true,
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
        assert!(apply_addr_override("lives=0x0577:u32", &mut m).is_err());
        assert!(apply_addr_override("nonsense=0x0577", &mut m).is_err());
    }
}
