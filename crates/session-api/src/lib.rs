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

#![forbid(unsafe_code)]

use std::fmt;

// ------------------------------------------------------------------ identity

/// A game in the library. Matches the `id` of exactly one manifest.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct GameId(pub String);

/// A stage within a game, unique inside its manifest.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct StageId(pub String);

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

/// The two shapes a challenge can take. Whether a given stage can carry each
/// of them is a property of the stage, declared in its manifest — a
/// forced-scrolling level cannot be a time trial, because everyone finishes it
/// in the same number of frames.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum ChallengeKind {
    TimeTrial,
    Endurance,
}

impl fmt::Display for ChallengeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChallengeKind::TimeTrial => f.write_str("time_trial"),
            ChallengeKind::Endurance => f.write_str("endurance"),
        }
    }
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
            | SessionEvent::ItemCollected { frame, .. } => frame,
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
    /// run is the challenge's question, and an endurance stage answers it
    /// differently from a time trial.
    #[test]
    fn a_death_is_a_fact_not_a_verdict() {
        let e = SessionEvent::Died { frame: 42 };
        assert_eq!(e.frame(), 42);
        assert!(!e.is_terminal());
    }
}
