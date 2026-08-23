//! Phase 1: a single-game libretro harness for Super Mario Bros. 3.
//!
//! Loads a NES core, runs the ROM in a window with sound and a gamepad, watches
//! RAM, jumps into a stage from a savestate, and prints when the stage ends.
//! Deliberately SMB3-specific. Nothing here is an abstraction (ADR 0012).

mod audio;
mod input;
mod libretro;
mod smb3;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use minifb::{Key, KeyRepeat, Scale, ScaleMode, Window, WindowOptions};

use libretro::{Core, RETRO_MEMORY_SAVE_RAM, RETRO_MEMORY_SYSTEM_RAM, VIDEO};

const DEFAULT_ROM: &str = "assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes";
const DEFAULT_STATE: &str = "assets/states/quick.state";

const HELP: &str = "\
smb3-harness - Phase 1 libretro harness

USAGE:
    smb3-harness [OPTIONS]

OPTIONS:
    --core <PATH>    NES libretro core DLL. Default: the single *_libretro.dll in cores/
    --rom <PATH>     ROM to run. Default: assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes
    --state <PATH>   Savestate to load at startup, and the target for F1/F2.
                     Frame counting restarts from 0 when it loads.
    --scale <1-4>    Window scale. Default: 3
    --watch <ADDRS>  Comma-separated RAM addresses to report on change,
                     e.g. --watch 0x0014,0x0727,0x0736
                     An entry may be a range lo-hi to sweep for an address,
                     e.g. --watch 0x0090-0x00ff
    --no-audio       Run without sound, paced off the wall clock instead
    -h, --help       This text

KEYS:
    Arrows  d-pad        Z  B        X  A        Enter  Start     RShift  Select
    F1  save state       F2  load state          F5  dump RAM
    F9  reset core       Esc  quit
";

struct Args {
    core: Option<PathBuf>,
    rom: PathBuf,
    state: Option<PathBuf>,
    scale: usize,
    watch: Vec<usize>,
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

    let core_path = match args.core {
        Some(p) => p,
        None => find_core()?,
    };

    let rom = fs::read(&args.rom)
        .map_err(|e| format!("cannot read ROM {}: {e}", args.rom.display()))?;
    if rom.len() < 16 || &rom[0..4] != b"NES\x1a" {
        return Err(format!(
            "{} is not an iNES ROM (bad 16-byte header)",
            args.rom.display()
        ));
    }
    println!("[rom]  {} ({} bytes)", args.rom.display(), rom.len());

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
    match core.memory(RETRO_MEMORY_SYSTEM_RAM) {
        Some(m) => println!("[ram]  system {} bytes", m.len()),
        None => {
            return Err(
                "core exposes no RETRO_MEMORY_SYSTEM_RAM, so RAM watching is impossible".into(),
            )
        }
    }
    if let Some(m) = core.memory(RETRO_MEMORY_SAVE_RAM) {
        println!("[ram]  save   {} bytes", m.len());
    }
    // $6000-$7FFF cartridge WRAM holds the card count, and Mesen does not
    // expose it as save RAM (it isn't battery-backed on this cart). The
    // core's published memory map is the only way to reach it; without one,
    // stage completion falls back to the $0014 heuristic alone.
    if libretro::has_memory_map() {
        println!("[ram]  memory map published; $6000-$7FFF readable");
    } else {
        eprintln!(
            "[ram]  core published no memory map: ${:04X} (cards) is unreadable, \
             so stage completion cannot be detected early",
            smb3::ADDR_CARDS
        );
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
        "SMB3 harness",
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
    let mut watcher = smb3::Watcher::new(args.watch);
    let state_path = args.state.clone().unwrap_or_else(|| PathBuf::from(DEFAULT_STATE));

    // Some cores are not ready to unserialize until they have produced a frame.
    core.run_frame();

    // `frame` is emulated frames since stage entry, not since process start.
    // ADR 0005: timing is measured in emulated frames, never wall-clock.
    let mut frame: u64 = 0;
    if let Some(path) = args.state.as_deref() {
        load_state(&mut core, path, &mut watcher, &mut frame)?;
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
                            Err(e) => eprintln!("[state] cannot write {}: {e}", state_path.display()),
                        }
                    }
                    Err(e) => eprintln!("[state] {e}"),
                },
                Key::F2 => {
                    if let Err(e) = load_state(&mut core, &state_path, &mut watcher, &mut frame) {
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
                    frame = 0;
                    println!("[core] reset; frame counter restarted");
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

        core.run_frame();
        frame += 1;

        if let Some(ram) = core.memory(RETRO_MEMORY_SYSTEM_RAM) {
            match watcher.tick(ram, frame) {
                Some(smb3::Event::StageComplete) => println!("STAGE COMPLETE at frame {frame}"),
                // No card was seen before the map-return flag fired, so this
                // could be a death — or a card-less level (fortress, castle,
                // airship) cleared normally. $0014 alone can't tell those
                // apart; see the confidence notes at the top of smb3.rs.
                Some(smb3::Event::StageFailed) => {
                    println!("STAGE FAILED at frame {frame} (died, or a card-less level cleared)")
                }
                None => {}
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

fn load_state(
    core: &mut Core,
    path: &Path,
    watcher: &mut smb3::Watcher,
    frame: &mut u64,
) -> Result<(), String> {
    let data =
        fs::read(path).map_err(|e| format!("cannot read savestate {}: {e}", path.display()))?;
    core.load_state(&data)?;
    watcher.reset();
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
/// for, not read out of RAM. Stage identity belongs to the entry point.
fn stage_label(path: &Path) -> String {
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    match stem.strip_prefix("smb3_") {
        Some(rest) => rest.to_string(),
        None => stem,
    }
}

/// Look for exactly one core in cores/. Ambiguity is an error, not a guess.
fn find_core() -> Result<PathBuf, String> {
    let dir = Path::new("cores");
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            if name.ends_with("_libretro.dll") {
                found.push(path);
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "no NES core found. Put a *_libretro.dll in {}/ or pass --core <PATH>",
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

fn parse_args() -> Result<Option<Args>, String> {
    let mut args = Args {
        core: None,
        rom: PathBuf::from(DEFAULT_ROM),
        state: None,
        scale: 3,
        watch: Vec::new(),
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
            "--scale" => {
                let v = value("--scale")?;
                args.scale = v
                    .parse()
                    .map_err(|_| format!("--scale wants a number 1-4, got {v}"))?;
            }
            "--watch" => {
                let list = value("--watch")?;
                let parse_one = |s: &str| -> Result<usize, String> {
                    let hex = s.trim().trim_start_matches("0x").trim_start_matches('$');
                    usize::from_str_radix(hex, 16)
                        .map_err(|_| format!("--watch wants hex addresses, got {s}"))
                };
                for part in list.split(',') {
                    let part = part.trim();
                    match part.split_once('-') {
                        Some((lo, hi)) => {
                            let lo = parse_one(lo)?;
                            let hi = parse_one(hi)?;
                            if hi < lo {
                                return Err(format!("--watch range {part} has hi < lo"));
                            }
                            args.watch.extend(lo..=hi);
                        }
                        None => args.watch.push(parse_one(part)?),
                    }
                }
            }
            other => return Err(format!("unknown argument {other}; try --help")),
        }
    }
    Ok(Some(args))
}
