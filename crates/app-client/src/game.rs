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

use std::collections::{BTreeMap, VecDeque};
use std::net::{SocketAddr, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use challenge_core::{Bout, Next, Run, RunResult};
use head_to_head::relay::RelayState;
use head_to_head::{Relay, Rollback, Settings as RollbackSettings, State as RollbackState, Udp};
use minifb::{Scale, ScaleMode, Window, WindowOptions};
use session_api::{ChallengeKind, Networked, Presentable, SessionEvent, Slot, SlotInput, StageRunner, StageSpec};
use session_libretro::audio::{self, Audio};
use session_libretro::input::PadState;
use session_libretro::libretro;
use session_libretro::manifest::{Controller, Manifest, Netplay, PadLayout};
use session_libretro::runner::LibretroRunner;
use session_libretro::{input, prep, replay};

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
    /// A bout (ADR 0036): this player's slot on the shared machine, and how
    /// to find the others.
    pub bout: Option<BoutLoad>,
    /// Press random buttons from this seed instead of reading the pad, for a
    /// hand test with nobody at the keyboard.
    pub script: Option<u64>,
    /// Give the run up this many seconds of play in, for a hand test of
    /// spectating with nobody at the keyboard.
    pub give_up_after: Option<f64>,
}

/// Someone else's run, to replay on this machine's core (ADR 0037): the
/// round as this player would prepare it, and whose input to replay on it.
pub struct Watching {
    pub load: Load,
    pub player: String,
}

pub struct BoutLoad {
    pub slot: u8,
    pub slots: u8,
    /// The server's UDP side, which this machine probes so the server learns
    /// where it is. `None` on a server that deals no bouts, which never deals
    /// this one.
    pub rendezvous: Option<SocketAddr>,
    /// Frames this player's own input is delayed (Settings).
    pub input_delay: u8,
    /// The UDP port to take, so a router can forward it (Settings); 0 for any.
    pub port: u16,
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
    /// A bout: where each other slot's machine might be (ADR 0036). The
    /// session starts on this and `Loaded` follows once it has synchronised.
    Peers(Vec<(u8, Vec<SocketAddr>)>),
    /// A bout: the player in that slot gave up on that frame, as the server
    /// judged it; whoever stays wins.
    PeerForfeited { slot: u8, frame: u64 },
    /// Replay another player's run here, or stop with `None` (ADR 0037). The
    /// round already loaded is used when it is the same one — a player whose
    /// own run is over — and loaded otherwise.
    Watch(Option<Box<Watching>>),
    /// Some of the watched run's input, as the server relayed it.
    Inputs {
        player: String,
        attempt: u32,
        from: u64,
        count: u32,
        frames: Vec<u8>,
        checks: Vec<(u64, u64)>,
        artifact: Option<String>,
    },
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
    /// A run's input, `count` frames of `attempt` from `from`, as the
    /// adapter encodes it, with (frame, picture digest) checks among them:
    /// what a spectator replays the run from (ADR 0037). Sent for every run,
    /// watched or not.
    Inputs { round: u64, attempt: u32, from: u64, count: u32, frames: Vec<u8>, checks: Vec<(u64, u64)> },
    /// A bout: this machine's UDP side is up and probing the server with
    /// `token`; `local` is the address it would be at on its own network.
    Endpoint { round: u64, token: u64, local: String },
    /// A bout: the link to the other slot died on this side, on the last
    /// frame confirmed. No claim follows; the server rules.
    PeerLost { round: u64, frame: u64 },
    /// How replaying a watched run is going, whenever that changes.
    Replay { round: u64, player: String, state: ReplayState },
    Closed,
}

/// Where a replay of someone else's run stands (ADR 0037).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplayState {
    /// Nothing to play yet: the run has not begun, or no input has come.
    Waiting,
    /// Running the attempt so far, unpaced and silent, to reach it.
    CatchingUp { done: u64, of: u64 },
    Live,
    /// This machine is not playing the same game, or cannot: why.
    Lost(String),
}

impl ReplayState {
    /// A word for the page to switch on, and a line to show.
    pub fn describe(&self) -> (&'static str, String) {
        match self {
            ReplayState::Waiting => ("waiting", "waiting for the run".into()),
            ReplayState::CatchingUp { done, of } => {
                ("catching_up", format!("catching up: {}%", done * 100 / (*of).max(1)))
            }
            ReplayState::Live => ("live", String::new()),
            ReplayState::Lost(why) => ("lost", why.clone()),
        }
    }
}

/// The runner alone, or inside the bout's rollback session once the other
/// machines are found (ADR 0036).
enum Machine {
    Solo(Box<LibretroRunner>),
    Bout(Box<Rollback<LibretroRunner>>),
    /// A machine with a network of its own, relayed (`netplay = "packets"`).
    Relay(Box<Relay<LibretroRunner, Udp>>),
    /// Between the two, for the length of a move.
    Moving,
}

impl Machine {
    fn runner(&self) -> &LibretroRunner {
        match self {
            Machine::Solo(r) => r,
            Machine::Bout(s) => s.machine(),
            Machine::Relay(r) => r.machine(),
            Machine::Moving => unreachable!("the runner is being moved"),
        }
    }

    fn runner_mut(&mut self) -> &mut LibretroRunner {
        match self {
            Machine::Solo(r) => r,
            Machine::Bout(s) => s.machine_mut(),
            Machine::Relay(r) => r.machine_mut(),
            Machine::Moving => unreachable!("the runner is being moved"),
        }
    }
}

/// How long a bout's machines are given to find each other before the
/// round is given up as unreachable (ADR 0029 voids it).
const SYNC_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a probe goes to the server's UDP side until the peers are known.
const PROBE_EVERY: Duration = Duration::from_millis(200);
/// A bout's client reports an empty frame this often, so the server's judge
/// hears from every slot (ADR 0036).
const HEARTBEAT: u64 = 15;

/// This player's half of a bout (ADR 0036).
struct BoutState {
    slot: Slot,
    slots: u8,
    /// The machine brings its own network and is relayed, not rolled back.
    packets: bool,
    rendezvous: Option<SocketAddr>,
    token: u64,
    input_delay: u8,
    /// The UDP side, until the session takes it.
    udp: Option<Udp>,
    /// The provisional judge over every slot's facts.
    judge: Bout,
    /// Held until the machines have found each other: `Prepared` means
    /// ready to go, and a bout is not until then.
    artifact: String,
    last_probe: Option<Instant>,
    endpoint_sent: bool,
    /// When the session started looking for the others.
    syncing_since: Option<Instant>,
    loaded_sent: bool,
    lost_sent: bool,
}

/// The game being run right now, and the run being judged on it.
struct Loaded {
    round: u64,
    machine: Machine,
    /// Present for a bout.
    bout: Option<BoutState>,
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
    /// Random buttons, when the round was loaded with a script.
    script: Option<(u64, u32)>,
    /// The frames of this run's input not yet sent (ADR 0037).
    outgoing: Outgoing,
    /// What this round's content hashes to.
    artifact: String,
    /// The frame on the run's clock to give up on, from `give_up_after`.
    give_up_at: Option<u64>,
    /// Someone else's run, replayed on this core instead of playing one.
    replica: Option<Replica>,
}

/// Frames a replay keeps in hand before it plays live: two batches, so one
/// late on the wire does not stall it.
const LEAD: u64 = 2 * BATCH as u64;
/// Further behind than this and a replay catches up, unpaced and silent:
/// three leads, so the jitter of batches on the wire never trips it, while a
/// watcher who arrives a second behind is brought up to the lead rather than
/// left to play the whole round that far back.
const CATCH_UP: u64 = 3 * LEAD;
/// How long one pass of catching up may hold the window before it is
/// pumped and painted again.
const CATCH_UP_SLICE: Duration = Duration::from_millis(30);

/// One attempt of a watched run, as far as its input has come.
struct Attempt {
    n: u32,
    inputs: Vec<replay::Input>,
    checks: BTreeMap<u64, u64>,
}

/// Another player's run, replayed here from its input (ADR 0037).
struct Replica {
    player: String,
    /// Oldest first; the front is the one being played. A later one waits
    /// until the front has played out, so a death does not cut it short.
    attempts: VecDeque<Attempt>,
    /// Frames of the front attempt played here.
    played: u64,
    /// The attempt the core was last entered for.
    entered: Option<u32>,
    /// Playing live, rather than waiting for a lead.
    playing: bool,
    /// Catching up, which goes on until the replay is within a lead of the
    /// newest input — not merely back under where it started — so it does
    /// not settle into playing live a second and more behind.
    catching: bool,
    /// Picture checks passed, for the log.
    checked: u64,
    /// Why this is not the player's game any more, until the next attempt.
    lost: Option<String>,
    /// The player prepared other content than this machine did. No attempt
    /// mends that.
    other_content: Option<String>,
    /// What was last reported, so a report goes only on a change.
    said: Option<ReplayState>,
}

impl Replica {
    fn new(player: String) -> Replica {
        Replica {
            player,
            attempts: VecDeque::new(),
            played: 0,
            entered: None,
            playing: false,
            catching: false,
            checked: 0,
            lost: None,
            other_content: None,
            said: None,
        }
    }

    /// Frames held and not yet played, across every attempt.
    fn ahead(&self) -> u64 {
        let held: u64 = self.attempts.iter().map(|a| a.inputs.len() as u64).sum();
        held.saturating_sub(self.played)
    }

    #[allow(clippy::too_many_arguments)]
    fn receive(
        &mut self,
        attempt: u32,
        from: u64,
        count: u32,
        bytes: &[u8],
        checks: Vec<(u64, u64)>,
        artifact: Option<&str>,
        ours: &str,
    ) {
        if let Some(theirs) = artifact.filter(|a| *a != ours) {
            let short = |s: &str| s.chars().take(8).collect::<String>();
            self.other_content = Some(format!(
                "this machine prepared other content ({}…) than the player did ({}…)",
                short(ours),
                short(theirs)
            ));
        }
        if attempt == 0 || count == 0 {
            return;
        }
        if self.attempts.front().is_some_and(|a| attempt < a.n) {
            return;
        }
        let frames = match replay::decode_frames(bytes, count as usize) {
            Ok(f) => f,
            Err(e) => {
                self.lost = Some(format!("the run's input could not be read: {e}"));
                return;
            }
        };
        if self.attempts.back().map_or(true, |a| attempt > a.n) {
            self.attempts.push_back(Attempt { n: attempt, inputs: Vec::new(), checks: BTreeMap::new() });
        }
        let Some(a) = self.attempts.iter_mut().find(|a| a.n == attempt) else { return };
        let have = a.inputs.len() as u64;
        if from > have + 1 {
            self.lost = Some(format!("frames {} to {} of the run never arrived", have + 1, from - 1));
            return;
        }
        // A backlog sent again overlaps what is held; only what is new counts.
        let skip = (have + 1 - from) as usize;
        a.inputs.extend(frames.into_iter().skip(skip));
        a.checks.extend(checks);
    }

    fn state(&self) -> ReplayState {
        if let Some(why) = self.other_content.as_ref().or(self.lost.as_ref()) {
            return ReplayState::Lost(why.clone());
        }
        let Some(front) = self.attempts.front() else { return ReplayState::Waiting };
        if self.catching || self.ahead() > CATCH_UP {
            let of = front.inputs.len() as u64;
            return ReplayState::CatchingUp { done: self.played.min(of), of };
        }
        ReplayState::Live
    }

    /// Tells the client how it is going, when that has changed. Catching up
    /// is said in tenths, not every frame.
    fn say(&mut self, round: u64, reports: &Sender<Report>) {
        let now = self.state();
        let bucket = |s: &ReplayState| match s {
            ReplayState::CatchingUp { done, of } => ReplayState::CatchingUp { done: done * 10 / (*of).max(1), of: 10 },
            other => other.clone(),
        };
        if self.said.as_ref().map(bucket) != Some(bucket(&now)) {
            self.said = Some(now.clone());
            let _ = reports.send(Report::Replay { round, player: self.player.clone(), state: now });
        }
    }

    /// The next frame's input, entering the core for a new attempt when the
    /// front one has played out. `None` when nothing is held to play.
    fn next(&mut self, runner: &mut LibretroRunner, spec: &StageSpec) -> Result<Option<replay::Input>, String> {
        loop {
            let Some(front) = self.attempts.front() else { return Ok(None) };
            if self.played < front.inputs.len() as u64 {
                break;
            }
            if self.attempts.len() == 1 {
                return Ok(None);
            }
            self.attempts.pop_front();
            self.played = 0;
        }
        let front = self.attempts.front().expect("checked above");
        if self.entered != Some(front.n) {
            // Every attempt starts from the stage's entry state, on the
            // player's machine and here alike.
            runner.enter(spec).map_err(|e| e.to_string())?;
            self.entered = Some(front.n);
            self.played = 0;
            self.lost = None;
        }
        Ok(Some(front.inputs[self.played as usize].clone()))
    }

    /// Plays `input` and checks the picture where the player sent a check.
    /// Catching up, nothing is read from the frame but its picture.
    fn play(&mut self, input: &replay::Input, runner: &mut LibretroRunner, catching_up: bool) -> Result<(), String> {
        replay::apply_input(input);
        if catching_up {
            runner.advance_unobserved();
        } else {
            runner.advance().map_err(|e| e.to_string())?;
        }
        self.played += 1;
        let frame = runner.frame();
        let want = self.attempts.front().and_then(|a| a.checks.get(&frame)).copied();
        if let Some(want) = want {
            if replay::picture_digest(runner) == want {
                self.checked += 1;
                if self.checked % 20 == 0 {
                    println!("[watch] {}: {} picture checks passed, the last on frame {frame}", self.player, self.checked);
                }
            } else if self.lost.is_none() {
                self.lost = Some(format!("the picture here differed from the player's on frame {frame}"));
            }
        }
        Ok(())
    }
}

/// A run's input on its way to the server, a few frames at a time: few
/// enough that a watcher plays only a little behind, enough that the lobby
/// thread is not handed a message a frame for every player.
#[derive(Default)]
struct Outgoing {
    attempt: u32,
    /// The frame number, within the attempt, of `frames[0]`.
    from: u64,
    frames: Vec<replay::Input>,
    checks: Vec<(u64, u64)>,
}

/// Frames per batch: about a tenth of a second.
const BATCH: usize = 8;
/// A picture digest every this many frames: a desync is seen within half a
/// second. Never on the first frames of an attempt, whose picture a core
/// restored from a state may still be showing from before it.
const CHECK_EVERY: u64 = 30;

impl Outgoing {
    /// A new attempt, from its first frame.
    fn start(&mut self, attempt: u32) {
        *self = Outgoing { attempt, from: 1, ..Outgoing::default() };
        // Edges the entry made — it releases whatever was held — are the
        // entry's, and a replay's own entry makes them again.
        libretro::take_key_edges();
    }

    fn push(&mut self, input: replay::Input, frame: u64, runner: &LibretroRunner) {
        self.frames.push(input);
        if frame % CHECK_EVERY == 0 {
            self.checks.push((frame, replay::picture_digest(runner)));
        }
    }

    fn flush(&mut self, round: u64, reports: &Sender<Report>) {
        if self.frames.is_empty() {
            return;
        }
        let count = self.frames.len() as u32;
        let _ = reports.send(Report::Inputs {
            round,
            attempt: self.attempt,
            from: self.from,
            count,
            frames: replay::encode_frames(&self.frames),
            checks: std::mem::take(&mut self.checks),
        });
        self.from += u64::from(count);
        self.frames.clear();
    }
}

/// The same scripted pad the harness's sync test presses: a random set of
/// buttons held for a while, never start or select.
fn scripted_pad(state: &mut (u64, u32), frame: u64) -> u32 {
    state.0 = state.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state.0;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    let r = (z ^ (z >> 31)) ^ frame;
    if r % 12 == 0 {
        let bits = (r >> 8) as u32;
        let dpad = 1u32 << (libretro::JOY_UP + ((bits >> 4) & 3));
        let face = bits & ((1 << libretro::JOY_B) | (1 << libretro::JOY_Y) | (1 << libretro::JOY_A) | (1 << libretro::JOY_X));
        state.1 = dpad | face;
    }
    state.1
}

impl Loaded {
    /// Sending a picture to a spectator right now. Only a bout's: a run is
    /// replayed from its input instead (ADR 0037).
    fn streaming(&self) -> bool {
        self.watched() && self.bout.is_some()
    }

    /// Somebody is watching this run while it is live.
    fn watched(&self) -> bool {
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
                            x.0.artifact = x.1.clone();
                            place_window(hwnd, &x.0, &mut borderless, &mut sized_for);
                            // A bout is prepared only once its machines have
                            // found each other (`bout_prepare`).
                            if x.0.bout.is_none() {
                                let _ = reports.send(Report::Loaded { round, artifact: x.1 });
                            }
                            x.0.watchers = watchers;
                            x.0.want_key = watchers > 0;
                            x.0.machine.runner().tap_audio(watchers > 0);
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
                            if let Some(b) = l.bout.as_mut() {
                                b.judge.enter();
                            }
                            if let Machine::Relay(r) = &mut l.machine {
                                if let Err(e) = r.join() {
                                    eprintln!("[bout] cannot join the game's network: {e}");
                                }
                            }
                            if l.bout.is_none() {
                                // What a spectator replays (ADR 0037): every
                                // key edge the core is handed, from here.
                                libretro::record_key_edges(true);
                                l.outgoing.start(l.run.attempts());
                            }
                            l.live = true;
                            l.next_deadline = Instant::now();
                            held = false;
                        }
                    }
                }
                Ok(Command::Peers(peers)) => {
                    if let Some(l) = loaded.as_mut() {
                        if let Err(e) = connect_bout(l, peers) {
                            let round = l.round;
                            let _ = reports.send(Report::LoadFailed { round, error: e });
                            loaded = None;
                        }
                    }
                }
                Ok(Command::PeerForfeited { slot, frame }) => {
                    if let Some(l) = loaded.as_mut() {
                        if let Some(b) = l.bout.as_mut() {
                            let verdicts = b.judge.forfeit(Slot(slot), frame);
                            if let Some((_, result)) = verdicts.into_iter().find(|(s, _)| *s == b.slot) {
                                if !l.done {
                                    l.done = true;
                                    let round = l.round;
                                    let _ = reports.send(Report::Finished { round, result, forfeited_at: None });
                                }
                            }
                        }
                    }
                }
                Ok(Command::Watch(Some(w))) => {
                    let Watching { load: what, player } = *w;
                    let round = what.round;
                    // A player whose run is over replays on the core they
                    // played on; anyone else prepares the round as a player
                    // would.
                    let same = loaded.as_ref().is_some_and(|l| l.round == round && l.bout.is_none());
                    if !same {
                        loaded = None;
                        match load(what, &mut window) {
                            Ok((mut x, artifact)) => {
                                x.artifact = artifact;
                                place_window(hwnd, &x, &mut borderless, &mut sized_for);
                                loaded = Some(x);
                            }
                            Err(e) => {
                                let state = ReplayState::Lost(format!("this machine cannot prepare the round: {e}"));
                                let _ = reports.send(Report::Replay { round, player: player.clone(), state });
                            }
                        }
                    }
                    if let Some(l) = loaded.as_mut() {
                        l.watchers = 0;
                        l.machine.runner().tap_audio(false);
                        libretro::record_key_edges(false);
                        let mut r = Replica::new(player);
                        r.say(round, &reports);
                        l.replica = Some(r);
                        held = false;
                    }
                }
                Ok(Command::Watch(None)) => {
                    if let Some(l) = loaded.as_mut() {
                        // Loaded only to replay someone else: nothing of this
                        // player's own is left in it.
                        if l.replica.take().is_some() && !l.live {
                            loaded = None;
                        }
                    }
                }
                Ok(Command::Inputs { player, attempt, from, count, frames, checks, artifact }) => {
                    if let Some(l) = loaded.as_mut() {
                        let (round, ours) = (l.round, l.artifact.clone());
                        if let Some(r) = l.replica.as_mut().filter(|r| r.player == player) {
                            r.receive(attempt, from, count, &frames, checks, artifact.as_deref(), &ours);
                            r.say(round, &reports);
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
                        l.machine.runner().tap_audio(false);
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
                        l.machine.runner().tap_audio(n > 0);
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

        if loaded.as_ref().is_some_and(|l| l.machine.runner().shutdown_requested()) {
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
                    // A shared machine cannot be held (ADR 0036): the menu
                    // comes up over a game that goes on.
                    held = l.bout.is_none();
                    let _ = reports.send(Report::Paused);
                }
            }
        }

        match loaded.as_mut() {
            None => {
                window.update();
                std::thread::sleep(Duration::from_millis(16));
            }
            Some(l) if l.replica.is_some() => {
                step_replica(l, &mut window, &mut canvas, &reports, &streamer)?;
            }
            Some(l) if held || !l.live || l.done => {
                // Present whatever is there, pump the window, wait a frame.
                if !l.live {
                    bout_prepare(l, &reports);
                    // The entry frame, if the core drew one on entering.
                    present(l, &mut window, &mut canvas, &streamer)?;
                } else {
                    window.update();
                }
                std::thread::sleep(l.frame_time);
            }
            Some(l) if l.bout.as_ref().is_some_and(|b| b.packets) => {
                step_relay(l, &mut window, &mut canvas, in_front, &pad, &reports, &streamer)?;
            }
            Some(l) if l.bout.is_some() => {
                step_bout(l, &mut window, &mut canvas, in_front, &pad, &reports, &streamer)?;
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
    let packets = runner.manifest().libretro.netplay == Some(Netplay::Packets);
    if let (true, Some(b)) = (packets, l.bout.as_ref()) {
        // The game's own network driver must be up before the game boots and
        // looks for it (ADR 0036): join now, with every other slot present.
        runner.join(Slot(b.slot), b.slots).map_err(|e| e.to_string())?;
        for i in 0..b.slots {
            if i != b.slot {
                runner.peer_joined(Slot(i));
            }
        }
    }
    runner.enter(&spec).map_err(|e| e.to_string())?;
    let timed = matches!(judged.kind, ChallengeKind::TimeTrial);
    let bout = match l.bout {
        Some(b) => {
            if judged.kind != ChallengeKind::HeadToHead {
                return Err(format!("{} is not a head-to-head challenge", judged.id));
            }
            let udp = bind_bout(b.port)?;
            let judge = Bout::new(judged.clone(), Some(l.limit)).map_err(|e| e.to_string())?;
            let token = token_for(&udp);
            Some(BoutState {
                slot: Slot(b.slot),
                slots: b.slots,
                packets,
                rendezvous: b.rendezvous,
                token,
                input_delay: b.input_delay,
                udp: Some(udp),
                judge,
                artifact: artifact.clone(),
                last_probe: None,
                endpoint_sent: false,
                syncing_since: None,
                loaded_sent: false,
                lost_sent: false,
            })
        }
        None => None,
    };
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
            machine: Machine::Solo(Box::new(runner)),
            bout,
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
            script: l.script.map(|seed| (seed, 0)),
            outgoing: Outgoing::default(),
            artifact: String::new(),
            give_up_at: l.give_up_after.map(|secs| (secs * fps).round() as u64),
            replica: None,
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
    pace(l);
    window.update();

    let next_frame = l.machine.runner().frame() + 1;
    let bits = match l.script.as_mut() {
        Some(state) => scripted_pad(state, next_frame),
        None if in_front => keys::joypad(&l.input, pad),
        None => 0,
    };
    match l.input.keyboard.as_ref() {
        None => libretro::JOYPAD.store(bits, Ordering::Relaxed),
        Some(kb) => {
            // A script presses a keyboard game's keys through its pad
            // bindings, in front or not; the mouse stays the player's.
            if in_front || l.script.is_some() {
                keys::keyboard_game(kb, bits, pad, l.input.deadzone);
                if let Some(m) = l.mouse.as_mut().filter(|_| in_front) {
                    m.pump(window);
                }
            } else {
                keys::keyboard_release_all();
            }
        }
    }

    // Exactly what the core is about to read, for a spectator to replay.
    let input = replay::capture_input();
    let events = l.machine.runner_mut().advance().map_err(|e| e.to_string())?.to_vec();
    let frame = l.machine.runner().frame();
    l.outgoing.push(input, frame, l.machine.runner());
    let next = l.run.observe(frame, &events);
    if l.outgoing.frames.len() >= BATCH || next != Next::Continue {
        l.outgoing.flush(l.round, reports);
    }
    // While watched, a frame twice a second besides: the server relays where
    // the run stands, and a watcher shows the clock and the run's events
    // from it (ADR 0034).
    let heartbeat = l.watched() && frame % 30 == 0;
    if !events.is_empty() || next != Next::Continue || heartbeat {
        let _ = reports.send(Report::Frame { round: l.round, frame, events });
    }
    l.done = matches!(next, Next::Finished(_));
    // Before `Finished`, so the last reading reaches the overlay ahead of
    // the round card.
    clock(l, reports);
    if !l.done && l.give_up_at.is_some_and(|at| l.run.frames() >= at) {
        // As the give-up button does: a scripted hand test's way out.
        l.outgoing.flush(l.round, reports);
        forfeit(l, reports);
        return present(l, window, canvas, streamer);
    }
    match next {
        Next::Continue => {}
        Next::Reenter => {
            l.machine.runner_mut().enter(&l.spec).map_err(|e| e.to_string())?;
            l.run.enter();
            l.outgoing.start(l.run.attempts());
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

/// Wait for the next frame's turn: on the sound card's queue when there is
/// one, since that is what playback drains at, else on the clock.
fn pace(l: &mut Loaded) {
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
}

/// One turn of replaying a watched run (ADR 0037): far behind, as many
/// frames as fit in a slice, unpaced and silent; otherwise one frame at the
/// game's rate, a lead behind the newest input, holding when it runs dry.
fn step_replica(
    l: &mut Loaded,
    window: &mut Window,
    canvas: &mut Canvas,
    reports: &Sender<Report>,
    streamer: &Streamer,
) -> Result<(), String> {
    let mut r = l.replica.take().expect("only called with a replica");
    let result = replica_turn(l, &mut r, window);
    r.say(l.round, reports);
    l.replica = Some(r);
    match result? {
        true => present(l, window, canvas, streamer),
        false => {
            window.update();
            std::thread::sleep(l.frame_time);
            Ok(())
        }
    }
}

/// Whether a frame was played, and so whether there is a picture to show.
fn replica_turn(l: &mut Loaded, r: &mut Replica, window: &mut Window) -> Result<bool, String> {
    let ahead = r.ahead();
    if r.catching || ahead > CATCH_UP {
        r.catching = true;
        r.playing = false;
        let was = libretro::AUDIO_ENABLED.swap(0, Ordering::Relaxed);
        let until = Instant::now() + CATCH_UP_SLICE;
        let mut played = false;
        let outcome = (|| {
            while r.ahead() > LEAD && Instant::now() < until {
                let Some(input) = r.next(l.machine.runner_mut(), &l.spec)? else { break };
                r.play(&input, l.machine.runner_mut(), true)?;
                played = true;
            }
            Ok::<_, String>(())
        })();
        libretro::AUDIO_ENABLED.store(was, Ordering::Relaxed);
        l.next_deadline = Instant::now();
        outcome?;
        if r.ahead() <= LEAD {
            r.catching = false;
        }
        return Ok(played);
    }
    if !r.playing && ahead < LEAD {
        return Ok(false);
    }
    pace(l);
    window.update();
    let Some(input) = r.next(l.machine.runner_mut(), &l.spec)? else {
        r.playing = false;
        return Ok(false);
    };
    r.playing = true;
    r.play(&input, l.machine.runner_mut(), false)?;
    Ok(true)
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
    if let Some(b) = l.bout.as_mut() {
        // On the last frame every slot's input was final for: the frames
        // after it were guesses, and the server has none of them.
        let frame = match &l.machine {
            Machine::Bout(s) => s.confirmed_frame(),
            other => other.runner().frame(),
        };
        let verdicts = b.judge.forfeit(b.slot, frame);
        let result = verdicts
            .into_iter()
            .find(|(s, _)| *s == b.slot)
            .map(|(_, r)| r)
            .unwrap_or_else(|| b.judge.run(b.slot).result().cloned().unwrap_or_else(|| {
                RunResult {
                    challenge: l.run.spec().id.clone(),
                    kind: ChallengeKind::HeadToHead,
                    ending: challenge_core::Ending::Forfeited,
                    frames: frame,
                    attempts: 1,
                }
            }));
        l.done = true;
        let _ = reports.send(Report::Finished { round: l.round, result, forfeited_at: Some(frame) });
        return;
    }
    let frame = l.machine.runner().frame();
    let result = l.run.forfeit();
    l.done = true;
    clock(l, reports);
    let _ = reports.send(Report::Finished { round: l.round, result, forfeited_at: Some(frame) });
}

/// A token for the probes: whatever this socket's port and the clock make
/// of each other. It only has to be unlikely to collide within one lobby.
fn token_for(udp: &Udp) -> u64 {
    let port = udp.local_addr().map(|a| u64::from(a.port())).unwrap_or(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    let mut z = nanos ^ (port << 48) ^ 0x9E37_79B9_7F4A_7C15;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The bout's UDP side, on the port Settings names so a router can forward
/// it; any port when that one is taken, as by a second client on this
/// machine, or when Settings says 0.
fn bind_bout(port: u16) -> Result<Udp, String> {
    let any = || Udp::bind("0.0.0.0:0".parse().unwrap()).map_err(|e| format!("cannot open a UDP socket: {e}"));
    if port == 0 {
        return any();
    }
    match Udp::bind(SocketAddr::from(([0, 0, 0, 0], port))) {
        Ok(udp) => Ok(udp),
        Err(e) => {
            eprintln!("[bout] UDP port {port}: {e}; taking any port, which a router will not forward");
            any()
        }
    }
}

/// The address this machine's UDP side is at on its own network: the
/// interface that reaches the server, and the bound port. What a peer on the
/// same LAN or the same machine uses, since the address the server saw may
/// be a router's that will not turn a packet back inside.
fn local_address(udp: &Udp, rendezvous: Option<SocketAddr>) -> String {
    let port = udp.local_addr().map(|a| a.port()).unwrap_or(0);
    let ip = rendezvous
        .and_then(|rv| {
            let probe = UdpSocket::bind("0.0.0.0:0").ok()?;
            probe.connect(rv).ok()?;
            probe.local_addr().ok().map(|a| a.ip())
        })
        .unwrap_or_else(|| "127.0.0.1".parse().unwrap());
    format!("{ip}:{port}")
}

/// Before a bout goes: probe the server until the peers are known, then
/// find them, and only then say the round is prepared (ADR 0036). A bout
/// whose machines cannot find each other within the timeout cannot be
/// prepared, and the round is voided for everyone (ADR 0029).
fn bout_prepare(l: &mut Loaded, reports: &Sender<Report>) {
    let Some(b) = l.bout.as_mut() else { return };
    if l.done {
        // Over on this side; the other machine may still want this one's
        // last inputs to confirm the same frame, and only a poll resends.
        match &mut l.machine {
            Machine::Bout(s) => {
                s.poll();
            }
            Machine::Relay(r) => {
                r.poll();
            }
            _ => {}
        }
        return;
    }
    if b.loaded_sent {
        // Prepared, waiting for `Go`: the engine's keep-alives leave only on
        // a poll, and the backing window is longer than its timeout.
        if let Machine::Relay(r) = &mut l.machine {
            r.poll();
        }
        if let Machine::Bout(s) = &mut l.machine {
            if let RollbackState::Lost(slot) = s.poll().clone() {
                if !b.lost_sent {
                    b.lost_sent = true;
                    l.done = true;
                    let frame = s.confirmed_frame();
                    eprintln!("[bout] lost {slot} before the bout went");
                    let _ = reports.send(Report::PeerLost { round: l.round, frame });
                }
            }
        }
        return;
    }
    if let (Some(udp), Some(rv)) = (b.udp.as_ref(), b.rendezvous) {
        if b.last_probe.map_or(true, |t| t.elapsed() >= PROBE_EVERY) {
            if let Err(e) = udp.probe(rv, b.token) {
                eprintln!("[bout] probe to {rv}: {e}");
            }
            b.last_probe = Some(Instant::now());
        }
        if !b.endpoint_sent {
            b.endpoint_sent = true;
            let local = local_address(udp, Some(rv));
            println!("[bout] slot {} of {}: probing {rv} as {local}", b.slot.0, b.slots);
            let _ = reports.send(Report::Endpoint { round: l.round, token: b.token, local });
        }
    } else if b.udp.is_some() && !b.endpoint_sent {
        let _ = reports.send(Report::LoadFailed {
            round: l.round,
            error: "this server has no UDP side to broker a bout through".into(),
        });
        b.endpoint_sent = true;
        l.done = true;
    }
    if let Machine::Relay(r) = &mut l.machine {
        match r.poll().clone() {
            RelayState::Connected | RelayState::Live => {
                b.loaded_sent = true;
                println!("[bout] the other machine answered");
                let _ = reports.send(Report::Loaded { round: l.round, artifact: b.artifact.clone() });
            }
            RelayState::Connecting => {
                if b.syncing_since.is_some_and(|t| t.elapsed() >= SYNC_TIMEOUT) {
                    l.done = true;
                    let _ = reports.send(Report::LoadFailed {
                        round: l.round,
                        error: "could not reach the other player's machine".into(),
                    });
                }
            }
        }
        return;
    }
    let Machine::Bout(s) = &mut l.machine else { return };
    let state = s.poll().clone();
    match state {
        RollbackState::Live => {
            b.loaded_sent = true;
            let stats = s.stats();
            println!("[bout] synchronised, round trip {} ms", stats.ping_ms);
            let _ = reports.send(Report::Loaded { round: l.round, artifact: b.artifact.clone() });
        }
        RollbackState::Synchronizing => {
            if b.syncing_since.is_some_and(|t| t.elapsed() >= SYNC_TIMEOUT) {
                l.done = true;
                let _ = reports.send(Report::LoadFailed {
                    round: l.round,
                    error: "could not reach the other player's machine".into(),
                });
            }
        }
        RollbackState::Lost(slot) => {
            l.done = true;
            let _ = reports.send(Report::LoadFailed { round: l.round, error: format!("lost {slot} while connecting") });
        }
        RollbackState::Desynced { frame } => {
            l.done = true;
            let _ = reports.send(Report::LoadFailed { round: l.round, error: format!("the machines disagreed at frame {frame}") });
        }
    }
}

/// The peers are known: the UDP side learns where they are and the runner
/// goes into the rollback session, which starts looking for them.
fn connect_bout(l: &mut Loaded, peers: Vec<(u8, Vec<SocketAddr>)>) -> Result<(), String> {
    let Some(b) = l.bout.as_mut() else { return Ok(()) };
    let Some(mut udp) = b.udp.take() else { return Ok(()) };
    let mine: Option<SocketAddr> = local_address(&udp, b.rendezvous).parse().ok();
    for (slot, candidates) in peers {
        if slot != b.slot.0 {
            // A peer on the server's own network is, from out here, at the
            // server's public address. This machine's own address is not
            // the peer, whatever two networks with one private range say.
            let candidates: Vec<SocketAddr> = head_to_head::reach(candidates, b.rendezvous)
                .into_iter()
                .filter(|c| Some(*c) != mine)
                .collect();
            println!("[bout] slot {slot} at {candidates:?}");
            udp.peer(Slot(slot), candidates);
        }
    }
    let runner = match std::mem::replace(&mut l.machine, Machine::Moving) {
        Machine::Solo(r) => *r,
        other => {
            l.machine = other;
            return Ok(());
        }
    };
    if b.packets {
        l.machine = Machine::Relay(Box::new(Relay::connect_joined(runner, udp, b.slot, b.slots)));
        b.syncing_since = Some(Instant::now());
        return Ok(());
    }
    let settings = RollbackSettings {
        input_delay: usize::from(b.input_delay),
        fps: l.fps.round().max(1.0) as usize,
        ..Default::default()
    };
    let session = Rollback::connect(runner, udp, b.slot, b.slots, settings).map_err(|e| e.to_string())?;
    l.machine = Machine::Bout(Box::new(session));
    b.syncing_since = Some(Instant::now());
    Ok(())
}

/// One frame of a relayed bout: paced as a solo frame is, the keyboard and
/// mouse (a DOS game) or the pad read as usual, the game's own packets
/// carried, and this machine's own facts judged — the other slot's come to
/// the server from the other machine, and its verdict for this one comes
/// back as `PlayerFinished`.
fn step_relay(
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

    let frame_now = l.machine.runner().frame() + 1;
    let scripted = l.script.is_some();
    let bits = match l.script.as_mut() {
        Some(state) => scripted_pad(state, frame_now),
        None if in_front => keys::joypad(&l.input, pad),
        None => 0,
    };
    match l.input.keyboard.as_ref() {
        None => libretro::JOYPAD.store(bits, Ordering::Relaxed),
        Some(kb) => {
            if in_front || scripted {
                keys::keyboard_game(kb, bits, pad, l.input.deadzone);
                if let Some(m) = l.mouse.as_mut() {
                    m.pump(window);
                }
            } else {
                keys::keyboard_release_all();
            }
        }
    }

    let round = l.round;
    let streaming = l.streaming();
    let watchers = l.watchers;
    let (Some(b), Machine::Relay(r)) = (l.bout.as_mut(), &mut l.machine) else { return Ok(()) };
    r.pump().map_err(|e| e.to_string())?;
    let frame = r.machine().frame();
    // A game in lockstep simulates every player on every machine, so its
    // facts are tagged per slot and every slot is judged here, as on a
    // rolled-back machine; only this slot's go to the server.
    let events = r.machine().slot_events().to_vec();
    if watchers > 0 {
        r.machine().take_audio(&mut |samples, rate| {
            if streaming {
                streamer.sound(Sound { round, frame, rate, samples: samples.to_vec() });
            }
        });
    }
    if !l.done {
        let mine: Vec<SessionEvent> = events
            .iter()
            .filter(|e| e.slot.is_none() || e.slot == Some(b.slot))
            .map(|e| e.event.clone())
            .collect();
        let mut verdicts = Vec::new();
        for slot in 0..b.slots {
            let theirs: Vec<SessionEvent> = events
                .iter()
                .filter(|e| e.slot.is_none() || e.slot == Some(Slot(slot)))
                .map(|e| e.event.clone())
                .collect();
            verdicts.extend(b.judge.observe(Slot(slot), frame, &theirs));
        }
        let settled = verdicts.into_iter().find(|(s, _)| *s == b.slot).map(|(_, r)| r);
        if !mine.is_empty() || frame % HEARTBEAT == 0 || settled.is_some() {
            let _ = reports.send(Report::Frame { round, frame, events: mine });
        }
        if let Some(result) = settled {
            l.done = true;
            let _ = reports.send(Report::Finished { round, result, forfeited_at: None });
        }
    }
    present(l, window, canvas, streamer)
}

/// One frame of a bout: pace on the clock (a frame the engine sits out
/// drains no audio ring), read the local pad, pump the session, and judge
/// every slot's confirmed facts; this slot's go to the server.
fn step_bout(
    l: &mut Loaded,
    window: &mut Window,
    canvas: &mut Canvas,
    in_front: bool,
    pad: &PadState,
    reports: &Sender<Report>,
    streamer: &Streamer,
) -> Result<(), String> {
    let now = Instant::now();
    if l.next_deadline > now {
        std::thread::sleep(l.next_deadline - now);
    } else {
        l.next_deadline = now;
    }
    l.next_deadline += l.frame_time;
    window.update();

    let frame_now = l.machine.runner().frame() + 1;
    let bits = match l.script.as_mut() {
        Some(state) => scripted_pad(state, frame_now),
        None if in_front => keys::joypad(&l.input, pad),
        None => 0,
    };
    let round = l.round;
    let streaming = l.streaming();
    let watchers = l.watchers;
    let (Some(b), Machine::Bout(s)) = (l.bout.as_mut(), &mut l.machine) else { return Ok(()) };
    let confirmed = s.pump(&[(b.slot, SlotInput(bits))]).map_err(|e| e.to_string())?;
    let frame = s.frame();
    if watchers > 0 {
        s.machine().take_audio(&mut |samples, rate| {
            if streaming {
                streamer.sound(Sound { round, frame, rate, samples: samples.to_vec() });
            }
        });
    }
    let mut mine_done = None;
    for c in confirmed {
        let mine: Vec<SessionEvent> = c
            .events
            .iter()
            .filter(|e| e.slot.is_none() || e.slot == Some(b.slot))
            .map(|e| e.event.clone())
            .collect();
        let mut verdicts = Vec::new();
        for slot in 0..b.slots {
            let theirs: Vec<SessionEvent> = c
                .events
                .iter()
                .filter(|e| e.slot.is_none() || e.slot == Some(Slot(slot)))
                .map(|e| e.event.clone())
                .collect();
            verdicts.extend(b.judge.observe(Slot(slot), c.frame, &theirs));
        }
        let settled = verdicts.into_iter().find(|(s, _)| *s == b.slot).map(|(_, r)| r);
        // The frame a verdict settles on is always reported, as a solo run's
        // is: the server's judge holds another slot's completion until this
        // slot has reported past it (ADR 0036).
        if !mine.is_empty() || c.frame % HEARTBEAT == 0 || settled.is_some() {
            let _ = reports.send(Report::Frame { round, frame: c.frame, events: mine });
        }
        if settled.is_some() {
            mine_done = settled;
            break;
        }
    }
    match s.state().clone() {
        RollbackState::Lost(slot) if !b.lost_sent => {
            b.lost_sent = true;
            l.done = true;
            let frame = s.confirmed_frame();
            eprintln!("[bout] lost {slot} at frame {frame}");
            let _ = reports.send(Report::PeerLost { round, frame });
        }
        RollbackState::Desynced { frame } if !l.done => {
            eprintln!("[bout] the machines disagreed at frame {frame}; giving up the run");
            forfeit(l, reports);
            return present(l, window, canvas, streamer);
        }
        _ => {}
    }
    if let Some(result) = mine_done {
        if !l.done {
            l.done = true;
            let _ = reports.send(Report::Finished { round, result, forfeited_at: None });
        }
    }
    present(l, window, canvas, streamer)
}

fn present(l: &mut Loaded, window: &mut Window, canvas: &mut Canvas, streamer: &Streamer) -> Result<(), String> {
    let mut painted = false;
    let mut paint_error = None;
    let client = window.get_size();
    let (video, aspect) = (l.video, l.core_aspect);
    // For a spectator: the core's own picture, before the player's settings
    // shape it (ADR 0032). One copy here; the encoder runs on its own thread
    // and a picture it has no time for is simply not offered.
    let (streaming, want_key, round, frame) = (l.streaming(), l.want_key, l.round, l.machine.runner().frame());
    let mut keyed = false;
    l.last_seq = l.machine.runner().present(l.last_seq, &mut |f| {
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
    let av = l.machine.runner().av_info();
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

#[cfg(test)]
mod tests {
    use super::*;

    fn pad(bits: u32) -> replay::Input {
        replay::Input { joypad: bits, ..replay::Input::default() }
    }

    fn bytes(bits: &[u32]) -> Vec<u8> {
        replay::encode_frames(&bits.iter().map(|b| pad(*b)).collect::<Vec<_>>())
    }

    fn held(r: &Replica) -> Vec<(u32, Vec<u32>)> {
        r.attempts.iter().map(|a| (a.n, a.inputs.iter().map(|i| i.joypad).collect())).collect()
    }

    /// A backlog sent again — a watcher who switches away and back — overlaps
    /// what is held, and only its new frames count.
    #[test]
    fn a_backlog_sent_again_adds_only_what_is_new() {
        let mut r = Replica::new("A".into());
        r.receive(1, 1, 3, &bytes(&[1, 2, 3]), vec![], Some("abc"), "abc");
        r.receive(1, 1, 5, &bytes(&[1, 2, 3, 4, 5]), vec![(5, 9)], None, "abc");
        r.receive(1, 6, 1, &bytes(&[6]), vec![], None, "abc");
        assert_eq!(held(&r), [(1, vec![1, 2, 3, 4, 5, 6])]);
        assert_eq!(r.attempts[0].checks.get(&5), Some(&9));
        assert_eq!(r.state(), ReplayState::Live);
    }

    /// A death on the player's side starts the next attempt while this
    /// machine is still behind them in the last: it waits its turn, so the
    /// end of the attempt being watched is not cut off.
    #[test]
    fn the_next_attempt_waits_for_this_one_to_play_out() {
        let mut r = Replica::new("A".into());
        r.receive(1, 1, 2, &bytes(&[1, 2]), vec![], None, "abc");
        r.receive(2, 1, 1, &bytes(&[7]), vec![], None, "abc");
        assert_eq!(held(&r), [(1, vec![1, 2]), (2, vec![7])]);
        assert_eq!(r.ahead(), 3);
        // An attempt already behind this one is nobody's any more.
        r.attempts.pop_front();
        r.receive(1, 3, 1, &bytes(&[3]), vec![], None, "abc");
        assert_eq!(held(&r), [(2, vec![7])]);
    }

    /// Frames that never arrived, or another machine's content, and this is
    /// not the player's game: said, not played on.
    #[test]
    fn a_gap_or_other_content_is_lost() {
        let mut r = Replica::new("A".into());
        r.receive(1, 1, 1, &bytes(&[1]), vec![], None, "abc");
        r.receive(1, 5, 1, &bytes(&[5]), vec![], None, "abc");
        assert!(matches!(r.state(), ReplayState::Lost(why) if why.contains("frames 2 to 4")));
        assert_eq!(held(&r), [(1, vec![1])], "nothing after a gap is kept");

        let mut r = Replica::new("A".into());
        r.receive(0, 1, 0, &[], vec![], Some("def"), "abc");
        assert!(matches!(r.state(), ReplayState::Lost(why) if why.contains("other content")));
    }

    /// Far enough behind and the replay catches up rather than playing at
    /// the game's rate; nothing yet and it waits.
    #[test]
    fn far_behind_is_catching_up_and_nothing_is_waiting() {
        let mut r = Replica::new("A".into());
        assert_eq!(r.state(), ReplayState::Waiting);
        let many: Vec<u32> = (0..CATCH_UP as u32 + 10).collect();
        r.receive(1, 1, many.len() as u32, &bytes(&many), vec![], None, "abc");
        assert_eq!(r.state(), ReplayState::CatchingUp { done: 0, of: many.len() as u64 });
    }
}
