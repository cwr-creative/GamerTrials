//! The libretro adapter: one manifest, one core, one stage at a time.
//!
//! This is the [`StageRunner`] ADR 0012 said to extract once there were two
//! implementations to read it off. There are, and the interface is what
//! survived both: enter a stage, advance a frame, be told what happened. Every
//! word either harness used that only made sense for an emulator — savestate,
//! core, WRAM, `retro_run` — stopped at this line (invariant 3).

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use session_api::{
    AssetKey, EntryPoint, Networked, Packet, Presentable, RunnerError, SessionEvent, SharedRunner,
    Slot, SlotEvent, SlotInput, Snapshot, StageRunner, StageSpec, VideoFrame,
};

use crate::libretro::{
    self, Core, RETRO_DEVICE_JOYPAD, RETRO_DEVICE_KEYBOARD, RETRO_MEMORY_SAVE_RAM,
    RETRO_MEMORY_SYSTEM_RAM, VIDEO,
};
use crate::manifest::{Controller, Manifest, Netplay};
use crate::prep::Content;
use crate::statebind;
use crate::watch::Watcher;

pub struct LibretroRunner {
    manifest: Manifest,
    core: Core,
    watcher: Watcher,
    frame: u64,
    /// Whatever `advance` last observed. Held here because [`StageRunner`]
    /// hands back a borrow rather than an allocation per frame.
    events: Vec<SessionEvent>,
    /// The same, each tagged with whose it is, for a shared machine.
    slot_events: Vec<SlotEvent>,
    /// How many pads are plugged: one, or a bout's slots (ADR 0036).
    ports: u8,
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
    /// What this runner's content hashes to. A stage says which content it
    /// runs on, and a runner holding different bytes refuses to enter it rather
    /// than running the wrong machine.
    content_key: AssetKey,
    /// Whether a `State` entry may be rebound to the content in hand before it
    /// is restored (ADR 0019).
    ///
    /// Off unless stage prep says otherwise, and it is a fact rather than a
    /// preference: it is true exactly when the seeded value-substitution engine
    /// produced this content from the ROM the stage names, which is the only
    /// pairing the transplant is sound for. A state that names a different ROM
    /// is otherwise a mistake, and the core refusing it is the mistake being
    /// caught.
    rebind_state: bool,
    /// When `retro_run` last returned, for a core that keeps emulating after
    /// it does (`LibretroConfig::settle`).
    last_frame: Option<Instant>,
}

/// How long a threaded core is given to finish booting after the frame
/// [`LibretroRunner::start`] spends.
///
/// dosbox-pure carries on booting on its own thread after that frame returns,
/// and a state restored while it is still booting is restored without the
/// thread paused: the stage then starts a frame further on or not, by timing
/// alone. Measured on the input-replay spike, a replay that reached its first
/// entry sooner than its recording ran exactly one frame ahead from the start;
/// 50 ms of waiting was enough to stop it. Five times that, once per runner.
const BOOT_SETTLE: Duration = Duration::from_millis(250);

impl LibretroRunner {
    /// Loads the core and the ROM the manifest names, and runs one frame so
    /// the core is ready to be handed a state.
    ///
    /// `core_override` is the `--core` flag; without it the manifest's
    /// `core_dir` must hold exactly one core ([`CORE_SUFFIX`]). Ambiguity is an
    /// error, not a guess — picking the NES core for a SNES game by accident
    /// is a real possibility now that the library has two consoles in it.
    pub fn start(
        manifest: Manifest,
        core_override: Option<PathBuf>,
        content: Content,
        watched: Vec<usize>,
    ) -> Result<LibretroRunner, String> {
        let core_path = match core_override {
            Some(p) => p,
            None => find_core(&manifest.libretro.core_dir)?,
        };
        let Content { key: content_key, path: rom_path, bytes: rom, rebindable, title } = content;
        println!("[rom]  {title}  {content_key}");

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
        // A machine that can be shared seats its biggest bout's players, each
        // on a port of their own (ADR 0036). A core that calls its own state
        // incomplete cannot host one: rollback would restore a machine that
        // is not the one it saved.
        let mut ports = 1;
        if manifest.libretro.netplay == Some(Netplay::Rollback) {
            ports = manifest.max_slots();
            core.plug_ports(ports);
            if libretro::serialization_quirks() & libretro::QUIRK_INCOMPLETE != 0 {
                return Err(format!(
                    "{} says netplay = \"rollback\", but the core declares its serialization \
                     incomplete, which rollback cannot be built on (ADR 0036)",
                    manifest.title
                ));
            }
        }

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
            slot_events: Vec::new(),
            ports,
            av,
            rom,
            rom_path,
            device,
            scan_buf: Vec::new(),
            fresh: true,
            content_key,
            rebind_state: rebindable,
            last_frame: None,
        };

        // Some cores are not ready to unserialize until they have produced a
        // frame, so this one is spent before anything is loaded into them.
        // `fresh` survives it deliberately: one frame in, a DOS machine has not
        // finished its BIOS, and calling that "already booted" is what saves
        // the first `Named` entry a redundant fifteen-second boot.
        runner.run_core_frame();
        if !runner.manifest.libretro.settle.is_zero() {
            std::thread::sleep(BOOT_SETTLE);
        }

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
    ///
    /// Capture on the *base* ROM. A state taken while a generated ROM is loaded
    /// describes that ROM's board, and rebinding cannot undo that (ADR 0019).
    pub fn capture_state(&self) -> Result<Vec<u8>, String> {
        // At a frame boundary, so the state is one a replay could reach.
        self.settle();
        self.core.save_state()
    }

    /// Allow a `State` entry to be rebound to the loaded ROM before it is
    /// restored (ADR 0019). The state file itself is never written to.
    pub fn set_state_rebinding(&mut self, on: bool) {
        self.rebind_state = on;
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
        self.run_core_frame();
        self.fresh = false;
    }

    /// One `retro_run`, and when it returned.
    fn run_core_frame(&mut self) {
        self.core.run_frame();
        self.last_frame = Some(Instant::now());
    }

    /// Wait until a threaded core has finished the frame it started, if it
    /// may not have yet. Nothing for a core that emulates inside `retro_run`,
    /// and nothing for a paced caller, which has already waited longer than
    /// this for the frame's turn. Public for a tool that reads memory between
    /// frames — the replay spike's digest.
    pub fn settle(&self) {
        let settle = self.manifest.libretro.settle;
        if let Some(t) = self.last_frame.filter(|_| !settle.is_zero()) {
            if let Some(left) = settle.checked_sub(t.elapsed()) {
                std::thread::sleep(left);
            }
        }
    }

    /// Hand the watcher the machine as it stands and keep what it reports.
    fn observe(&mut self) {
        // An empty slice is the honest input for a core that publishes no
        // system RAM: `AddressSpace::read` then finds every byte through the
        // memory map, which is the route such a manifest declares anyway.
        let ram = self.core.memory(RETRO_MEMORY_SYSTEM_RAM).unwrap_or(&[]);
        // The watcher borrows RAM out of the core, so what it produces is
        // copied before the core is touched again.
        let observed = self.watcher.tick(ram, self.frame);
        let start = self.events.len();
        self.events.extend_from_slice(observed);
        let slots: Vec<Option<Slot>> = self.watcher.last_slots().to_vec();
        self.slot_events.extend(
            self.events[start..].iter().zip(slots).map(|(e, s)| SlotEvent { slot: s, event: e.clone() }),
        );
    }

    /// One frame of the machine, observed: what `advance` does, for every
    /// caller that advances.
    fn step(&mut self) {
        self.events.clear();
        self.slot_events.clear();
        self.frame += 1;
        if self.manifest.libretro.settle.is_zero() {
            self.run_core_frame();
            self.observe();
        } else {
            // A threaded core's `retro_run` shows the frame that finished
            // before it was called and starts the next before returning, so
            // straight after it memory is moving. The frame it is about to
            // show is the one that holds still now, once settled: read that,
            // then let the next one start — with this frame's input landing
            // at the frame boundary rather than wherever the thread had got
            // to. The facts are the picture's, as they are for a console.
            self.settle();
            self.observe();
            self.run_core_frame();
        }
        self.fresh = false;
    }

    /// What the last frame reported, each fact tagged with whose it is: the
    /// same facts `advance` returned, for a caller judging every slot of a
    /// machine that keeps itself in step (ADR 0036).
    pub fn slot_events(&self) -> &[SlotEvent] {
        &self.slot_events
    }

    /// Plug this many pads, for a sync test on a manifest that declares no
    /// bout. Port 0 keeps its device.
    pub fn plug(&mut self, ports: u8) {
        self.core.plug_ports(ports);
        self.ports = ports.clamp(1, libretro::MAX_PORTS);
    }
}

/// `RETRO_SAVESTATE_CONTEXT_RUNAHEAD_SAME_INSTANCE`: a state that stays in
/// this session and this address space, which a core may make fast.
const SAVESTATE_SAME_INSTANCE: u32 = 1;

impl SharedRunner for LibretroRunner {
    fn slots(&self) -> u8 {
        self.ports
    }

    fn set_input(&mut self, slot: Slot, input: SlotInput) {
        libretro::set_joypad(slot.0, input.0);
    }

    fn advance_slots(&mut self, replaying: bool) -> Result<&[SlotEvent], RunnerError> {
        libretro::set_replaying(replaying);
        // The watcher's own log is a player's running commentary; a replay is
        // not the player's.
        let log = self.watcher.log;
        if replaying {
            self.watcher.log = false;
        }
        self.step();
        self.watcher.log = log;
        libretro::set_replaying(false);
        Ok(&self.slot_events)
    }

    fn snapshot(&mut self) -> Result<Snapshot, RunnerError> {
        self.settle();
        libretro::set_savestate_context(SAVESTATE_SAME_INSTANCE);
        let state = self.core.save_state();
        libretro::set_savestate_context(0);
        let state = state.map_err(RunnerError)?;
        let observer = self.watcher.state();
        let mut bytes = Vec::with_capacity(8 + 4 + observer.len() + state.len());
        bytes.extend_from_slice(&self.frame.to_le_bytes());
        bytes.extend_from_slice(&(observer.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&observer);
        bytes.extend_from_slice(&state);
        Ok(Snapshot(bytes))
    }

    fn restore(&mut self, snapshot: &Snapshot) -> Result<(), RunnerError> {
        let b = &snapshot.0;
        if b.len() < 12 {
            return Err(RunnerError("snapshot is too short to be one of this runner's".into()));
        }
        let frame = u64::from_le_bytes(b[..8].try_into().expect("8 bytes"));
        let n = u32::from_le_bytes(b[8..12].try_into().expect("4 bytes")) as usize;
        let observer = b.get(12..12 + n).ok_or_else(|| RunnerError("snapshot is short".into()))?;
        let state = &b[12 + n..];
        self.settle();
        libretro::set_savestate_context(SAVESTATE_SAME_INSTANCE);
        let loaded = self.core.load_state(state);
        libretro::set_savestate_context(0);
        loaded.map_err(RunnerError)?;
        self.watcher.restore(observer).map_err(RunnerError)?;
        self.frame = frame;
        self.events.clear();
        self.slot_events.clear();
        self.last_frame = Some(Instant::now());
        Ok(())
    }

    fn digest(&self) -> Vec<u8> {
        // What the facts are read from, and the observer that reads them:
        // the spike proved these exact across machines; the core's own
        // state blob carries headers and host buffers it did not.
        let mut out = self.core.memory(RETRO_MEMORY_SYSTEM_RAM).unwrap_or(&[]).to_vec();
        out.extend(self.watcher.state());
        out
    }
}

/// A core that brings its own network, under the netpacket interface (ADR
/// 0036): the game's netcode keeps the machines in step, and this runner
/// hands its packets out and in. Slot numbers are the interface's client
/// ids: 0 hosts.
impl Networked for LibretroRunner {
    fn join(&mut self, slot: Slot, slots: u8) -> Result<(), RunnerError> {
        if self.manifest.libretro.netplay != Some(Netplay::Packets) {
            return Err(RunnerError(format!(
                "{} does not say netplay = \"packets\", so its machine is not shared this way",
                self.manifest.title
            )));
        }
        if !libretro::netpacket_supported() {
            return Err(RunnerError(
                "the core registered no network of its own (no netpacket interface)".into(),
            ));
        }
        if !libretro::netpacket_start(u16::from(slot.0)) {
            return Err(RunnerError("the core would not start its network".into()));
        }
        println!(
            "[net]  joined as client {} of {slots}{}",
            slot.0,
            libretro::netpacket_protocol().map(|p| format!(", protocol {p}")).unwrap_or_default()
        );
        Ok(())
    }

    fn deliver(&mut self, from: Slot, bytes: &[u8]) {
        libretro::netpacket_receive(bytes, u16::from(from.0));
    }

    fn outgoing(&mut self) -> Vec<Packet> {
        libretro::netpacket_poll();
        libretro::netpacket_outgoing()
            .into_iter()
            .map(|(to, flags, bytes)| Packet {
                to: if to == libretro::NETPACKET_BROADCAST { None } else { Some(Slot(to as u8)) },
                bytes,
                reliable: flags & libretro::NETPACKET_RELIABLE != 0,
            })
            .collect()
    }

    fn peer_joined(&mut self, slot: Slot) {
        if !libretro::netpacket_connected(u16::from(slot.0)) {
            eprintln!("[net]  the core refused client {}", slot.0);
        }
    }

    fn peer_left(&mut self, slot: Slot) {
        libretro::netpacket_disconnected(u16::from(slot.0));
    }
}

impl StageRunner for LibretroRunner {
    fn enter(&mut self, spec: &StageSpec) -> Result<(), RunnerError> {
        // A runner is per (game, content). Entering a stage that runs on
        // something else is not a thing to fix up in place: content changes at a
        // challenge boundary, where a new runner is started, and a mismatch here
        // means a stage was routed to the wrong machine (ADR 0020).
        if spec.content != self.content_key {
            return Err(RunnerError(format!(
                "{} runs on content {} and this runner holds {}; content changes at a                  challenge boundary, not inside one",
                spec.stage, spec.content, self.content_key
            )));
        }

        // A key the core still thinks is held would survive into the stage, and
        // a stage that starts with the player walking into a wall is not the
        // same stage for the next person.
        libretro::keys_clear();
        libretro::pads_clear();

        // A threaded core restored mid-frame resumes the rest of that frame on
        // the restored machine, from wherever host timing had got it to. The
        // keys just released reach it now, at that boundary and before the
        // restore, as they always have.
        self.settle();
        libretro::deliver_keys();

        match &spec.entry {
            EntryPoint::State(key) => {
                let path = PathBuf::from(&key.0);
                let mut data = fs::read(&path).map_err(|e| {
                    RunnerError(format!("cannot read entry state {}: {e}", path.display()))
                })?;
                // A generated ROM is a different ROM to the one this state was
                // captured on. Whether that needs saying is the state format's
                // business, not this one's: Mesen checks the pairing and is
                // given the answer it wants (ADR 0019) — in memory, so the file
                // stays bound to the base ROM and stays the shipped asset —
                // while snes9x records no ROM identity and there is nothing to
                // do. Both are reported; neither is assumed.
                if self.rebind_state {
                    match statebind::reconcile(&data, &self.rom) {
                        Ok(statebind::Pairing::Rebound(rebound)) => {
                            println!(
                                "[state] rebound from ROM {}… to {}… ({}); \
                                 the file on disk is untouched",
                                &rebound.from[..8],
                                &rebound.to[..8],
                                self.rom_path.display()
                            );
                            data = rebound.state;
                        }
                        Ok(statebind::Pairing::NoIdentity(format)) => {
                            println!(
                                "[state] a {format} state records no ROM identity, so this one \
                                 restores onto a derived ROM unchanged"
                            );
                        }
                        Ok(statebind::Pairing::Bound) => {}
                        Err(e) => {
                            return Err(RunnerError(format!(
                                "cannot reconcile this state with the ROM it will run on: {e}"
                            )))
                        }
                    }
                }
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
                    self.run_core_frame();
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
        self.slot_events.clear();
        self.events.push(SessionEvent::StageEntered { frame: 0, stage: spec.stage.clone() });
        Ok(())
    }

    fn advance(&mut self) -> Result<&[SessionEvent], RunnerError> {
        self.step();
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

    fn tap_audio(&self, on: bool) {
        let mut tap = match crate::libretro::AUDIO_TAP.lock() {
            Ok(t) => t,
            Err(e) => e.into_inner(),
        };
        if on {
            tap.get_or_insert_with(Vec::new);
        } else {
            *tap = None;
        }
    }

    fn take_audio(&self, f: &mut dyn FnMut(&[i16], u32)) {
        let mut tap = match crate::libretro::AUDIO_TAP.lock() {
            Ok(t) => t,
            Err(e) => e.into_inner(),
        };
        if let Some(samples) = tap.as_mut() {
            if !samples.is_empty() {
                f(samples, self.av.timing.sample_rate.round() as u32);
                samples.clear();
            }
        }
    }
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

/// What a libretro core's file name ends in on this platform. A core
/// directory may hold one build per platform side by side; each platform only
/// ever sees its own.
#[cfg(windows)]
pub const CORE_SUFFIX: &str = "_libretro.dll";
#[cfg(target_os = "macos")]
pub const CORE_SUFFIX: &str = "_libretro.dylib";
#[cfg(not(any(windows, target_os = "macos")))]
pub const CORE_SUFFIX: &str = "_libretro.so";

/// Exactly one core in the manifest's core directory, or an error naming what
/// it found.
pub fn find_core(dir: &Path) -> Result<PathBuf, String> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
            if name.ends_with(CORE_SUFFIX) {
                found.push(path);
            }
        }
    }
    match found.len() {
        1 => Ok(found.remove(0)),
        0 => Err(format!(
            "no core found. Put a *{CORE_SUFFIX} in {}/ or pass --core <PATH>",
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
