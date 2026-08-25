//! Phase 2, first half: a single-game libretro harness for Donkey Kong
//! Country, written as a sibling of `smb3-harness` rather than on top of it.
//!
//! Loads a SNES core, runs the ROM in a window with sound and a gamepad,
//! watches RAM for death / damage / stage clear / enemy kills / items, jumps
//! into a stage from a savestate, and prints what it sees. Two harnesses that
//! solved the same problem independently is what Phase 2's second half needs
//! in order to extract an interface from evidence rather than from taste
//! (ADR 0012). Nothing here is shared with the NES harness on purpose.
//!
//! Every default address is a lead, not a fact — see the provenance table at
//! the top of `dkc.rs`. `--addr` retargets any of them without a rebuild, and
//! F3-F8/F10/F11 are the tools for finding the right one.

mod audio;
mod dkc;
mod input;
mod levels;
mod libretro;
mod ramsearch;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use minifb::{Key, KeyRepeat, Scale, ScaleMode, Window, WindowOptions};

use dkc::Width;
use libretro::{Core, RETRO_MEMORY_SAVE_RAM, RETRO_MEMORY_SYSTEM_RAM, VIDEO};
use ramsearch::{Filter, Probe, Search};

const DEFAULT_ROM: &str = "assets/ROMs/snes/Donkey Kong Country (U) (V1.0) [!].smc";
const DEFAULT_STATE: &str = "assets/states/dkc_quick.state";
/// SNES cores live in their own folder so the NES core next door is not an
/// ambiguity every run.
const CORE_DIR: &str = "cores/snes";

/// Candidates listed by one keypress. A search that has not narrowed yet holds
/// every address in RAM, and a console is not the place for 128 K lines.
const LIST_LIMIT: usize = 40;

const HELP: &str = "\
dkc-harness - Phase 2 libretro harness (Donkey Kong Country, SNES)

USAGE:
    dkc-harness [OPTIONS]

OPTIONS:
    --core <PATH>    SNES libretro core DLL. Default: the single
                     *_libretro.dll in cores/snes/
    --rom <PATH>     ROM to run.
                     Default: assets/ROMs/snes/Donkey Kong Country (U) (V1.0) [!].smc
    --state <PATH>   Savestate to load at startup, and the target for F1/F2.
                     Frame counting restarts from 0 when it loads.
    --scale <1-4>    Window scale. Default: 3
    --watch <ADDRS>  Comma-separated addresses to report on change, e.g.
                     --watch 0x7E0577,0x057B. An entry may be a range lo-hi,
                     e.g. --watch 0x0570-0x05a0. Offsets and full $7Exxxx
                     addresses both work.
    --addr <N=A[:W]> Retarget a watched quantity without rebuilding, e.g.
                     --addr lives=0x7E0577 --addr hit_timer=0x08A5:u8
                     W is u8 or u16 (default u16). --addr <name>=off turns
                     that detector off. Repeatable.
    --list-addrs     Print the address table with its sources, and exit.
    --search-width <u8|u16>
                     Width the F3-F8 RAM search compares at. Default: u16
    --probe-recent <N>
                     F11 reports bytes that changed within N frames. Default: 20
    --probe-stable <N>
                     ...and that held still for N frames before that. Default: 180
    --transition-window <N>
                     Frames to wait after a fade for the level code to catch
                     up and say where the stage was left for. Default: 120.
    --no-probe       Skip the per-frame change history (saves ~2.3 MB and a
                     full RAM compare each frame; disables F11)
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
    core: Option<PathBuf>,
    rom: PathBuf,
    state: Option<PathBuf>,
    scale: usize,
    watch: Vec<usize>,
    addrs: dkc::Addrs,
    list_addrs: bool,
    search_width: Width,
    probe_recent: u64,
    probe_stable: u64,
    transition_window: u64,
    probe: bool,
    audio: bool,
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

    if args.list_addrs {
        print_addrs(&args.addrs);
        return Ok(());
    }

    let core_path = match args.core {
        Some(p) => p,
        None => find_core()?,
    };

    let rom = fs::read(&args.rom)
        .map_err(|e| format!("cannot read ROM {}: {e}", args.rom.display()))?;
    // An SNES ROM has no magic number to check — the header is in-band, at a
    // location that depends on the mapper. So the only cheap checks are that
    // it is plausibly a ROM at all, and whether it carries a copier header,
    // which shifts every ROM offset by 512 bytes. snes9x strips one itself;
    // say so anyway, because it is the sort of thing that makes an address
    // table look wrong for no visible reason.
    if rom.len() < 0x8000 {
        return Err(format!(
            "{} is {} bytes, too small to be a SNES ROM",
            args.rom.display(),
            rom.len()
        ));
    }
    let copier_header = rom.len() % 1024 == 512;
    println!(
        "[rom]  {} ({} bytes{})",
        args.rom.display(),
        rom.len(),
        if copier_header { ", has a 512-byte copier header" } else { "" }
    );

    let mut core = Core::load(&core_path)?;
    let (name, version) = core.system_info();
    println!("[core] {name} {version}  <- {}", core_path.display());
    core.init();
    core.load_game(&args.rom, &rom)?;

    let av = core.av_info();
    let (width, height) = (
        av.geometry.base_width as usize,
        av.geometry.base_height as usize,
    );
    println!(
        "[av]   {width}x{height} @ {:.4} fps, {} Hz",
        av.timing.fps, av.timing.sample_rate as u32
    );

    // Unlike the NES side, the whole of the address space this harness cares
    // about arrives here: snes9x publishes all 128 KB of work RAM as system
    // RAM. If that is not what we got, every address below is meaningless and
    // it is better to say so than to watch the wrong bytes.
    match core.memory(RETRO_MEMORY_SYSTEM_RAM) {
        Some(m) if m.len() == dkc::WRAM_SIZE => {
            println!("[ram]  system {} bytes ($7E0000-$7FFFFF)", m.len())
        }
        Some(m) => println!(
            "[ram]  system {} bytes, expected {} — this core does not expose SNES \
             work RAM the way the address table assumes",
            m.len(),
            dkc::WRAM_SIZE
        ),
        None => {
            return Err(
                "core exposes no RETRO_MEMORY_SYSTEM_RAM, so RAM watching is impossible".into(),
            )
        }
    }
    if let Some(m) = core.memory(RETRO_MEMORY_SAVE_RAM) {
        // DKC's cartridge SRAM is battery-backed, so unlike SMB3 it does show
        // up here. Nothing watches it yet — it holds the save file, not live
        // stage state — but its presence is worth confirming.
        println!("[ram]  save   {} bytes (cartridge SRAM)", m.len());
    }
    if libretro::has_memory_map() {
        println!("[ram]  memory map published (only used outside work RAM)");
    }

    print_addrs(&args.addrs);
    print_completion_rule();

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
        "DKC harness",
        width.max(1),
        height.max(1),
        WindowOptions {
            scale: match args.scale {
                1 => Scale::X1,
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

    let mut pads = input::Pads::new();
    let mut watcher = dkc::Watcher::new(args.addrs, args.watch, args.transition_window);
    let mut search = Search::new(args.search_width);
    let mut probe = args.probe.then(Probe::new);
    let state_path = args.state.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_STATE));

    // Some cores are not ready to unserialize until they have produced a frame.
    core.run_frame();

    // `frame` is emulated frames since stage entry, not since process start.
    // ADR 0005: timing is measured in emulated frames, never wall-clock.
    let mut frame: u64 = 0;
    if let Some(path) = args.state.as_deref() {
        load_state(&mut core, path, &mut watcher, probe.as_mut(), &mut frame)?;
    }

    let frame_time = Duration::from_secs_f64(1.0 / av.timing.fps.max(1.0));
    let audio_target = audio
        .as_ref()
        .map(|a| (audio::TARGET_QUEUED_FRAMES as f64 * a.device_rate / av.timing.fps) as usize);
    let mut next_deadline = Instant::now();
    let mut last_seq = 0u64;

    while window.is_open() && !window.is_key_down(Key::Escape) {
        for key in window.get_keys_pressed(KeyRepeat::No) {
            // Every RAM key needs the same slice, and the borrow ends with the
            // match arm, so it is taken per key rather than held across the
            // loop (the core needs `&mut` to run a frame).
            match key {
                Key::F1 => match core.save_state() {
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
                    if let Err(e) = load_state(
                        &mut core,
                        &state_path,
                        &mut watcher,
                        probe.as_mut(),
                        &mut frame,
                    ) {
                        eprintln!("[state] {e}");
                    }
                }
                Key::F5 => {
                    if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
                        watcher.dump(ram, frame);
                    }
                }
                Key::F9 => {
                    core.reset();
                    watcher.reset();
                    if let Some(p) = probe.as_mut() {
                        p.reset();
                    }
                    frame = 0;
                    println!("[core] reset; frame counter restarted");
                }
                Key::F3 => {
                    if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
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
                    if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
                        search.filter(ram, f);
                    }
                }
                Key::F10 => {
                    if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
                        search.list(ram, LIST_LIMIT);
                    }
                }
                Key::F11 => match (probe.as_ref(), core.memory(RETRO_MEMORY_SYSTEM_RAM)) {
                    (Some(p), Some(ram)) => {
                        p.report(ram, frame, args.probe_recent, args.probe_stable, LIST_LIMIT)
                    }
                    (None, _) => println!("[probe] disabled by --no-probe"),
                    (_, None) => {}
                },
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

        core.run_frame();
        frame += 1;

        if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
            if let Some(p) = probe.as_mut() {
                p.tick(ram, frame);
            }
            for event in watcher.tick(ram, frame) {
                match event {
                    dkc::Event::StageComplete { at } => println!("STAGE COMPLETE at frame {at}"),
                    dkc::Event::StageFailed { at, reason: dkc::FailReason::Death } => {
                        println!("STAGE FAILED at frame {at} (died)")
                    }
                    // Already printed its own [event] line with the count and
                    // what the count means; listed explicitly so it is clear
                    // it is deliberately not a verdict of its own.
                    dkc::Event::Fade { .. } => {}
                    // The rest already printed their own [event] line; they
                    // are returned so a caller that is not a console — the
                    // real client, later — has them too.
                    _ => {}
                }
            }
        }

        let video = match VIDEO.lock() {
            Ok(v) => v,
            Err(e) => e.into_inner(),
        };
        if video.width > 0 && video.buf.len() == video.width * video.height {
            // A duped frame still needs an update() to pump window messages,
            // but repainting an identical buffer is wasted work.
            if video.seq != last_seq {
                last_seq = video.seq;
                let (w, h) = (video.width, video.height);
                window
                    .update_with_buffer(&video.buf, w, h)
                    .map_err(|e| format!("cannot present frame: {e}"))?;
            } else {
                drop(video);
                window.update();
            }
        } else {
            drop(video);
            window.update();
        }
    }

    Ok(())
}

/// The address table as it will actually be used, with where each default came
/// from. Printed at startup because a harness whose whole job is checking
/// unverified addresses should say which ones it is checking.
fn print_addrs(addrs: &dkc::Addrs) {
    println!("[addr] watched quantities (override with --addr name=ADDR[:u8|u16]):");
    let mut unset = Vec::new();
    for (name, field, note) in addrs.list() {
        let shown = format!("{field}");
        println!("        {name:<14} {shown:<18} {note}");
        if field.at.is_none() {
            unset.push(name);
        }
    }
    if !unset.is_empty() {
        println!(
            "[addr] no address yet, so these detectors are off: {}",
            unset.join(", ")
        );
        println!("[addr] F3-F8/F10 hunt for a counter; F11 reports what a one-shot event wrote");
    }
}

/// How a clear is decided, stated at startup because it rests on two signals
/// rather than one and the pairing is not obvious from the output alone.
fn print_completion_rule() {
    println!(
        "[stage] a clear is a fade at $7E051B=0x82 (timing; never fires on a death) \
         followed by the level code at $7E003E leaving this stage's family"
    );
    println!(
        "[stage] bonus rooms, checkpoints and returns stay inside the family, so the \
         route through a stage does not change the verdict"
    );
}

fn load_state(
    core: &mut Core,
    path: &Path,
    watcher: &mut dkc::Watcher,
    probe: Option<&mut Probe>,
    frame: &mut u64,
) -> Result<(), String> {
    let data =
        fs::read(path).map_err(|e| format!("cannot read savestate {}: {e}", path.display()))?;
    core.load_state(&data)?;
    watcher.reset();
    // RAM has just been replaced wholesale. Without re-arming, every byte that
    // differs from before the load looks like it changed this instant.
    if let Some(p) = probe {
        p.reset();
    }
    // Stage entry is frame 0. Every duration this harness reports is measured
    // from here, which is what makes two players comparable.
    *frame = 0;
    println!(
        "[stage] entered {} from {} ({} bytes); frame counter reset to 0",
        stage_label(path),
        path.display(),
        data.len()
    );
    Ok(())
}

/// The stage a savestate drops you into is known from the file we were asked
/// for, not read out of RAM. Stage identity belongs to the entry point — the
/// level id in RAM only confirms it, and is what the watcher compares against
/// to notice the stage ending.
fn stage_label(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem.strip_prefix("dkc_") {
        Some(rest) => rest.to_string(),
        None => stem,
    }
}

/// Look for exactly one core in cores/snes/. Ambiguity is an error, not a
/// guess — and now that there are two harnesses, picking the NES core by
/// accident is a real possibility, which is why this looks in a subfolder
/// rather than in cores/ directly.
fn find_core() -> Result<PathBuf, String> {
    let dir = Path::new(CORE_DIR);
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_lowercase();
            if name.ends_with("_libretro.dll") {
                found.push(path);
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "no SNES core found. Put a *_libretro.dll in {}/ or pass --core <PATH>",
            dir.display()
        )),
        _ => Err(format!(
            "several cores in {}/; pick one with --core: {}",
            dir.display(),
            found
                .iter()
                .map(|p| p.file_name().unwrap_or_default().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(", ")
        )),
    }
}

/// Parses one hex address, with or without an `0x` or `$` prefix.
fn parse_addr(s: &str) -> Result<usize, String> {
    let hex = s.trim().trim_start_matches("0x").trim_start_matches('$');
    usize::from_str_radix(hex, 16).map_err(|_| format!("wanted a hex address, got `{s}`"))
}

/// `name=ADDR[:u8|u16]`, or `name=off` to turn a detector off.
fn parse_addr_override(spec: &str, addrs: &mut dkc::Addrs) -> Result<(), String> {
    let (name, rest) = spec
        .split_once('=')
        .ok_or_else(|| format!("--addr wants name=ADDR, got `{spec}`"))?;
    let name = name.trim();
    let rest = rest.trim();
    if rest.eq_ignore_ascii_case("off") || rest.eq_ignore_ascii_case("none") {
        return addrs.set(name, None, None);
    }
    let (addr, width) = match rest.split_once(':') {
        Some((a, w)) => (parse_addr(a)?, Some(Width::parse(w.trim())?)),
        None => (parse_addr(rest)?, None),
    };
    addrs.set(name, Some(addr), width)
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        core: None,
        rom: PathBuf::from(DEFAULT_ROM),
        state: None,
        scale: 3,
        watch: Vec::new(),
        addrs: dkc::Addrs::default(),
        list_addrs: false,
        search_width: Width::U16,
        // ~a third of a second either side of the keypress: long enough to
        // cover a human's reaction time, short enough that the frame the game
        // wrote is still nearby.
        probe_recent: 20,
        // Three seconds of holding still. Anything busier than that is a
        // timer, a position, or the RNG, and is not what a hit wrote.
        probe_stable: 180,
        transition_window: dkc::DEFAULT_TRANSITION_WINDOW_FRAMES,
        probe: true,
        audio: true,
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = |flag: &str| -> Result<String, String> {
            it.next().ok_or_else(|| format!("{flag} needs a value"))
        };
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--core" => args.core = Some(PathBuf::from(value("--core")?)),
            "--rom" => args.rom = PathBuf::from(value("--rom")?),
            "--state" => args.state = Some(PathBuf::from(value("--state")?)),
            "--no-audio" => args.audio = false,
            "--no-probe" => args.probe = false,
            "--list-addrs" => args.list_addrs = true,
            "--addr" => {
                let spec = value("--addr")?;
                parse_addr_override(&spec, &mut args.addrs)?;
            }
            "--search-width" => args.search_width = Width::parse(value("--search-width")?.trim())?,
            "--probe-recent" => {
                let v = value("--probe-recent")?;
                args.probe_recent = v
                    .parse()
                    .map_err(|_| format!("--probe-recent wants a frame count, got {v}"))?;
            }
            "--probe-stable" => {
                let v = value("--probe-stable")?;
                args.probe_stable = v
                    .parse()
                    .map_err(|_| format!("--probe-stable wants a frame count, got {v}"))?;
            }
            "--transition-window" => {
                let v = value("--transition-window")?;
                args.transition_window = v
                    .parse()
                    .map_err(|_| format!("--transition-window wants a frame count, got {v}"))?;
            }
            "--scale" => {
                let v = value("--scale")?;
                args.scale = v
                    .parse()
                    .map_err(|_| format!("--scale wants a number 1-4, got {v}"))?;
            }
            "--watch" => {
                let list = value("--watch")?;
                for part in list.split(',') {
                    let part = part.trim();
                    // A range is `lo-hi`, and both ends may carry a `$` or
                    // `0x`, so the split has to look past a leading prefix
                    // rather than at the first '-'.
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
    fn an_override_retargets_a_field() {
        let mut a = dkc::Addrs::default();
        parse_addr_override("lives=0x7E08A6", &mut a).unwrap();
        assert_eq!(a.lives.at, Some(0x08A6));
        // Width is preserved when the spec does not mention one.
        assert_eq!(a.lives.width, Width::U16);

        parse_addr_override("hit_timer=0x08A5:u8", &mut a).unwrap();
        assert_eq!(a.hit_timer.at, Some(0x08A5));
        assert_eq!(a.hit_timer.width, Width::U8);
    }

    #[test]
    fn an_override_can_turn_a_detector_off() {
        let mut a = dkc::Addrs::default();
        assert!(a.lives.at.is_some());
        parse_addr_override("lives=off", &mut a).unwrap();
        assert_eq!(a.lives.at, None);
    }

    #[test]
    fn a_malformed_override_is_refused_rather_than_ignored() {
        let mut a = dkc::Addrs::default();
        assert!(parse_addr_override("lives", &mut a).is_err());
        assert!(parse_addr_override("lives=zzz", &mut a).is_err());
        assert!(parse_addr_override("lives=0x0577:u32", &mut a).is_err());
        assert!(parse_addr_override("nonsense=0x0577", &mut a).is_err());
    }

    #[test]
    fn the_stage_label_comes_from_the_savestate_name() {
        assert_eq!(stage_label(Path::new("assets/states/dkc_jungle-hijinxs.state")), "jungle-hijinxs");
        assert_eq!(stage_label(Path::new("quick.state")), "quick");
    }
}
