//! Input replay: what a spectator is sent (ADR 0037), and the spike that
//! measured whether a second machine fed the same inputs plays the same game.
//!
//! For spectating, a frame's [`Input`] travels as [`encode_frames`] writes it —
//! a few bytes, opaque to everything outside this crate — with a
//! [`picture_digest`] every so often for the watcher to check its own against.
//! The rest of this module is the spike's harness: logs, digests and reports.
//!
//! ADR 0003 rejected spectating by replicated input because two machines'
//! emulators can diverge silently, and ADR 0034 left the question open once
//! Doom was emulated (ADR 0018). This module is how it gets measured rather
//! than argued: record what the core was told every frame and a digest of the
//! machine after it, replay the inputs somewhere else, and report the first
//! frame the digests disagree.
//!
//! A frame records everything `input_state` and the keyboard callback answer
//! from — the pad mask, the key edges in order, the latched mouse — and nothing
//! else, because that is all a spectator would be sent. It digests every region
//! the core publishes that a game can write (system RAM too, when the core
//! offers it), the picture, and the sound, each apart, so a divergence says
//! *where*: a picture that drifts while RAM holds is a presentation problem;
//! RAM that drifts is a different game.
//!
//! Every re-entry restores the stage's entry state, so the log marks each one
//! and a replay re-enters at the same frame. That also means a divergence can
//! be seen ending: the next attempt starts from the same bytes on both sides.
//!
//! The log is text, one line per frame, so two can be read side by side.

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::Ordering;
use std::time::Duration;

use session_api::{Presentable, SessionEvent, StageRunner, StageSpec};

use crate::keymap::{self, RetroButton};
use crate::libretro;
use crate::manifest::{Controller, Manifest};
use crate::runner::LibretroRunner;

/// The first line of every log, and its format version.
pub const MAGIC: &str = "gamertrials-replay 1";

/// What the core could read in one frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Input {
    pub joypad: u32,
    /// Key edges since the frame before, oldest first: down, `RETROK_*` code.
    pub keys: Vec<(bool, u32)>,
    /// Motion since the frame before, then held buttons.
    pub mouse: (i32, i32, u32),
}

// What a frame's input holds, as the first byte of its encoding says.
const HAS_PAD: u8 = 1 << 0;
const HAS_KEYS: u8 = 1 << 1;
const HAS_MOUSE: u8 = 1 << 2;
/// Set on a key code when the edge is a press.
const KEY_DOWN: u16 = 1 << 15;

impl Input {
    /// Appends this frame: a byte saying what follows, then only what is
    /// there — the pad mask (`u16`), the key edges (a count, then each code
    /// with its top bit set for a press), the mouse (`i16` motion, `u16`
    /// buttons). All little-endian. An idle console frame is one byte.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let (dx, dy, buttons) = self.mouse;
        let mut flags = 0;
        if self.joypad != 0 {
            flags |= HAS_PAD;
        }
        if !self.keys.is_empty() {
            flags |= HAS_KEYS;
        }
        if dx != 0 || dy != 0 || buttons != 0 {
            flags |= HAS_MOUSE;
        }
        out.push(flags);
        if flags & HAS_PAD != 0 {
            out.extend_from_slice(&(self.joypad as u16).to_le_bytes());
        }
        if flags & HAS_KEYS != 0 {
            // Never more than a frame's worth of a keyboard; more would be a
            // stuck event loop, and the rest are the next frame's anyway.
            let keys = &self.keys[..self.keys.len().min(255)];
            out.push(keys.len() as u8);
            for &(down, code) in keys {
                let code = (code as u16 & !KEY_DOWN) | if down { KEY_DOWN } else { 0 };
                out.extend_from_slice(&code.to_le_bytes());
            }
        }
        if flags & HAS_MOUSE != 0 {
            let clamp = |v: i32| v.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            out.extend_from_slice(&clamp(dx).to_le_bytes());
            out.extend_from_slice(&clamp(dy).to_le_bytes());
            out.extend_from_slice(&(buttons as u16).to_le_bytes());
        }
    }

    /// Reads one frame from `bytes` at `*at`, and moves `*at` past it.
    pub fn decode(bytes: &[u8], at: &mut usize) -> Result<Input, String> {
        let mut take = |n: usize| -> Result<&[u8], String> {
            let b = bytes.get(*at..*at + n).ok_or("input ends mid-frame")?;
            *at += n;
            Ok(b)
        };
        let u16_at = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
        let flags = take(1)?[0];
        if flags & !(HAS_PAD | HAS_KEYS | HAS_MOUSE) != 0 {
            return Err(format!("a frame flagged {flags:#04x}, which this build cannot read"));
        }
        let mut input = Input::default();
        if flags & HAS_PAD != 0 {
            input.joypad = u32::from(u16_at(take(2)?));
        }
        if flags & HAS_KEYS != 0 {
            let n = take(1)?[0];
            for _ in 0..n {
                let code = u16_at(take(2)?);
                input.keys.push((code & KEY_DOWN != 0, u32::from(code & !KEY_DOWN)));
            }
        }
        if flags & HAS_MOUSE != 0 {
            let dx = u16_at(take(2)?) as i16;
            let dy = u16_at(take(2)?) as i16;
            let buttons = u16_at(take(2)?);
            input.mouse = (i32::from(dx), i32::from(dy), u32::from(buttons));
        }
        Ok(input)
    }
}

/// Frames one after another, as a spectator is sent them.
pub fn encode_frames(frames: &[Input]) -> Vec<u8> {
    let mut out = Vec::with_capacity(frames.len() * 2);
    for f in frames {
        f.encode(&mut out);
    }
    out
}

/// Every frame in `bytes`, which must hold exactly `count`.
pub fn decode_frames(bytes: &[u8], count: usize) -> Result<Vec<Input>, String> {
    let mut at = 0;
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        out.push(Input::decode(bytes, &mut at)?);
    }
    if at != bytes.len() {
        return Err(format!("{} bytes left over after {count} frames", bytes.len() - at));
    }
    Ok(out)
}

/// The format of [`save_state`]'s bytes.
const STATE_FORMAT: u8 = 1;

/// The machine as a watcher on another machine can resume it (ADR 0037): the
/// input the frame just run was given — pad, mouse buttons, keys held — then
/// the runner's portable snapshot: its frame, its watcher's state, the
/// core's. Taken at a frame boundary.
pub fn save_state(runner: &mut LibretroRunner) -> Result<Vec<u8>, String> {
    let pad = libretro::JOYPAD.load(Ordering::Relaxed);
    let (_, _, buttons) = libretro::mouse();
    let keys = libretro::keys_held();
    let snapshot = runner.snapshot_portable().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity(8 + keys.len() * 2 + snapshot.0.len());
    out.push(STATE_FORMAT);
    out.extend_from_slice(&(pad as u16).to_le_bytes());
    out.extend_from_slice(&(buttons as u16).to_le_bytes());
    out.push(keys.len().min(255) as u8);
    for code in keys.iter().take(255) {
        out.extend_from_slice(&(*code as u16).to_le_bytes());
    }
    out.extend_from_slice(&snapshot.0);
    Ok(out)
}

/// Resumes `runner` where [`save_state`] left another machine's, and returns
/// the frame that was.
///
/// A core may keep notes on its input outside the state it saves:
/// dosbox-pure keeps which keys and buttons are down, passes a release on
/// only for one it has seen pressed, and turns a change in the pad or the
/// mouse into an event by comparing with the last frame. Restored with this
/// machine's notes, the other's next release is lost and the game goes
/// another way. So first this machine is given the other's input and run one
/// frame, which brings the notes into line and spends the events they raise
/// on a machine about to be replaced; then the state replaces it.
pub fn restore_state(runner: &mut LibretroRunner, bytes: &[u8]) -> Result<u64, String> {
    let head = bytes.get(..6).ok_or("a state cut short")?;
    if head[0] != STATE_FORMAT {
        return Err(format!("a state in format {}, which this build cannot read", head[0]));
    }
    let pad = u32::from(u16::from_le_bytes([head[1], head[2]]));
    let buttons = u32::from(u16::from_le_bytes([head[3], head[4]]));
    let n = head[5] as usize;
    let keys = bytes.get(6..6 + 2 * n).ok_or("a state cut short in its keys")?;
    let keys: Vec<u32> = keys.chunks_exact(2).map(|k| u32::from(u16::from_le_bytes([k[0], k[1]]))).collect();
    let snapshot = session_api::Snapshot(bytes[6 + 2 * n..].to_vec());
    libretro::keys_clear();
    for &code in &keys {
        libretro::key_down(code);
    }
    libretro::JOYPAD.store(pad, Ordering::Relaxed);
    libretro::set_mouse(0, 0, buttons);
    runner.run_frame_raw();
    runner.restore_portable(&snapshot).map_err(|e| e.to_string())?;
    Ok(runner.frame())
}

/// The picture the core last presented, as a number: its size and every pixel.
/// What a watcher compares with the player's to know it is still watching the
/// same game (ADR 0037). It is the picture rather than memory because the
/// picture is what a watcher sees, and because a threaded core's memory moves
/// after `retro_run` returns while the picture it handed over does not.
pub fn picture_digest(runner: &LibretroRunner) -> u64 {
    let mut video = Fx::new(0);
    // `u64::MAX` is never a sequence number, so the current picture is handed
    // over whether or not it is new.
    runner.present(u64::MAX, &mut |f| {
        video.word(f.width as u64);
        video.word(f.height as u64);
        for p in f.pixels {
            video.word(u64::from(*p));
        }
    });
    video.0
}

/// The machine after one frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Digest {
    /// One per label in [`region_labels`], in that order.
    pub regions: Vec<u64>,
    pub video: u64,
    pub audio: u64,
    /// The telemetry the manifest's rules emitted this frame: the facts a run
    /// is judged on and a watcher is shown, whatever else in memory differs.
    pub events: u64,
}

impl Digest {
    /// The labels of every part that differs from `other`.
    fn differs(&self, other: &Digest, labels: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for (i, label) in labels.iter().enumerate() {
            if self.regions.get(i) != other.regions.get(i) {
                out.push(label.clone());
            }
        }
        if self.video != other.video {
            out.push("video".into());
        }
        if self.audio != other.audio {
            out.push("audio".into());
        }
        if self.events != other.events {
            out.push("events".into());
        }
        out
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// The stage's entry point was entered; frames count from here.
    Enter,
    Frame(Input, Digest),
}

/// A parsed log.
pub struct Log {
    /// Everything between the magic line and `---`, in order.
    pub header: Vec<(String, String)>,
    pub steps: Vec<Step>,
}

impl Log {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.header.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    /// The region labels the recording digested, from its header.
    pub fn regions(&self) -> Vec<String> {
        match self.get("regions") {
            Some("") | None => Vec::new(),
            Some(v) => v.split(' ').map(str::to_string).collect(),
        }
    }

    pub fn read(path: &Path) -> Result<Log, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        Log::parse(&text).map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn parse(text: &str) -> Result<Log, String> {
        let mut lines = text.lines().enumerate();
        match lines.next() {
            Some((_, l)) if l.trim_end() == MAGIC => {}
            _ => return Err(format!("not a replay log: the first line is not `{MAGIC}`")),
        }
        let mut header = Vec::new();
        for (n, line) in lines.by_ref() {
            let line = line.trim_end();
            if line == "---" {
                break;
            }
            let (k, v) = line.split_once(' ').unwrap_or((line, ""));
            if k.is_empty() {
                return Err(format!("line {}: an empty header line", n + 1));
            }
            header.push((k.to_string(), v.to_string()));
        }
        let mut steps = Vec::new();
        for (n, line) in lines {
            let line = line.trim_end();
            if line.is_empty() {
                continue;
            }
            steps.push(parse_step(line).map_err(|e| format!("line {}: {e}", n + 1))?);
        }
        Ok(Log { header, steps })
    }
}

fn parse_step(line: &str) -> Result<Step, String> {
    if line == "enter" {
        return Ok(Step::Enter);
    }
    let mut t = line.split(' ');
    if t.next() != Some("f") {
        return Err(format!("expected `enter` or a frame, got `{line}`"));
    }
    let mut next = |what: &str| t.next().ok_or_else(|| format!("a frame with no {what}"));
    let joypad = u32::from_str_radix(next("pad")?, 16).map_err(|e| format!("pad: {e}"))?;
    let m: Vec<&str> = next("mouse")?.split(',').collect();
    let mouse = match m.as_slice() {
        [x, y, b] => (
            x.parse().map_err(|e| format!("mouse x: {e}"))?,
            y.parse().map_err(|e| format!("mouse y: {e}"))?,
            u32::from_str_radix(b, 16).map_err(|e| format!("mouse buttons: {e}"))?,
        ),
        _ => return Err("mouse wants x,y,buttons".into()),
    };
    let keys = match next("keys")? {
        "." => Vec::new(),
        list => list
            .split(',')
            .map(|e| {
                let (down, code) = match e.split_at(1) {
                    ("+", c) => (true, c),
                    ("-", c) => (false, c),
                    _ => return Err(format!("key edge `{e}` is neither +code nor -code")),
                };
                Ok((down, code.parse().map_err(|_| format!("key edge `{e}`"))?))
            })
            .collect::<Result<_, String>>()?,
    };
    let hex = |s: &str, what: &str| u64::from_str_radix(s, 16).map_err(|e| format!("{what}: {e}"));
    let video = hex(next("video digest")?, "video")?;
    let audio = hex(next("audio digest")?, "audio")?;
    let events = hex(next("events digest")?, "events")?;
    let regions = match t.next() {
        None | Some("") => Vec::new(),
        Some(list) => list.split(',').map(|r| hex(r, "region")).collect::<Result<_, _>>()?,
    };
    Ok(Step::Frame(Input { joypad, keys, mouse }, Digest { regions, video, audio, events }))
}

fn write_step(out: &mut impl Write, step: &Step) -> std::io::Result<()> {
    match step {
        Step::Enter => writeln!(out, "enter"),
        Step::Frame(input, d) => {
            let keys = if input.keys.is_empty() {
                ".".to_string()
            } else {
                input
                    .keys
                    .iter()
                    .map(|(down, code)| format!("{}{code}", if *down { '+' } else { '-' }))
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let regions: Vec<String> = d.regions.iter().map(|r| format!("{r:016x}")).collect();
            let (x, y, b) = input.mouse;
            writeln!(
                out,
                "f {:x} {x},{y},{b:x} {keys} {:016x} {:016x} {:016x} {}",
                input.joypad,
                d.video,
                d.audio,
                d.events,
                regions.join(",")
            )
        }
    }
}

/// Writes a log as it is played.
pub struct Recorder {
    out: BufWriter<File>,
    frames: u64,
    attempts: u32,
}

impl Recorder {
    pub fn create(path: &Path, header: &[(String, String)]) -> Result<Recorder, String> {
        let file =
            File::create(path).map_err(|e| format!("cannot create {}: {e}", path.display()))?;
        let mut out = BufWriter::new(file);
        let io = |e: std::io::Error| format!("cannot write {}: {e}", path.display());
        writeln!(out, "{MAGIC}").map_err(io)?;
        for (k, v) in header {
            writeln!(out, "{k} {v}").map_err(io)?;
        }
        writeln!(out, "---").map_err(io)?;
        Ok(Recorder { out, frames: 0, attempts: 0 })
    }

    /// The stage was just entered. Key edges the entry made — it releases
    /// whatever was held — are the entry's, not the next frame's input, and a
    /// replay's own entry makes them again.
    pub fn entered(&mut self) -> Result<(), String> {
        libretro::take_key_edges();
        self.attempts += 1;
        write_step(&mut self.out, &Step::Enter).map_err(|e| e.to_string())
    }

    pub fn frame(&mut self, input: Input, digest: Digest) -> Result<(), String> {
        self.frames += 1;
        write_step(&mut self.out, &Step::Frame(input, digest)).map_err(|e| e.to_string())
    }

    /// Frames and attempts written.
    pub fn finish(mut self) -> Result<(u64, u32), String> {
        self.out.flush().map_err(|e| e.to_string())?;
        Ok((self.frames, self.attempts))
    }
}

/// Wait after a frame, before the machine is digested or told the next
/// frame's input.
///
/// For a core that emulates on a thread of its own and lets it run on after
/// `retro_run` returns. dosbox-pure does: `retro_run` reads the input, waits
/// for the frame in flight, and starts the next one before returning, and its
/// thread takes queued input whenever it next looks — mid-frame. So where a
/// key lands in emulated time depends on how far the thread got before the
/// key arrived, and memory read after `retro_run` is read while it changes.
/// Waiting longer than the thread takes to finish a frame parks it at the
/// frame boundary on both sides of a replay. A diagnostic, not a fix: a
/// player's frontend cannot wait like this.
pub fn settle(d: Duration) {
    if !d.is_zero() {
        std::thread::sleep(d);
    }
}

/// Start keeping what a recording or a replay reads: the key edges, and the
/// sound.
pub fn arm(runner: &LibretroRunner, recording: bool) {
    libretro::record_key_edges(recording);
    runner.tap_audio(true);
}

/// What the core will read in the frame about to run. Call after the input is
/// set and before `advance`.
pub fn capture_input() -> Input {
    Input {
        joypad: libretro::JOYPAD.load(Ordering::Relaxed),
        keys: libretro::take_key_edges(),
        mouse: libretro::mouse(),
    }
}

/// Tell the core what a recording says it was told.
pub fn apply_input(input: &Input) {
    libretro::JOYPAD.store(input.joypad, Ordering::Relaxed);
    for &(down, code) in &input.keys {
        if down {
            libretro::key_down(code);
        } else {
            libretro::key_up(code);
        }
    }
    let (x, y, b) = input.mouse;
    libretro::set_mouse(x, y, b);
}

/// What each region digest is called: `ram` for the system-RAM slice, then
/// `start+len` in hex for each writable published region, then `scan:` and the
/// manifest's scan window when it declares one.
///
/// The scan window is digested apart because it is where a game's own
/// variables were found — Doom's player struct and game globals — so it alone
/// answers whether the *game* matched when a whole region did not: a DOS
/// machine's 16 MB also holds sound buffers and driver state that never reach
/// the screen.
pub fn region_labels(runner: &LibretroRunner) -> Vec<String> {
    let mut labels = Vec::new();
    if runner.system_ram().is_some_and(|r| !r.is_empty()) {
        labels.push("ram".to_string());
    }
    libretro::with_writable_regions(&mut |start, bytes| {
        labels.push(format!("{start:x}+{:x}", bytes.len()));
    });
    if let Some(w) = runner.manifest().libretro.scan {
        labels.push(format!("scan:{:x}+{:x}", w.at, w.size));
    }
    labels
}

/// The machine as it stands after a frame, and what that frame emitted.
pub fn digest(runner: &mut LibretroRunner, emitted: &[SessionEvent]) -> Digest {
    // `advance` ends by starting the next frame, which a threaded core is
    // still emulating; its manifest says how long to give it.
    runner.settle();
    let mut regions = Vec::new();
    if let Some(ram) = runner.system_ram().filter(|r| !r.is_empty()) {
        regions.push(hash_bytes(ram));
    }
    libretro::with_writable_regions(&mut |_, bytes| regions.push(hash_bytes(bytes)));
    if runner.manifest().libretro.scan.is_some() {
        if let Some((_, window)) = runner.scan_window() {
            regions.push(hash_bytes(window));
        }
    }
    let video = picture_digest(runner);
    let mut audio = Fx::new(0);
    runner.take_audio(&mut |samples, rate| {
        audio.word(u64::from(rate));
        for s in samples {
            audio.word(*s as u16 as u64);
        }
    });
    let mut events = Fx::new(0);
    for e in emitted {
        events.word(hash_bytes(e.to_string().as_bytes()));
    }
    Digest { regions, video, audio: audio.0, events: events.0 }
}

/// FxHash's step over 64-bit words. Not a cryptographic digest and not meant
/// to be: it only has to notice that two machines differ, and it has to keep
/// up with 16 MB of DOS memory every frame. Each step is a bijection of the
/// state for a given word, so a single differing word always shows.
struct Fx(u64);

impl Fx {
    const K: u64 = 0x517c_c1b7_2722_0a95;

    fn new(seed: u64) -> Fx {
        Fx(seed)
    }

    fn word(&mut self, w: u64) {
        self.0 = (self.0.rotate_left(5) ^ w).wrapping_mul(Self::K);
    }
}

pub fn hash_bytes(bytes: &[u8]) -> u64 {
    let mut h = Fx::new(bytes.len() as u64);
    let mut words = bytes.chunks_exact(8);
    for w in &mut words {
        h.word(u64::from_le_bytes(w.try_into().expect("chunks_exact(8)")));
    }
    for &b in words.remainder() {
        h.word(u64::from(b));
    }
    h.0
}

/// Plays in place of a person, so a recording can be made with nobody at the
/// keyboard: each button or key is pressed or released with a small chance
/// every frame, so presses last a while as a player's do. Start and select are
/// left alone — on a keyboard game they open the menu, from which random keys
/// can quit the game.
pub struct Script {
    rng: u64,
    pad: Vec<u32>,
    keys: Vec<u32>,
    mouse: bool,
    held_pad: u32,
    held_keys: BTreeSet<u32>,
    buttons: u32,
}

impl Script {
    pub fn new(seed: u64, manifest: &Manifest) -> Script {
        let playable = |b: &RetroButton| !matches!(b, RetroButton::Start | RetroButton::Select);
        let (pad, keys, mouse) = match manifest.libretro.controller {
            Controller::Pad(_) => (
                RetroButton::ALL.iter().filter(|b| playable(b)).map(|b| b.bit()).collect(),
                Vec::new(),
                false,
            ),
            Controller::Keyboard => {
                let keys: BTreeSet<u32> = manifest
                    .libretro
                    .pad_keys
                    .iter()
                    .filter(|(b, _)| playable(b))
                    .filter_map(|(_, name)| keymap::guest_key(name).map(|k| k.retrok))
                    .collect();
                (Vec::new(), keys.into_iter().collect(), true)
            }
        };
        Script {
            rng: seed,
            pad,
            keys,
            mouse,
            held_pad: 0,
            held_keys: BTreeSet::new(),
            buttons: 0,
        }
    }

    /// SplitMix64.
    fn next(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn flip(&mut self, one_in: u64) -> bool {
        self.next() % one_in == 0
    }

    /// Let go of everything, for a frame with nothing held.
    pub fn release_all(&mut self) {
        for code in std::mem::take(&mut self.held_keys) {
            libretro::key_up(code);
        }
        self.held_pad = 0;
        self.buttons = 0;
        libretro::JOYPAD.store(0, Ordering::Relaxed);
        if self.mouse {
            libretro::set_mouse(0, 0, 0);
        }
    }

    /// Set the input for the next frame.
    pub fn drive(&mut self) {
        for i in 0..self.pad.len() {
            if self.flip(16) {
                self.held_pad ^= self.pad[i];
            }
        }
        libretro::JOYPAD.store(self.held_pad, Ordering::Relaxed);
        for i in 0..self.keys.len() {
            if self.flip(16) {
                let code = self.keys[i];
                if !self.held_keys.remove(&code) {
                    self.held_keys.insert(code);
                    libretro::key_down(code);
                } else {
                    libretro::key_up(code);
                }
            }
        }
        if self.mouse {
            if self.flip(32) {
                self.buttons ^= 1 << 2; // MOUSE_LEFT
            }
            let dx = (self.next() % 13) as i32 - 6;
            let dy = (self.next() % 5) as i32 - 2;
            libretro::set_mouse(dx, dy, self.buttons);
        }
    }
}

/// How one attempt of a replay compared with the recording.
#[derive(Debug, Default)]
pub struct AttemptReport {
    pub frames: u64,
    /// The first frame (counted from entry) that differed, and in what.
    pub first: Option<(u64, Vec<String>)>,
    /// Frames that differed in anything.
    pub bad: u64,
    /// Whether the final frame matched again after a divergence.
    pub ended_in_step: bool,
}

/// How a whole replay compared, attempt by attempt and part by part.
#[derive(Debug, Default)]
pub struct Report {
    pub attempts: Vec<AttemptReport>,
    /// Per label, then `video` and `audio`: frames that differed.
    pub diverged: Vec<(String, u64)>,
    /// Set when the core asked to shut down before the log ran out.
    pub cut_short: Option<u64>,
}

impl Report {
    pub fn clean(&self) -> bool {
        self.cut_short.is_none() && self.attempts.iter().all(|a| a.first.is_none())
    }

    pub fn print(&self, title: &str) {
        let frames: u64 = self.attempts.iter().map(|a| a.frames).sum();
        let verdict = if self.clean() { "IN STEP" } else { "DIVERGED" };
        println!(
            "[replay] {title}: {verdict} - {frames} frames over {} attempt(s)",
            self.attempts.len()
        );
        for (i, a) in self.attempts.iter().enumerate() {
            match &a.first {
                None => println!("  attempt {}: {} frames, every one matched", i + 1, a.frames),
                Some((f, parts)) => println!(
                    "  attempt {}: {} frames; first differed at frame {f} in {}; {} differed in \
                     all; the last frame {}",
                    i + 1,
                    a.frames,
                    parts.join(", "),
                    a.bad,
                    if a.ended_in_step { "matched again" } else { "still differed" },
                ),
            }
        }
        let bad: Vec<String> = self
            .diverged
            .iter()
            .filter(|(_, n)| *n > 0)
            .map(|(l, n)| format!("{l} {n}"))
            .collect();
        if !bad.is_empty() {
            println!("  frames differing, by part: {}", bad.join(", "));
        }
        if let Some(at) = self.cut_short {
            println!("  the core asked to shut down after {at} frames; the rest was not replayed");
        }
    }
}

/// Play a log's inputs into `runner` from its first entry, and compare every
/// frame's digest with the one recorded. `settle` is waited after every frame;
/// see [`settle`].
pub fn replay(
    runner: &mut LibretroRunner,
    spec: &StageSpec,
    log: &Log,
    settle_for: Duration,
    resume: Option<&[u8]>,
) -> Result<Report, String> {
    let labels = log.regions();
    // With a state from the recording machine: restored straight after the
    // first entry, and the frames it already holds skipped rather than
    // played (ADR 0037's catch-up).
    let mut skip = 0u64;
    let mut resume = resume;
    let mut report = Report::default();
    let mut parts: Vec<(String, u64)> = labels.iter().cloned().map(|l| (l, 0)).collect();
    parts.push(("video".into(), 0));
    parts.push(("audio".into(), 0));
    parts.push(("events".into(), 0));
    let mut played = 0u64;
    for step in &log.steps {
        match step {
            Step::Enter => {
                runner.enter(spec).map_err(|e| e.to_string())?;
                // The recorder's own entry made the same releases, and threw
                // them away for the same reason.
                libretro::take_key_edges();
                report.attempts.push(AttemptReport::default());
                // The state belongs to the first attempt alone.
                skip = 0;
                if let Some(state) = resume.take() {
                    skip = restore_state(runner, state)?;
                    println!("[replay] resumed from a state at frame {skip}");
                }
            }
            Step::Frame(_, _) if skip > 0 && report.attempts.last().is_some_and(|a| a.frames < skip) => {
                let attempt = report.attempts.last_mut().expect("checked");
                attempt.frames += 1;
                played += 1;
            }
            Step::Frame(input, want) => {
                let attempt = report
                    .attempts
                    .last_mut()
                    .ok_or("the log has a frame before its first `enter`")?;
                if runner.shutdown_requested() {
                    report.cut_short = Some(played);
                    break;
                }
                apply_input(input);
                let events = runner.advance().map_err(|e| e.to_string())?.to_vec();
                settle(settle_for);
                played += 1;
                attempt.frames += 1;
                let got = digest(runner, &events);
                let differs = got.differs(want, &labels);
                attempt.ended_in_step = differs.is_empty();
                if differs.is_empty() {
                    continue;
                }
                attempt.bad += 1;
                for d in &differs {
                    if let Some(p) = parts.iter_mut().find(|(l, _)| l == d) {
                        p.1 += 1;
                    }
                }
                if attempt.first.is_none() {
                    attempt.first = Some((attempt.frames, differs));
                }
            }
        }
    }
    report.diverged = parts;
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_log_reads_back_as_written() {
        let steps = vec![
            Step::Enter,
            Step::Frame(
                Input { joypad: 0x81, keys: vec![], mouse: (0, 0, 0) },
                Digest { regions: vec![1, u64::MAX], video: 2, audio: 3, events: 0 },
            ),
            Step::Frame(
                Input { joypad: 0, keys: vec![(true, 273), (false, 306)], mouse: (-6, 2, 4) },
                Digest { regions: vec![4, 5], video: 6, audio: 0, events: 7 },
            ),
            Step::Enter,
        ];
        let mut text = format!("{MAGIC}\ngame doom\nregions 0+9e7a0 200000+f60000\n---\n");
        let mut body = Vec::new();
        for s in &steps {
            write_step(&mut body, s).unwrap();
        }
        text.push_str(std::str::from_utf8(&body).unwrap());
        let log = Log::parse(&text).unwrap();
        assert_eq!(log.steps, steps);
        assert_eq!(log.get("game"), Some("doom"));
        assert_eq!(log.regions(), vec!["0+9e7a0", "200000+f60000"]);
    }

    #[test]
    fn frames_read_back_as_encoded() {
        let frames = vec![
            Input::default(),
            Input { joypad: 0x8001, keys: vec![], mouse: (0, 0, 0) },
            Input { joypad: 0, keys: vec![(true, 273), (false, 306)], mouse: (-6, 2, 4) },
            Input { joypad: 0, keys: vec![], mouse: (40000, -40000, 1 << 9) },
        ];
        let bytes = encode_frames(&frames);
        // An idle frame is one byte; a pad-only frame three.
        assert_eq!(bytes[0], 0);
        assert_eq!(&bytes[1..4], &[HAS_PAD, 0x01, 0x80]);
        let back = decode_frames(&bytes, frames.len()).unwrap();
        assert_eq!(back[..3], frames[..3]);
        // Motion past what an i16 holds is clamped, not wrapped.
        assert_eq!(back[3].mouse, (i16::MAX as i32, i16::MIN as i32, 1 << 9));
    }

    #[test]
    fn frames_that_do_not_add_up_are_refused() {
        let bytes = encode_frames(&[Input { joypad: 1, ..Input::default() }]);
        assert!(decode_frames(&bytes, 2).is_err(), "fewer frames than said");
        assert!(decode_frames(&bytes[..2], 1).is_err(), "a frame cut short");
        assert!(decode_frames(&[0, 0], 1).is_err(), "bytes left over");
        assert!(decode_frames(&[0x80], 1).is_err(), "a flag from a newer build");
    }

    #[test]
    fn a_log_without_the_magic_line_is_refused() {
        assert!(Log::parse("game doom\n---\n").is_err());
    }

    #[test]
    fn one_differing_word_always_changes_the_hash() {
        let a = vec![0u8; 4096];
        for at in [0, 7, 8, 2048, 4095] {
            let mut b = a.clone();
            b[at] = 1;
            assert_ne!(hash_bytes(&a), hash_bytes(&b), "byte {at}");
        }
        assert_ne!(hash_bytes(&a[..4095]), hash_bytes(&a), "length counts");
    }
}
