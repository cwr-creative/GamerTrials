//! The vocabulary shared between the challenge core and the adapters that run
//! a game. Owned by the consumer, never by an adapter (invariant 2), and free
//! of any word that only makes sense for one of them (invariant 3).
//!
//! Nothing here knows what a libretro core, a savestate, a WAD or a RAM address
//! is. `session-libretro` and `session-external` translate their own worlds into
//! these types; `challenge-core` reads them and decides what a run was worth.
//!
//! Two rules are load-bearing and are encoded in the types rather than written
//! down:
//!
//! * Every [`SessionEvent`] carries a `frame`, and nothing carries a wall-clock
//!   instant (ADR 0005).
//! * An event says what *happened*, never what it was worth. "The player reached
//!   the end of the stage" is a fact an adapter can observe; "the run passed" is
//!   a judgement that depends on the challenge, and belongs to `challenge-core`.
//!   That is why there is a [`SessionEvent::StageCleared`] and no `Passed`.
//!
//! Three parties, and the split between them is the point (ADR 0017):
//!
//! * an **adapter** produces facts — [`SessionEvent`];
//! * a **manifest** declares criteria — [`ChallengeSpec`] and [`Completion`],
//!   authored by whoever contributed the game;
//! * **`challenge-core`** compares one against the other and produces the
//!   verdict. It is the only one of the three that judges anything.
//!
//! [`Completion`] lives here rather than in an adapter because it is the
//! consumer's vocabulary: `challenge-core` must be able to read a challenge
//! without depending on whichever adapter happened to parse it.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fmt;

// ------------------------------------------------------------------ identity

/// A game in the library. Matches the `id` of exactly one manifest.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct GameId(pub String);

/// A stage within a game, unique inside its manifest.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StageId(pub String);

/// A challenge within a game, unique inside its manifest. What a player is
/// actually dealt — several may share one [`StageId`].
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ChallengeId(pub String);

/// A piece of content the store can hand back: an entry state, a ROM, a
/// generated artifact. A path today; a content hash once `asset-store` exists.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct AssetKey(pub String);

impl fmt::Display for GameId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for StageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for ChallengeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for AssetKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

// --------------------------------------------------------------- entry points

/// How a runner is told where a stage begins.
///
/// An enum rather than a path because the two adapters cannot be made to agree
/// on one: an emulator restores a captured machine state, and a PC port is told
/// a level name on its command line and starts it itself (ADR 0002). Neither
/// spelling is allowed to become the shared one.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EntryPoint {
    /// A previously captured machine state, restored wholesale. An emulator
    /// savestate is one form of this.
    State(AssetKey),
    /// A location the game itself can be asked to start at, by name.
    Named(String),
}

/// What a runner needs in order to put a player at the start of a stage.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StageSpec {
    pub game: GameId,
    pub stage: StageId,
    pub entry: EntryPoint,
}

// ------------------------------------------------------------------- timing

/// The rate the emulated machine advances at, used for exactly one thing:
/// turning a duration a manifest author wrote in seconds into the frames
/// everything downstream counts in. Seconds never reach a [`SessionEvent`].
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Framerate(pub f64);

impl Framerate {
    /// Rounds to the nearest frame. A manifest that says "survive 30 seconds"
    /// means the same number of frames on every machine running that game, so
    /// this conversion happens once, at load, and never again.
    pub fn frames(self, seconds: f64) -> u64 {
        (seconds * self.0).round().max(0.0) as u64
    }
}

// ------------------------------------------------------------------ challenge

/// The two shapes a challenge can take.
///
/// They differ in what a death costs and in what is scored — **not** in what
/// completes them, which is why one [`Completion`] vocabulary serves both:
///
/// * [`TimeTrial`](ChallengeKind::TimeTrial): a death re-enters at the stage's
///   entry point and the clock keeps running. The run is worth the total frames
///   across every attempt, which is the caller's arithmetic — see
///   [`StageRunner::enter`].
/// * [`Survival`](ChallengeKind::Survival): a death ends the run. Because of
///   that, "without dying" is never spelled out as a criterion; it is what the
///   kind means.
///
/// Which kinds a stage can carry is not declared anywhere: it is expressed by
/// which challenges a manifest defines. A forced-scrolling level simply has no
/// time trial, because everyone would finish it in the same number of frames.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ChallengeKind {
    TimeTrial,
    Survival,
}

impl fmt::Display for ChallengeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChallengeKind::TimeTrial => f.write_str("time_trial"),
            ChallengeKind::Survival => f.write_str("survival"),
        }
    }
}

/// What completes a run, declared by the manifest's author.
///
/// A conjunction: **every** criterion present must hold. There is no `any`, no
/// negation and no arithmetic, on purpose — the moment this grows an expression
/// it has become a scripting language living in a data file (ADR 0017).
///
/// Nothing here is evaluated in this crate. These are the criteria;
/// `challenge-core` is what compares [`SessionEvent`]s against them.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Completion {
    /// A [`SessionEvent::StageCleared`] was seen — the game itself signalled
    /// that the stage was finished.
    pub cleared: bool,
    /// The run reached this many frames. Already in frames: a manifest may
    /// author seconds, and [`Framerate::frames`] converts once, at load.
    ///
    /// Meaningful only for [`ChallengeKind::Survival`] — "survive 30 seconds
    /// fastest" is not something that can be won.
    pub survive_frames: Option<u64>,
    /// The [`SessionEvent::EnemyDefeated`] counts sum to at least this.
    pub enemies_defeated: Option<u32>,
    /// Per item name, the [`SessionEvent::ItemCollected`] counts that must sum.
    pub items_collected: BTreeMap<String, u32>,
    /// Every one of these must arrive as a [`SessionEvent::ObjectiveReached`].
    pub objectives: Vec<String>,
}

impl Completion {
    /// A challenge with no criteria at all can never be completed, which is a
    /// manifest bug rather than a challenge. Loaders refuse it.
    pub fn is_empty(&self) -> bool {
        !self.cleared
            && self.survive_frames.is_none()
            && self.enemies_defeated.is_none()
            && self.items_collected.is_empty()
            && self.objectives.is_empty()
    }
}

/// One challenge from the library: a stage, a kind, and what completes it.
///
/// This is the unit a session deals out. Several may name the same
/// [`StageId`] — the same place with different criteria is different
/// challenges, and that is the normal case rather than an edge case.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ChallengeSpec {
    pub id: ChallengeId,
    /// What to show a player. Defaults to the stage's name.
    pub name: String,
    pub game: GameId,
    pub stage: StageId,
    pub kind: ChallengeKind,
    pub complete_when: Completion,
}

// --------------------------------------------------------------------- events

/// Why an attempt left a stage without clearing it.
///
/// `Unknown` is not a placeholder to be filled in later — it is the honest
/// answer for a game that announces "the player is no longer in this level"
/// and nothing more. SMB3's return-to-map flag is exactly that: a level with no
/// end-of-level card cannot be told apart from a death by watching it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExitReason {
    Death,
    Quit,
    Unknown,
}

impl fmt::Display for ExitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExitReason::Death => f.write_str("death"),
            ExitReason::Quit => f.write_str("quit"),
            ExitReason::Unknown => f.write_str("unknown"),
        }
    }
}

/// Something a runner observed, and the emulated frame it happened on.
///
/// `frame` counts from stage entry, not from process start (ADR 0005): that is
/// what makes two players on two machines comparable at all.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SessionEvent {
    /// The player is at the start of `stage` and `frame` is now 0.
    StageEntered { frame: u64, stage: StageId },
    /// The game itself signalled that the stage was finished.
    StageCleared { frame: u64 },
    /// The player is out of the stage without having finished it.
    StageExited { frame: u64, reason: ExitReason },
    Died { frame: u64 },
    /// A hit that cost something other than a life.
    Damaged { frame: u64 },
    EnemyDefeated { frame: u64, count: u32 },
    ItemCollected { frame: u64, item: String, count: u32 },
    /// A named milestone the game announced: a boss's health reaching zero, a
    /// gate opening, a progress counter crossing a threshold.
    ///
    /// Still a fact, in exactly the sense the others are. A manifest names the
    /// milestone; whether reaching it completes anything is the challenge's
    /// question, which is why this is not `StageCleared`.
    ObjectiveReached { frame: u64, objective: String },
}

impl SessionEvent {
    pub fn frame(&self) -> u64 {
        match *self {
            SessionEvent::StageEntered { frame, .. }
            | SessionEvent::StageCleared { frame }
            | SessionEvent::StageExited { frame, .. }
            | SessionEvent::Died { frame }
            | SessionEvent::Damaged { frame }
            | SessionEvent::EnemyDefeated { frame, .. }
            | SessionEvent::ItemCollected { frame, .. }
            | SessionEvent::ObjectiveReached { frame, .. } => frame,
        }
    }

    /// Whether this event ends the attempt. Both terminal events are facts
    /// about the game, not verdicts: what an ending is *worth* is
    /// `challenge-core`'s question, and it needs a challenge to answer it.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SessionEvent::StageCleared { .. } | SessionEvent::StageExited { .. }
        )
    }
}

impl fmt::Display for SessionEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SessionEvent::StageEntered { frame, stage } => {
                write!(f, "frame {frame}  entered {stage}")
            }
            SessionEvent::StageCleared { frame } => write!(f, "frame {frame}  stage cleared"),
            SessionEvent::StageExited { frame, reason } => {
                write!(f, "frame {frame}  stage exited ({reason})")
            }
            SessionEvent::Died { frame } => write!(f, "frame {frame}  died"),
            SessionEvent::Damaged { frame } => write!(f, "frame {frame}  damaged"),
            SessionEvent::EnemyDefeated { frame, count } => {
                write!(f, "frame {frame}  defeated {count} enemy/enemies")
            }
            SessionEvent::ItemCollected { frame, item, count } => {
                write!(f, "frame {frame}  collected {count} x {item}")
            }
            SessionEvent::ObjectiveReached { frame, objective } => {
                write!(f, "frame {frame}  reached {objective}")
            }
        }
    }
}

// --------------------------------------------------------------------- ports

/// What went wrong in an adapter, in words the caller can show a player.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunnerError(pub String);

impl fmt::Display for RunnerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for RunnerError {}

impl From<String> for RunnerError {
    fn from(s: String) -> Self {
        RunnerError(s)
    }
}

/// Runs one stage of one game and reports what happens in it.
///
/// The interface is deliberately this small. Everything an adapter needs that
/// is not here — which core to load, which ROM, which button is jump — is the
/// adapter's own business, configured before a `StageRunner` is handed out.
pub trait StageRunner {
    /// Put the player at the start of `spec` and reset the frame counter to 0.
    ///
    /// Resetting to 0 is deliberate, and it is why a time trial's total is the
    /// caller's arithmetic rather than a runner's: a death in a time trial is a
    /// second `enter` on the same stage, and the run is worth the sum of the
    /// frames each attempt reached. A runner counts one attempt and knows
    /// nothing about the challenge it belongs to (ADR 0005, ADR 0017).
    fn enter(&mut self, spec: &StageSpec) -> Result<(), RunnerError>;

    /// Advance exactly one emulated frame and report what happened on it.
    ///
    /// One frame, not "some time": the caller decides pacing, and a replay or a
    /// headless test advances as fast as it likes without changing a single
    /// reported number.
    fn advance(&mut self) -> Result<&[SessionEvent], RunnerError>;

    /// Emulated frames since the stage was entered.
    fn frame(&self) -> u64;
}

/// The most recently completed picture, borrowed for as long as it takes to
/// copy it somewhere.
pub struct VideoFrame<'a> {
    pub pixels: &'a [u32],
    pub width: usize,
    pub height: usize,
    /// Increases once per *new* picture. A core that dupes a frame does not
    /// bump it, so a presenter can skip the repaint and still pump its window.
    pub seq: u64,
}

/// A runner whose output can be put on a screen — locally, or encoded for a
/// spectator (ADR 0003).
pub trait Presentable {
    /// Calls `f` with the latest frame if it is newer than `since`, and returns
    /// the sequence number of whatever is current.
    ///
    /// A callback rather than a returned borrow because the frame usually lives
    /// behind a lock the adapter owns, and handing that lifetime out would make
    /// every implementation contort around it.
    fn present(&self, since: u64, f: &mut dyn FnMut(VideoFrame<'_>)) -> u64;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seconds_become_frames_at_the_game_rate() {
        // NES: 60.0988 Hz, so 30 s is not 1800 frames.
        assert_eq!(Framerate(60.0988).frames(30.0), 1803);
        // SNES: 60.0988 as well on NTSC, but a manifest states its own rate.
        assert_eq!(Framerate(60.0).frames(30.0), 1800);
        assert_eq!(Framerate(60.0).frames(0.0), 0);
    }

    #[test]
    fn only_stage_endings_are_terminal() {
        assert!(SessionEvent::StageCleared { frame: 1 }.is_terminal());
        assert!(SessionEvent::StageExited { frame: 1, reason: ExitReason::Death }.is_terminal());
        assert!(!SessionEvent::Died { frame: 1 }.is_terminal());
        assert!(!SessionEvent::Damaged { frame: 1 }.is_terminal());
    }

    /// A death is reported as a death, not as a failure: whether dying ends the
    /// run is the challenge's question, and a survival run answers it
    /// differently from a time trial.
    #[test]
    fn a_death_is_a_fact_not_a_verdict() {
        let e = SessionEvent::Died { frame: 42 };
        assert_eq!(e.frame(), 42);
        assert!(!e.is_terminal());
    }

    /// An objective is a milestone the game announced, not the end of anything.
    /// Whether reaching it completes a run is `complete_when`'s answer.
    #[test]
    fn an_objective_is_a_fact_not_an_ending() {
        let e = SessionEvent::ObjectiveReached {
            frame: 900,
            objective: "boss-defeated".to_string(),
        };
        assert_eq!(e.frame(), 900);
        assert!(!e.is_terminal());
    }

    /// A challenge nobody can ever complete is a manifest bug, and the loader
    /// leans on this to say so.
    #[test]
    fn a_completion_with_no_criteria_is_empty() {
        assert!(Completion::default().is_empty());
        assert!(!Completion { cleared: true, ..Default::default() }.is_empty());
        assert!(!Completion { survive_frames: Some(1800), ..Default::default() }.is_empty());
        assert!(!Completion {
            objectives: vec!["boss-defeated".to_string()],
            ..Default::default()
        }
        .is_empty());
    }
}
