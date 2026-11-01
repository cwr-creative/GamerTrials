//! Input replay, as a spike: does a second machine fed the same inputs play the
//! same game?
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
    Digest { regions, video: video.0, audio: audio.0, events: events.0 }
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
) -> Result<Report, String> {
    let labels = log.regions();
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
