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

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use session_api::{ChallengeKind, ExitReason, SessionEvent};

pub const VERSION: u32 = 1;

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
    /// library (ADR 0009).
    Hello { name: String, library_hash: String },
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
}

/// Server to client.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ToClient {
    Welcome { player: String, lobby: LobbyState },
    Refused { reason: String },
    Lobby(LobbyState),
    /// The next round. Every player prepares it, the eliminated included:
    /// they spectate it.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packets_round_trip_and_carry_the_version() {
        let text = encode(&ToServer::Start);
        assert!(text.contains("\"v\":1"), "{text}");
        assert_eq!(decode::<ToServer>(&text).unwrap(), ToServer::Start);
    }

    #[test]
    fn another_version_is_refused_before_the_body_is_read() {
        let text = r#"{"v":99,"body":{"type":"start"}}"#;
        assert_eq!(decode::<ToServer>(text), Err(ProtocolError::Version { got: 99, want: 1 }));
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
}
