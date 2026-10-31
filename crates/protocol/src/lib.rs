//! The wire between a client and the server (ADR 0009, ADR 0024).
//!
//! Every message is a [`Packet`]: a version and a body. The body types are this
//! crate's own, derived onto JSON, and never borrowed from `session-api` or
//! `challenge-core`: a shared Rust type on the wire is a protocol that changes
//! whenever the struct does. Conversions live here so there is one place that
//! knows both vocabularies.
//!
//! The rules of the protocol (CLAUDE.md): a field is never removed, only
//! deprecated; `VERSION` is bumped when a reader of the old version could
//! misread the new one. Solo play speaks exactly this protocol in-process:
//! a rule that only holds when a server is present is a rule solo play breaks.

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use session_api::{ChallengeKind, ExitReason, SessionEvent};

/// 2 added `Unprepared` and `Voided` (ADR 0029) and `Hello.lobby` (ADR 0030).
/// 3 added `Watch`, `Video`, `Audio` and `Watched`: spectating rides this
/// socket (ADR 0032).
pub const VERSION: u32 = 3;

/// A versioned message. `v` is read before anything else.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Packet<M> {
    pub v: u32,
    pub body: M,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    Version { got: u32, want: u32 },
    Malformed(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Version { got, want } => {
                write!(f, "protocol version {got}, this side speaks {want}")
            }
            ProtocolError::Malformed(e) => write!(f, "malformed message: {e}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

/// Wraps a body in the current version and encodes it as one line of JSON.
pub fn encode<M: Serialize>(body: &M) -> String {
    serde_json::to_string(&Packet { v: VERSION, body }).expect("wire types serialise")
}

/// Decodes a packet and refuses one from another version.
pub fn decode<M: DeserializeOwned>(text: &str) -> Result<M, ProtocolError> {
    let packet: Packet<M> =
        serde_json::from_str(text).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    if packet.v != VERSION {
        return Err(ProtocolError::Version { got: packet.v, want: VERSION });
    }
    Ok(packet.body)
}

// ------------------------------------------------------------------- lobby

/// A challenge, named the way the library names it: a game and a challenge id.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct ChallengeRef {
    pub game: String,
    pub challenge: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    TimeTrial,
    Survival,
    Combo,
}

impl From<Mode> for challenge_core::Mode {
    fn from(m: Mode) -> Self {
        match m {
            Mode::TimeTrial => challenge_core::Mode::TimeTrial,
            Mode::Survival => challenge_core::Mode::Survival,
            Mode::Combo => challenge_core::Mode::Both,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "n", rename_all = "snake_case")]
pub enum Format {
    Challenges(u32),
    Strikes(u32),
    Wins(u32),
    Stock(u32),
}

impl From<Format> for challenge_core::Format {
    fn from(f: Format) -> Self {
        match f {
            Format::Challenges(n) => challenge_core::Format::Challenges(n),
            Format::Strikes(n) => challenge_core::Format::Strikes(n),
            Format::Wins(n) => challenge_core::Format::Wins(n),
            Format::Stock(n) => challenge_core::Format::Stock(n),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Grouping {
    Era,
    Genre,
    Console,
}

/// What the host sets. Sent whole on every change.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct LobbySettings {
    pub mode: Mode,
    pub format: Format,
    /// The bound for a challenge whose manifest hints none (ADR 0026).
    pub default_limit_seconds: f64,
    pub allow_custom: bool,
    pub grouping: Grouping,
}

impl Default for LobbySettings {
    fn default() -> Self {
        LobbySettings {
            mode: Mode::Survival,
            format: Format::Challenges(10),
            default_limit_seconds: 180.0,
            allow_custom: true,
            grouping: Grouping::Console,
        }
    }
}

/// A base ROM a player does not have, or has the wrong dump of. Named by
/// filename and hash so they can go and find it; the platform never supplies
/// one.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct MissingFile {
    pub game: String,
    pub rom: String,
    pub name: String,
    pub sha1: Option<String>,
    pub problem: String,
}

/// What a player has, said as what they lack: every game not listed is one
/// they can play.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct FileReport {
    pub missing: Vec<MissingFile>,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct LobbyPlayer {
    pub id: String,
    pub name: String,
    pub host: bool,
    /// `None` until the player's client has checked its files.
    pub files: Option<FileReport>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct LobbyState {
    /// The invite code; "local" for a lobby of one played in-process.
    pub code: String,
    pub settings: LobbySettings,
    pub players: Vec<LobbyPlayer>,
    /// The challenges the host has toggled on, whether or not the settings
    /// admit them right now.
    pub enabled: Vec<ChallengeRef>,
    /// Those of `enabled` that the mode, the custom toggle and every player's
    /// files admit: what a match started now would deal from.
    pub dealable: Vec<ChallengeRef>,
    pub can_start: bool,
    /// Why not, when `can_start` is false.
    pub blocking: Vec<String>,
    /// A match is being played, or its score screen is up. When it turns
    /// false the host has gone back to the lobby, and so does everyone.
    #[serde(default)]
    pub in_match: bool,
}

// ------------------------------------------------------------------- facts

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Entered { frame: u64, stage: String },
    Cleared { frame: u64 },
    Exited { frame: u64, reason: String },
    Died { frame: u64 },
    Damaged { frame: u64 },
    EnemyDefeated { frame: u64, count: u32 },
    ItemCollected { frame: u64, item: String, count: u32 },
    Counted { frame: u64, counter: String, count: u32 },
    ObjectiveReached { frame: u64, objective: String },
}

impl From<&SessionEvent> for Event {
    fn from(e: &SessionEvent) -> Self {
        match e {
            SessionEvent::StageEntered { frame, stage } => {
                Event::Entered { frame: *frame, stage: stage.0.clone() }
            }
            SessionEvent::StageCleared { frame } => Event::Cleared { frame: *frame },
            SessionEvent::StageExited { frame, reason } => {
                Event::Exited { frame: *frame, reason: reason.to_string() }
            }
            SessionEvent::Died { frame } => Event::Died { frame: *frame },
            SessionEvent::Damaged { frame } => Event::Damaged { frame: *frame },
            SessionEvent::EnemyDefeated { frame, count } => {
                Event::EnemyDefeated { frame: *frame, count: *count }
            }
            SessionEvent::ItemCollected { frame, item, count } => {
                Event::ItemCollected { frame: *frame, item: item.clone(), count: *count }
            }
            SessionEvent::Counted { frame, counter, count } => {
                Event::Counted { frame: *frame, counter: counter.clone(), count: *count }
            }
            SessionEvent::ObjectiveReached { frame, objective } => {
                Event::ObjectiveReached { frame: *frame, objective: objective.clone() }
            }
        }
    }
}

impl Event {
    /// Back to the judge's vocabulary. An exit reason this side does not know
    /// is `Unknown`, which is what the runner would have said about it too.
    pub fn to_session(&self) -> SessionEvent {
        match self {
            Event::Entered { frame, stage } => SessionEvent::StageEntered {
                frame: *frame,
                stage: session_api::StageId(stage.clone()),
            },
            Event::Cleared { frame } => SessionEvent::StageCleared { frame: *frame },
            Event::Exited { frame, reason } => SessionEvent::StageExited {
                frame: *frame,
                reason: match reason.as_str() {
                    "death" => ExitReason::Death,
                    "quit" => ExitReason::Quit,
                    _ => ExitReason::Unknown,
                },
            },
            Event::Died { frame } => SessionEvent::Died { frame: *frame },
            Event::Damaged { frame } => SessionEvent::Damaged { frame: *frame },
            Event::EnemyDefeated { frame, count } => {
                SessionEvent::EnemyDefeated { frame: *frame, count: *count }
            }
            Event::ItemCollected { frame, item, count } => SessionEvent::ItemCollected {
                frame: *frame,
                item: item.clone(),
                count: *count,
            },
            Event::Counted { frame, counter, count } => SessionEvent::Counted {
                frame: *frame,
                counter: counter.clone(),
                count: *count,
            },
            Event::ObjectiveReached { frame, objective } => {
                SessionEvent::ObjectiveReached { frame: *frame, objective: objective.clone() }
            }
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    TimeTrial,
    Survival,
}

impl From<ChallengeKind> for Kind {
    fn from(k: ChallengeKind) -> Self {
        match k {
            ChallengeKind::TimeTrial => Kind::TimeTrial,
            ChallengeKind::Survival => Kind::Survival,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Ending {
    Completed,
    /// `how` is the attempt's end in words: "died", "left the stage (quit)".
    Failed { how: String },
    TimedOut,
    Forfeited,
}

/// What one run came to. The client sends its provisional one and the server
/// compares it with its own; they differ only by a bug (ADR 0009).
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RunOutcome {
    pub challenge: String,
    pub kind: Kind,
    pub ending: Ending,
    pub frames: u64,
    pub attempts: u32,
}

impl From<&challenge_core::RunResult> for RunOutcome {
    fn from(r: &challenge_core::RunResult) -> Self {
        RunOutcome {
            challenge: r.challenge.0.clone(),
            kind: r.kind.into(),
            ending: match &r.ending {
                challenge_core::Ending::Completed => Ending::Completed,
                challenge_core::Ending::Failed(how) => Ending::Failed { how: how.to_string() },
                challenge_core::Ending::TimedOut => Ending::TimedOut,
                challenge_core::Ending::Forfeited => Ending::Forfeited,
            },
            frames: r.frames,
            attempts: r.attempts,
        }
    }
}

impl RunOutcome {
    pub fn completed(&self) -> bool {
        self.ending == Ending::Completed
    }
}

// ----------------------------------------------------------------- results

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RoundEntry {
    pub player: String,
    pub outcome: RunOutcome,
    pub completed: bool,
    pub place: u32,
    /// The frames the round cost: the run's, or the limit if it did not
    /// complete (ADR 0026).
    pub frames: u64,
    pub success: bool,
    pub failure: bool,
    pub points: u32,
}

impl From<&challenge_core::RoundEntry> for RoundEntry {
    fn from(e: &challenge_core::RoundEntry) -> Self {
        RoundEntry {
            player: e.player.0.clone(),
            outcome: (&e.result).into(),
            completed: e.completed,
            place: e.place,
            frames: e.frames,
            success: e.success,
            failure: e.failure,
            points: e.points,
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Standing {
    pub player: String,
    pub rank: u32,
    pub rounds: u32,
    pub frames: u64,
    pub completions: u32,
    pub points: u32,
    pub wins: u32,
    pub failures: u32,
    pub streak: u32,
    /// The round index the player went out on, if they did.
    pub eliminated: Option<u64>,
}

impl From<&challenge_core::Standing> for Standing {
    fn from(s: &challenge_core::Standing) -> Self {
        Standing {
            player: s.player.0.clone(),
            rank: s.rank,
            rounds: s.rounds,
            frames: s.frames,
            completions: s.completions,
            points: s.points,
            wins: s.wins,
            failures: s.failures,
            streak: s.streak,
            eliminated: s.eliminated,
        }
    }
}

// ---------------------------------------------------------------- messages

/// Client to server.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToServer {
    /// First message on a connection. `library_hash` is the client's
    /// `Catalog::hash`, in hex; a lobby refuses a client holding another
    /// library (ADR 0009). `lobby` names the lobby to join; `None` asks for a
    /// new one. A networked server routes on it (ADR 0030); a lobby itself,
    /// already chosen, ignores it.
    Hello {
        name: String,
        library_hash: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lobby: Option<String>,
    },
    /// Host only.
    Settings(LobbySettings),
    /// Host only. Replaces the enabled set.
    Enable { challenges: Vec<ChallengeRef> },
    /// The client's file check, sent on joining and again whenever it changes.
    Files(FileReport),
    /// Host only.
    Start,
    /// This round's content is derived and the runner is at the entry point.
    /// `artifact` is the SHA-1 of the bytes the game runs on (ADR 0013).
    Prepared { round: u64, artifact: String },
    /// Sent instead of `Prepared`: this client cannot run the round — a dump,
    /// a core or the challenge itself is missing here. The round is void for
    /// everyone (ADR 0029).
    Unprepared { round: u64, reason: String },
    /// One advanced frame with what the runner reported on it. Sent for every
    /// frame that carried an event, and for the frame the client's own judge
    /// settled the run on, so the server can replay the judgement exactly.
    Frame { round: u64, frame: u64, events: Vec<Event> },
    /// The client's provisional verdict. The server checks it against its own.
    Finished { round: u64, outcome: RunOutcome },
    /// The player gave up at this frame.
    Forfeit { round: u64, frame: u64 },
    /// Host only. Ends the match where it stands.
    EndMatch,
    /// Host only, from the score screen: the same settings, a new seed.
    PlayAgain,
    /// Host only, from the score screen.
    BackToLobby,
    /// Watch `player`'s picture for this round, or stop with `None`. Only a
    /// player whose own run is over, or who is not in the round, may watch
    /// (ADR 0026); the server refuses the rest. Whoever is watched hears
    /// `Watched` (ADR 0032).
    Watch { round: u64, player: Option<String> },
    /// One encoded picture: an H.264 access unit in Annex B form, base64 in
    /// `data`, with its parameter sets in-band when `key`. `frame` is the
    /// emulated frame it shows (ADR 0005); `aspect` is the picture's shape as
    /// the core reports it. Sent only while watched, and relayed to each
    /// watcher as `ToClient::Video`.
    Video { round: u64, frame: u64, key: bool, width: u32, height: u32, aspect: f32, data: String },
    /// The sound under one or more pictures: IMA ADPCM, base64 in `data`, at
    /// the core's own `rate`, `channels` interleaved. Each chunk carries its
    /// own predictor, so a lost chunk is a gap and not a glitch. `frame` is
    /// the emulated frame the chunk begins on.
    Audio { round: u64, frame: u64, rate: u32, channels: u8, data: String },
}

/// Server to client.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToClient {
    Welcome { player: String, lobby: LobbyState },
    Refused { reason: String },
    Lobby(LobbyState),
    /// The next round. Everyone hears of it; only `players` prepare it. The
    /// eliminated wait it out, and will spectate it.
    Dealt {
        round: u64,
        challenge: ChallengeRef,
        kind: Kind,
        /// Rolls the challenge's patch slots, if it has any (ADR 0013).
        seed: u64,
        limit_frames: u64,
        /// Who plays this round.
        players: Vec<String>,
    },
    /// A player could not prepare the round, so nobody plays it and its
    /// challenge is withdrawn for the rest of the match (ADR 0029). The next
    /// `Dealt`, or `MatchOver`, follows.
    Voided { round: u64, player: String, reason: String },
    /// Every player is prepared on the same artifact: count down and go.
    Go { round: u64 },
    /// A player's run ended; the server's verdict. For the waiting view.
    PlayerFinished { round: u64, player: String, outcome: RunOutcome },
    RoundResult {
        round: u64,
        entries: Vec<RoundEntry>,
        eliminated: Vec<String>,
        standings: Vec<Standing>,
    },
    MatchOver { standings: Vec<Standing> },
    /// The match cannot go on: an artifact mismatch, a verdict the client and
    /// server disagree on, or a protocol breach. Fatal to the match, not to
    /// the lobby; a `MatchOver` follows.
    Error { message: String },
    /// How many are watching this client's run now. Encode and send while
    /// it is above zero, and start on a key frame whenever it rises (ADR
    /// 0032).
    Watched { round: u64, watchers: u32 },
    /// A watched player's picture, relayed as it was sent.
    Video {
        round: u64,
        player: String,
        frame: u64,
        key: bool,
        width: u32,
        height: u32,
        aspect: f32,
        data: String,
    },
    /// A watched player's sound, relayed as it was sent.
    Audio { round: u64, player: String, frame: u64, rate: u32, channels: u8, data: String },
}

/// `data` on the wire: standard base64, padded.
pub fn to_base64(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub fn from_base64(text: &str) -> Result<Vec<u8>, String> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.decode(text).map_err(|e| e.to_string())
}

/// How each end of a socket notices the other has gone. TCP alone does not:
/// a peer whose link dropped without a close is silent, not closed, for as
/// long as the operating system cares to wait. So each end pings every
/// `ping_every` and gives up after `silence` with nothing heard — a ping, a
/// pong, or a message. The pings are WebSocket control frames, not packets,
/// so they need no version: every WebSocket answers them.
///
/// The two ends agree only in that each pings well inside the other's
/// `silence`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Liveness {
    pub ping_every: Duration,
    pub silence: Duration,
}

impl Liveness {
    /// Rides out a Wi-Fi blip; ends a dead match in seconds, not minutes.
    pub const DEFAULT: Liveness = Liveness { ping_every: Duration::from_secs(2), silence: Duration::from_secs(10) };
}

/// What a socket's loop should do now.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Beat {
    Wait,
    Ping,
    /// Nothing heard for `silence`: the peer is gone.
    Silent,
}

/// One socket's [`Liveness`], kept by its loop.
#[derive(Clone, Debug)]
pub struct Heartbeat {
    rules: Liveness,
    heard: Instant,
    pinged: Instant,
}

impl Heartbeat {
    pub fn new(rules: Liveness, now: Instant) -> Heartbeat {
        Heartbeat { rules, heard: now, pinged: now }
    }

    /// Anything arrived from the peer.
    pub fn heard(&mut self, now: Instant) {
        self.heard = now;
    }

    pub fn check(&mut self, now: Instant) -> Beat {
        if now.duration_since(self.heard) >= self.rules.silence {
            Beat::Silent
        } else if now.duration_since(self.pinged) >= self.rules.ping_every {
            self.pinged = now;
            Beat::Ping
        } else {
            Beat::Wait
        }
    }

    pub fn silence(&self) -> Duration {
        self.rules.silence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_heartbeat_pings_on_its_beat_and_gives_up_after_silence() {
        let rules = Liveness { ping_every: Duration::from_secs(2), silence: Duration::from_secs(10) };
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let mut h = Heartbeat::new(rules, t0);
        assert_eq!(h.check(at(1)), Beat::Wait);
        assert_eq!(h.check(at(2)), Beat::Ping);
        assert_eq!(h.check(at(3)), Beat::Wait, "one ping per beat");
        assert_eq!(h.check(at(4)), Beat::Ping);
        h.heard(at(9));
        assert_eq!(h.check(at(18)), Beat::Ping, "hearing anything puts silence off");
        assert_eq!(h.check(at(19)), Beat::Silent);
    }

    #[test]
    fn packets_round_trip_and_carry_the_version() {
        let text = encode(&ToServer::Start);
        assert!(text.contains("\"v\":3"), "{text}");
        assert_eq!(decode::<ToServer>(&text).unwrap(), ToServer::Start);
    }

    #[test]
    fn another_version_is_refused_before_the_body_is_read() {
        let text = r#"{"v":99,"body":{"type":"start"}}"#;
        assert_eq!(decode::<ToServer>(text), Err(ProtocolError::Version { got: 99, want: 3 }));
    }

    #[test]
    fn a_stream_survives_the_wire() {
        let data = to_base64(&[0, 0, 0, 1, 0x67, 0xff]);
        assert_eq!(from_base64(&data).unwrap(), vec![0, 0, 0, 1, 0x67, 0xff]);
        let up = ToServer::Video {
            round: 2, frame: 300, key: true, width: 256, height: 240, aspect: 1.1428572, data: data.clone(),
        };
        assert_eq!(decode::<ToServer>(&encode(&up)).unwrap(), up);
        let down = ToClient::Video {
            round: 2, player: "B".into(), frame: 300, key: true, width: 256, height: 240, aspect: 1.1428572, data,
        };
        assert_eq!(decode::<ToClient>(&encode(&down)).unwrap(), down);
        let sound = ToServer::Audio { round: 2, frame: 300, rate: 32040, channels: 2, data: "AA==".into() };
        assert_eq!(decode::<ToServer>(&encode(&sound)).unwrap(), sound);
        let watch = ToServer::Watch { round: 2, player: None };
        assert_eq!(decode::<ToServer>(&encode(&watch)).unwrap(), watch);
        let watched = ToClient::Watched { round: 2, watchers: 1 };
        assert_eq!(decode::<ToClient>(&encode(&watched)).unwrap(), watched);
    }

    #[test]
    fn events_survive_the_wire() {
        let all = vec![
            SessionEvent::StageEntered { frame: 1, stage: session_api::StageId("1-1".into()) },
            SessionEvent::StageCleared { frame: 2 },
            SessionEvent::StageExited { frame: 3, reason: ExitReason::Quit },
            SessionEvent::Died { frame: 4 },
            SessionEvent::Damaged { frame: 5 },
            SessionEvent::EnemyDefeated { frame: 6, count: 2 },
            SessionEvent::ItemCollected { frame: 7, item: "banana".into(), count: 3 },
            SessionEvent::Counted { frame: 8, counter: "score_award".into(), count: 1 },
            SessionEvent::ObjectiveReached { frame: 9, objective: "boss".into() },
        ];
        for e in &all {
            let wire: Event = e.into();
            let back = decode::<Event>(&encode(&wire)).unwrap().to_session();
            assert_eq!(&back, e);
        }
    }

    #[test]
    fn formats_tag_their_kind() {
        let s = LobbySettings { format: Format::Strikes(2), ..Default::default() };
        let text = serde_json::to_string(&s).unwrap();
        assert!(text.contains(r#""format":{"kind":"strikes","n":2}"#), "{text}");
        assert_eq!(serde_json::from_str::<LobbySettings>(&text).unwrap(), s);
    }

    #[test]
    fn a_hello_without_a_lobby_leaves_it_off_the_wire() {
        let new = ToServer::Hello { name: "A".into(), library_hash: "h".into(), lobby: None };
        let text = encode(&new);
        assert!(!text.contains("lobby"), "{text}");
        assert_eq!(decode::<ToServer>(&text).unwrap(), new);
        let join = ToServer::Hello { name: "B".into(), library_hash: "h".into(), lobby: Some("KXQFM".into()) };
        assert_eq!(decode::<ToServer>(&encode(&join)).unwrap(), join);
    }

    #[test]
    fn a_void_survives_the_wire() {
        let up = ToServer::Unprepared { round: 3, reason: "no core".into() };
        assert_eq!(decode::<ToServer>(&encode(&up)).unwrap(), up);
        let down = ToClient::Voided { round: 3, player: "B".into(), reason: "no core".into() };
        assert_eq!(decode::<ToClient>(&encode(&down)).unwrap(), down);
    }
}
