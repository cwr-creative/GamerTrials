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

use crate::libretro::{self, Core, RETRO_MEMORY_SAVE_RAM, RETRO_MEMORY_SYSTEM_RAM, VIDEO};
use crate::manifest::{Manifest, RomHeader};
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
            .map_err(|e| format!("cannot read ROM {}: {e}", rom_path.display()))?;
        check_rom(&manifest, &rom, &rom_path)?;

        let mut core = Core::load(&core_path)?;
        let (name, version) = core.system_info();
        println!("[core] {name} {version}  <- {}", core_path.display());
        core.init();
        core.load_game(&rom_path, &rom)?;

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
            None => {
                return Err(
                    "core exposes no RETRO_MEMORY_SYSTEM_RAM, so RAM watching is impossible".into(),
                )
            }
        }
        if let Some(m) = core.memory(RETRO_MEMORY_SAVE_RAM) {
            println!("[ram]  save   {} bytes", m.len());
        }
        if libretro::has_memory_map() {
            println!("[ram]  memory map published");
        }

        let watcher = Watcher::new(&manifest, watched);
        let mut runner =
            LibretroRunner { manifest, core, watcher, frame: 0, events: Vec::new(), av };

        // Some cores are not ready to unserialize until they have produced a
        // frame, so this one is spent before anything is loaded into them.
        runner.core.run_frame();

        // Which addresses this core can actually reach, said once. The NES
        // harness had to write this warning by hand for one address; it is
        // worth having for all of them.
        if let Some(ram) = runner.core.memory(RETRO_MEMORY_SYSTEM_RAM) {
            let unreadable = runner.watcher.unreadable(ram);
            if !unreadable.is_empty() {
                eprintln!(
                    "[addr] this core cannot resolve: {} — every detector built on \
                     them stays quiet",
                    unreadable.join(", ")
                );
            }
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

    /// One of the four `retro_get_memory_data` regions, for a memory-map dump.
    pub fn memory(&self, which: u32) -> Option<&[u8]> {
        self.core.memory(which)
    }

    /// Print every watched field and every `--watch` address right now.
    pub fn dump_watches(&self) {
        match self.core.memory(RETRO_MEMORY_SYSTEM_RAM) {
            Some(ram) => self.watcher.dump(ram, self.frame),
            None => println!("[ram] the core exposes no system RAM to dump"),
        }
    }

    /// Capture the machine's whole state, for authoring a new entry point.
    pub fn capture_state(&self) -> Result<Vec<u8>, String> {
        self.core.save_state()
    }

    /// Start over from the machine's power-on state, and restart the clock.
    pub fn reset(&mut self) {
        self.core.reset();
        self.watcher.reset();
        self.frame = 0;
    }

    /// Advance a frame without reading RAM. Only the dev harness needs this,
    /// to spin the core while a menu is up.
    pub fn run_frame_raw(&mut self) {
        self.core.run_frame();
    }
}

impl StageRunner for LibretroRunner {
    fn enter(&mut self, spec: &StageSpec) -> Result<(), RunnerError> {
        let EntryPoint::State(key) = &spec.entry else {
            return Err(RunnerError(format!(
                "{} is not an entry point a libretro core can take",
                spec.stage
            )));
        };
        let path = PathBuf::from(&key.0);
        let data = fs::read(&path)
            .map_err(|e| RunnerError(format!("cannot read entry state {}: {e}", path.display())))?;
        self.core.load_state(&data).map_err(RunnerError)?;
        self.watcher.reset();
        // Stage entry is frame 0. Every duration reported from here is measured
        // from this point, which is what makes two players comparable at all
        // (ADR 0005).
        self.frame = 0;
        self.events.clear();
        self.events.push(SessionEvent::StageEntered { frame: 0, stage: spec.stage.clone() });
        println!(
            "[stage] entered {} from {} ({} bytes); frame counter reset to 0",
            spec.stage,
            path.display(),
            data.len()
        );
        Ok(())
    }

    fn advance(&mut self) -> Result<&[SessionEvent], RunnerError> {
        self.core.run_frame();
        self.frame += 1;
        self.events.clear();
        if let Some(ram) = self.core.memory(RETRO_MEMORY_SYSTEM_RAM) {
            // The watcher borrows RAM out of the core, so what it produces is
            // copied before the core is touched again.
            let observed = self.watcher.tick(ram, self.frame);
            self.events.extend_from_slice(observed);
        }
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
