//! The libretro adapter: one manifest, one core, one stage at a time.
//!
//! This is the [`StageRunner`] ADR 0012 said to extract once there were two
//! implementations to read it off. There are, and the interface is what
//! survived both: enter a stage, advance a frame, be told what happened. Every
//! word either harness used that only made sense for an emulator — savestate,
//! core, WRAM, `retro_run` — stopped at this line (invariant 3).

use std::fs;
use std::path::{Path, PathBuf};

use session_api::{
    EntryPoint, Presentable, RunnerError, SessionEvent, StageRunner, StageSpec, VideoFrame,
};

use crate::libretro::{
    self, Core, RETRO_DEVICE_JOYPAD, RETRO_DEVICE_KEYBOARD, RETRO_MEMORY_SAVE_RAM,
    RETRO_MEMORY_SYSTEM_RAM, VIDEO,
};
use crate::manifest::{Controller, Manifest, RomHeader};
use crate::watch::Watcher;

pub struct LibretroRunner {
    manifest: Manifest,
    core: Core,
    watcher: Watcher,
    frame: u64,
    /// Whatever `advance` last observed. Held here because [`StageRunner`]
    /// hands back a borrow rather than an allocation per frame.
    events: Vec<SessionEvent>,
    av: libretro::SystemAvInfo,
    /// The content, kept so a `Named` entry can re-load it. A DOS bundle is a
    /// few megabytes and a console ROM less; holding it costs nothing next to
    /// the core's own working set, and re-reading it per entry would let the
    /// file change underneath a run.
    rom: Vec<u8>,
    rom_path: PathBuf,
    /// `RETRO_DEVICE_JOYPAD` or `RETRO_DEVICE_KEYBOARD`, from the manifest.
    device: u32,
    /// Filled by the scan window each time a discovery tool asks. Held rather
    /// than allocated per call because it is megabytes for a DOS game.
    scan_buf: Vec<u8>,
    /// Whether the machine is still as loading the content left it.
    ///
    /// A `Named` entry means "boot from cold", and a machine that has not been
    /// played yet is already cold — so the first one costs no reload. It
    /// matters because for a DOS game a reload is a real DOS boot, and paying
    /// for two of them every time the harness starts is fifteen wasted seconds
    /// on the exact workflow this entry point exists for.
    fresh: bool,
}

impl LibretroRunner {
    /// Loads the core and the ROM the manifest names, and runs one frame so
    /// the core is ready to be handed a state.
    ///
    /// `core_override` is the `--core` flag; without it the manifest's
    /// `core_dir` must hold exactly one `*_libretro.dll`. Ambiguity is an
    /// error, not a guess — picking the NES core for a SNES game by accident
    /// is a real possibility now that the library has two consoles in it.
    pub fn start(
        manifest: Manifest,
        core_override: Option<PathBuf>,
        rom_override: Option<PathBuf>,
        watched: Vec<usize>,
    ) -> Result<LibretroRunner, String> {
        let core_path = match core_override {
            Some(p) => p,
            None => find_core(&manifest.libretro.core_dir)?,
        };
        let rom_path = match rom_override {
            Some(p) => p,
            None => manifest
                .rom
                .path
                .clone()
                .ok_or("the manifest names no rom.path; pass --rom")?,
        };

        let rom = fs::read(&rom_path)
            .map_err(|e| format!("cannot read content {}: {e}", rom_path.display()))?;
        check_rom(&manifest, &rom, &rom_path)?;

        // Where the core persists what the guest writes. Beside the content
        // rather than beside the core, because it belongs to the game: for a
        // DOS bundle it is the guest's own filesystem, and a stale one turns
        // the next cold boot into a warm one.
        let save_dir = save_dir_for(&rom_path);
        if let Err(e) = fs::create_dir_all(&save_dir) {
            eprintln!("[save] cannot create {}: {e}", save_dir.display());
        }

        let mut core = Core::load(&core_path, Some(&save_dir))?;
        let info = core.system_info();
        println!("[core] {} {}  <- {}", info.name, info.version, core_path.display());

        // Options are pinned *before* `retro_init`: that is when a core first
        // reads them, and dosbox-pure sizes the machine it builds from them.
        // Setting one afterwards would be a setting the boot never saw.
        for (key, value) in &manifest.libretro.options {
            libretro::set_option(key, value);
        }
        core.init();
        // Only now has the core published what its options are, so only now can
        // the manifest's be checked against them. A typo here is a player on a
        // differently configured machine, not a no-op (ADR 0018).
        libretro::check_options()?;
        if !manifest.libretro.options.is_empty() {
            println!(
                "[opts] pinned {}: {}",
                manifest.libretro.options.len(),
                manifest
                    .libretro
                    .options
                    .iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }

        let device = match manifest.libretro.controller {
            Controller::Pad(_) => RETRO_DEVICE_JOYPAD,
            Controller::Keyboard => RETRO_DEVICE_KEYBOARD,
        };
        core.load_game(&rom_path, &rom, device)?;

        let av = core.av_info();
        println!(
            "[av]   {}x{} @ {:.4} fps, {} Hz",
            av.geometry.base_width, av.geometry.base_height, av.timing.fps, av.timing.sample_rate as u32
        );
        // A manifest states its own framerate, because that is what durations
        // authored in seconds are converted at, and it has to be the same
        // number on every machine. If the core disagrees, the manifest is
        // still authoritative — but a large gap means one of them is wrong
        // about the region.
        if (av.timing.fps - manifest.framerate.0).abs() > 0.5 {
            eprintln!(
                "[av]   the manifest says {:.4} fps and the core says {:.4}; \
                 durations are converted at the manifest's rate",
                manifest.framerate.0, av.timing.fps
            );
        }

        // Two routes to a byte, and a game needs *one* of them — which one is
        // the manifest's `prefer`. Requiring the system-RAM slice was right
        // while every core was a console core; dosbox-pure answers
        // `retro_get_memory_data` with nothing at all and publishes three
        // `SET_MEMORY_MAPS` descriptors instead (ADR 0018), and refusing it
        // would be refusing the route the NES manifest already uses.
        match core.memory(RETRO_MEMORY_SYSTEM_RAM) {
            Some(m) => {
                let expected = manifest.libretro.system_ram_bytes;
                match expected {
                    Some(n) if m.len() != n => println!(
                        "[ram]  system {} bytes, expected {n} — this core does not expose \
                         memory the way the manifest's addresses assume",
                        m.len()
                    ),
                    _ => println!("[ram]  system {} bytes", m.len()),
                }
            }
            None => println!("[ram]  system none — every address goes through the memory map"),
        }
        if let Some(m) = core.memory(RETRO_MEMORY_SAVE_RAM) {
            println!("[ram]  save   {} bytes", m.len());
        }
        if libretro::has_memory_map() {
            let map = libretro::memory_map();
            let readable = map.iter().filter(|r| r.readable()).count();
            println!("[ram]  memory map: {} descriptors, {readable} readable", map.len());
        }
        if core.memory(RETRO_MEMORY_SYSTEM_RAM).is_none() && !libretro::has_memory_map() {
            return Err("core exposes neither RETRO_MEMORY_SYSTEM_RAM nor a memory map, \
                        so there is no route to a byte and RAM watching is impossible"
                .into());
        }

        let watcher = Watcher::new(&manifest, watched);
        let mut runner = LibretroRunner {
            manifest,
            core,
            watcher,
            frame: 0,
            events: Vec::new(),
            av,
            rom,
            rom_path,
            device,
            scan_buf: Vec::new(),
            fresh: true,
        };

        // Some cores are not ready to unserialize until they have produced a
        // frame, so this one is spent before anything is loaded into them.
        // `fresh` survives it deliberately: one frame in, a DOS machine has not
        // finished its BIOS, and calling that "already booted" is what saves
        // the first `Named` entry a redundant fifteen-second boot.
        runner.core.run_frame();

        // Which addresses this core can actually reach, said once. The NES
        // harness had to write this warning by hand for one address; it is
        // worth having for all of them.
        let unreadable = {
            let ram = runner.core.memory(RETRO_MEMORY_SYSTEM_RAM).unwrap_or(&[]);
            runner.watcher.unreadable(ram)
        };
        if !unreadable.is_empty() {
            eprintln!(
                "[addr] this core cannot resolve: {} — every detector built on \
                 them stays quiet",
                unreadable.join(", ")
            );
        }
        let unset = runner.watcher.unset();
        if !unset.is_empty() {
            println!("[addr] no address found yet, so contributing nothing: {}", unset.join(", "));
        }

        Ok(runner)
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn av_info(&self) -> libretro::SystemAvInfo {
        self.av
    }

    pub fn watcher_mut(&mut self) -> &mut Watcher {
        &mut self.watcher
    }

    pub fn system_ram(&self) -> Option<&[u8]> {
        self.core.memory(RETRO_MEMORY_SYSTEM_RAM)
    }

    /// The bytes the RAM discovery tools work over, and the address the first
    /// of them sits at.
    ///
    /// For a console this is the system-RAM slice, borrowed, and the address
    /// is the space's own base — exactly what `Search` and `Probe` have always
    /// been handed. For a core with no such slice it is the manifest's
    /// `[runner.libretro.memory.scan]` window, read out of the memory map into
    /// a buffer. `None` when there is neither, which means there is nothing to
    /// search rather than that something failed.
    pub fn scan_window(&mut self) -> Option<(usize, &[u8])> {
        if let Some(w) = self.manifest.libretro.scan {
            let space = self.manifest.libretro.space;
            let ram = self.core.memory(RETRO_MEMORY_SYSTEM_RAM).unwrap_or(&[]);
            space.read_into(w.at, w.size, ram, &mut self.scan_buf);
            return Some((w.at, &self.scan_buf));
        }
        let base = self.manifest.libretro.space.base;
        self.core.memory(RETRO_MEMORY_SYSTEM_RAM).map(|ram| (base, ram))
    }

    /// How much of the declared scan window no route reaches. Printed once, so
    /// a window aimed at nothing is visible rather than looking like a search
    /// that keeps finding zeros.
    pub fn scan_window_gap(&mut self) -> Option<(usize, usize)> {
        let w = self.manifest.libretro.scan?;
        let space = self.manifest.libretro.space;
        let ram = self.core.memory(RETRO_MEMORY_SYSTEM_RAM).unwrap_or(&[]);
        let missing = space.read_into(w.at, w.size, ram, &mut self.scan_buf);
        Some((missing, w.size))
    }

    /// One of the four `retro_get_memory_data` regions, for a memory-map dump.
    pub fn memory(&self, which: u32) -> Option<&[u8]> {
        self.core.memory(which)
    }

    /// Print every watched field and every `--watch` address right now.
    pub fn dump_watches(&self) {
        let ram = self.core.memory(RETRO_MEMORY_SYSTEM_RAM).unwrap_or(&[]);
        self.watcher.dump(ram, self.frame);
    }

    /// Capture the machine's whole state, for authoring a new entry point.
    pub fn capture_state(&self) -> Result<Vec<u8>, String> {
        self.core.save_state()
    }

    /// Start over from the machine's power-on state, and restart the clock.
    pub fn reset(&mut self) {
        self.core.reset();
        libretro::keys_clear();
        self.watcher.reset();
        self.frame = 0;
    }

    /// Whether the core has asked to be shut down. A DOS core reaches this the
    /// moment the guest program exits — quitting Doom from its own menu is
    /// enough — and running frames past it drives a torn-down core.
    pub fn shutdown_requested(&self) -> bool {
        libretro::shutdown_requested()
    }

    /// Advance a frame without reading RAM. Only the dev harness needs this,
    /// to spin the core while a menu is up.
    pub fn run_frame_raw(&mut self) {
        self.core.run_frame();
        self.fresh = false;
    }
}

impl StageRunner for LibretroRunner {
    fn enter(&mut self, spec: &StageSpec) -> Result<(), RunnerError> {
        // A key the core still thinks is held would survive into the stage, and
        // a stage that starts with the player walking into a wall is not the
        // same stage for the next person.
        libretro::keys_clear();
        libretro::JOYPAD.store(0, std::sync::atomic::Ordering::Relaxed);

        match &spec.entry {
            EntryPoint::State(key) => {
                let path = PathBuf::from(&key.0);
                let data = fs::read(&path).map_err(|e| {
                    RunnerError(format!("cannot read entry state {}: {e}", path.display()))
                })?;
                // No boot first, deliberately. A DOS state carries the whole
                // machine, so it restores into one that has barely started —
                // measured at 60 frames, against ~960 for a cold boot. That is
                // the difference between a death costing a second and costing
                // sixteen, which is why a real session enters at a state and
                // only an author enters at a `named` point.
                self.core.load_state(&data).map_err(RunnerError)?;
                println!(
                    "[stage] entered {} from {} ({} bytes); frame counter reset to 0",
                    spec.stage,
                    path.display(),
                    data.len()
                );
            }
            // The game is asked to start somewhere itself. Nothing is passed to
            // the core: a DOS bundle carries its own `dosbox.conf`, whose
            // `[autoexec]` is the command line that does the warping (ADR
            // 0002's note), so the name identifies the place and the content
            // knows how to reach it.
            //
            // Re-loading rather than `retro_reset`, because a DOS core builds
            // its machine out of the content at load time, and that is what
            // reproduces a cold boot.
            EntryPoint::Named(name) => {
                let boot = self.manifest.libretro.boot_frames;
                if boot == 0 {
                    return Err(RunnerError(format!(
                        "{} enters at the named point `{name}`, but this manifest \
                         declares no runner.libretro.boot_seconds, so there is no \
                         way to know when the machine has finished booting",
                        spec.stage
                    )));
                }
                if !self.fresh {
                    let rom = std::mem::take(&mut self.rom);
                    let result = self.core.reload_game(&self.rom_path, &rom, self.device);
                    self.rom = rom;
                    result.map_err(RunnerError)?;
                }
                println!("[stage] booting {} to `{name}` ({boot} frames)", spec.stage);
                for _ in 0..boot {
                    self.core.run_frame();
                }
            }
        }

        self.fresh = false;
        self.watcher.reset();
        // Stage entry is frame 0. Every duration reported from here is measured
        // from this point, which is what makes two players comparable at all
        // (ADR 0005). Boot frames are before the stage, so they are not in it.
        self.frame = 0;
        self.events.clear();
        self.events.push(SessionEvent::StageEntered { frame: 0, stage: spec.stage.clone() });
        Ok(())
    }

    fn advance(&mut self) -> Result<&[SessionEvent], RunnerError> {
        self.core.run_frame();
        self.fresh = false;
        self.frame += 1;
        self.events.clear();
        // An empty slice is the honest input for a core that publishes no
        // system RAM: `AddressSpace::read` then finds every byte through the
        // memory map, which is the route such a manifest declares anyway.
        let ram = self.core.memory(RETRO_MEMORY_SYSTEM_RAM).unwrap_or(&[]);
        // The watcher borrows RAM out of the core, so what it produces is
        // copied before the core is touched again.
        let observed = self.watcher.tick(ram, self.frame);
        self.events.extend_from_slice(observed);
        Ok(&self.events)
    }

    fn frame(&self) -> u64 {
        self.frame
    }
}

impl Presentable for LibretroRunner {
    fn present(&self, since: u64, f: &mut dyn FnMut(VideoFrame<'_>)) -> u64 {
        let video = match VIDEO.lock() {
            Ok(v) => v,
            Err(e) => e.into_inner(),
        };
        if video.width == 0 || video.buf.len() != video.width * video.height {
            return since;
        }
        // A duped frame keeps its sequence number, so the caller can skip the
        // repaint and still pump its window.
        if video.seq != since {
            f(VideoFrame {
                pixels: &video.buf,
                width: video.width,
                height: video.height,
                seq: video.seq,
            });
        }
        video.seq
    }
}

/// What can be checked about a ROM without a hash of a known-good dump.
///
/// Not much, and deliberately per console family rather than per game: the NES
/// has a magic number, the SNES has none at all — its header is in-band, at a
/// position that depends on the mapper — so all the SNES gets is a size floor
/// and a note about the copier header, which shifts every ROM offset by 512
/// bytes and makes an address table look wrong for no visible reason.
fn check_rom(manifest: &Manifest, rom: &[u8], path: &Path) -> Result<(), String> {
    if rom.len() < manifest.rom.min_bytes {
        return Err(format!(
            "{} is {} bytes, too small to be a {} ROM",
            path.display(),
            rom.len(),
            manifest.console
        ));
    }
    let mut note = String::new();
    match manifest.rom.header {
        RomHeader::None => {}
        RomHeader::INes => {
            if rom.len() < 16 || &rom[0..4] != b"NES\x1a" {
                return Err(format!("{} is not an iNES ROM (bad 16-byte header)", path.display()));
            }
        }
        RomHeader::CopierOptional => {
            if rom.len() % 1024 == 512 {
                note = ", has a 512-byte copier header".to_string();
            }
        }
    }
    println!("[rom]  {} ({} bytes{note})", path.display(), rom.len());
    Ok(())
}

/// Where a core is told to persist what the guest writes.
///
/// Beside the content and named after it, because that is what it belongs to: a
/// DOS bundle's save directory holds the guest's own filesystem writes, and
/// mixing two games' into one directory is how one game's boot stops being
/// reproducible because of another.
fn save_dir_for(rom_path: &Path) -> PathBuf {
    let stem = rom_path.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    rom_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join(".saves")
        .join(stem)
}

/// Exactly one core in the manifest's core directory, or an error naming what
/// it found.
pub fn find_core(dir: &Path) -> Result<PathBuf, String> {
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
            "no core found. Put a *_libretro.dll in {}/ or pass --core <PATH>",
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
