//! A [`StageRunner`] that plays a script instead of a game.
//!
//! Every integration test runs on this, because a test that needs a ROM is a
//! manual test. What it replaces is the emulator and the RAM watcher; what it
//! keeps is the contract a driver sees, so a driver that works against a
//! [`FakeRunner`] works against `session-libretro`'s runner unchanged:
//!
//! * [`enter`](StageRunner::enter) resets the frame counter to 0, and the first
//!   frame [`advance`](StageRunner::advance) reports is 1.
//! * Events arrive on their frame, in the order they were scripted. Order on a
//!   shared frame is the only fact `challenge-core` has to go on when a fourth
//!   stomp and a death land together (ADR 0023), so it is kept exactly.
//! * [`SessionEvent::StageEntered`] never reaches a caller, because the real
//!   runner clears it before one can read it — which is why a driver tells a
//!   run when an attempt begins.
//! * A runner holds one piece of content, and entering a stage on another is
//!   refused rather than fixed up (ADR 0020).
//!
//! A script is a list of [`Attempt`]s, one per entry. A time trial that dies
//! twice and then clears is three of them; a survival run is one. When an
//! attempt's script runs out the player is still in the stage doing nothing,
//! and every later frame reports nothing — exactly what a player who put the
//! pad down produces. Only a run's limit or a forfeit ends that, as in a real
//! session, so a test driver that might hit it should pass a limit.
//!
//! The fake is stricter than the real runner in two places, both to catch a
//! driver that has lost track of the player: it is scripted for one stage and
//! refuses any other, and it refuses to be entered more times than it has
//! attempts, or advanced before it has been entered at all.
//!
//! Like every adapter, it never judges (ADR 0017). A script is a list of facts;
//! whether they complete a challenge is `challenge-core`'s question.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use session_api::{
    AssetKey, ChallengeSpec, Completion, EntryPoint, ExitReason, RunnerError, SessionEvent,
    StageId, StageRunner, StageSpec, DAMAGED, ENEMY_DEFEATED,
};

// ------------------------------------------------------------------- script

/// What one entry into the stage produces, frame by frame.
///
/// Built from facts and the frame each lands on, counted from entry. Frames may
/// be scripted in any order; events on the same frame keep the order they were
/// added in.
///
/// ```
/// use session_fake::Attempt;
/// // Three stomps, then a death on the frame of the fourth.
/// let attempt = Attempt::new()
///     .defeated(100, 1)
///     .defeated(200, 1)
///     .defeated(300, 1)
///     .died(450);
/// assert_eq!(attempt.last_frame(), 450);
/// ```
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Attempt {
    events: BTreeMap<u64, Vec<SessionEvent>>,
    /// The frame the runner fails on, and what it says. Nothing after it is
    /// reported.
    failure: Option<(u64, String)>,
}

impl Attempt {
    /// An attempt in which nothing happens: the player stands at the entry
    /// point for as long as the driver keeps advancing.
    pub fn new() -> Attempt {
        Attempt::default()
    }

    /// Any fact, on the frame it carries.
    ///
    /// # Panics
    ///
    /// On frame 0, which is the entry point itself. A runner's first reported
    /// frame is 1, so an event on 0 is one nobody would ever see.
    pub fn event(mut self, event: SessionEvent) -> Attempt {
        let frame = event.frame();
        assert!(frame > 0, "frame 0 is the entry point; the first frame a runner reports is 1");
        self.events.entry(frame).or_default().push(event);
        self
    }

    pub fn cleared(self, frame: u64) -> Attempt {
        self.event(SessionEvent::StageCleared { frame })
    }

    pub fn exited(self, frame: u64, reason: ExitReason) -> Attempt {
        self.event(SessionEvent::StageExited { frame, reason })
    }

    pub fn died(self, frame: u64) -> Attempt {
        self.event(SessionEvent::Died { frame })
    }

    pub fn damaged(self, frame: u64) -> Attempt {
        self.event(SessionEvent::Damaged { frame })
    }

    pub fn defeated(self, frame: u64, count: u32) -> Attempt {
        self.event(SessionEvent::EnemyDefeated { frame, count })
    }

    pub fn item(self, frame: u64, item: &str, count: u32) -> Attempt {
        self.event(SessionEvent::ItemCollected { frame, item: item.to_string(), count })
    }

    pub fn counted(self, frame: u64, counter: &str, count: u32) -> Attempt {
        self.event(SessionEvent::Counted { frame, counter: counter.to_string(), count })
    }

    pub fn objective(self, frame: u64, objective: &str) -> Attempt {
        self.event(SessionEvent::ObjectiveReached { frame, objective: objective.to_string() })
    }

    /// The runner breaks on `frame`: [`advance`](StageRunner::advance) returns
    /// an error instead of reaching it, and keeps returning it until the next
    /// entry. What a client whose core crashed looks like to its driver.
    ///
    /// # Panics
    ///
    /// On frame 0, for the reason [`event`](Attempt::event) does.
    pub fn fails_at(mut self, frame: u64, message: &str) -> Attempt {
        assert!(frame > 0, "frame 0 is the entry point; the first frame a runner reports is 1");
        self.failure = Some((frame, message.to_string()));
        self
    }

    /// An attempt in which every criterion of `complete_when` holds by
    /// `frame`: the tallies and objectives it asks for, then the clear if it
    /// asks for one, all on that frame.
    ///
    /// For scripting a player who does what was asked without writing out
    /// what that is, so a test can play whatever a session dealt. A tally is
    /// reported as [`Counted`](SessionEvent::Counted) unless it is one of the
    /// two built-in names — which event advanced a tally is not something a
    /// run can tell apart, and an item is a counter by another name.
    ///
    /// A survival criterion needs no event: the run completes on the frame it
    /// is reached, which may be before `frame` when it is the only criterion.
    ///
    /// # Panics
    ///
    /// If `frame` is 0, or earlier than the frame `survive_frames` asks for —
    /// the criteria cannot all hold there.
    pub fn meeting(complete_when: &Completion, frame: u64) -> Attempt {
        if let Some(n) = complete_when.survive_frames {
            assert!(frame >= n, "a run surviving {n} frames cannot meet its criteria by frame {frame}");
        }
        let mut a = Attempt::new();
        for (name, &n) in &complete_when.counts {
            a = match name.as_str() {
                DAMAGED => (0..n).fold(a, |a, _| a.damaged(frame)),
                ENEMY_DEFEATED => a.defeated(frame, n),
                counter => a.counted(frame, counter, n),
            };
        }
        for objective in &complete_when.objectives {
            a = a.objective(frame, objective);
        }
        if complete_when.cleared {
            a = a.cleared(frame);
        }
        a
    }

    /// The last frame anything is scripted on, failure included; 0 for an
    /// attempt that scripts nothing.
    pub fn last_frame(&self) -> u64 {
        let events = self.events.keys().next_back().copied().unwrap_or(0);
        let failure = self.failure.as_ref().map_or(0, |(f, _)| *f);
        events.max(failure)
    }
}

// ------------------------------------------------------------------- stages

/// A stage for a challenge, for a test that has a [`ChallengeSpec`] and no
/// manifest's runner half to resolve it through.
///
/// The content and entry point name nothing on disk — a fake never reads
/// either — but they are stable, so two runs of one challenge are entered at
/// the same stage, and runs of two games on two different pieces of content.
pub fn stage_for(challenge: &ChallengeSpec) -> StageSpec {
    StageSpec {
        game: challenge.game.clone(),
        stage: challenge.stage.clone(),
        content: AssetKey(format!("fake:{}", challenge.game)),
        entry: EntryPoint::Named(challenge.stage.0.clone()),
    }
}

// ------------------------------------------------------------------- runner

/// Plays a list of [`Attempt`]s, one per [`enter`](StageRunner::enter). See
/// the crate docs for the contract it keeps.
#[derive(Debug)]
pub struct FakeRunner {
    stage: StageSpec,
    attempts: Vec<Attempt>,
    /// How many times the stage has been entered. The live attempt, if any, is
    /// `attempts[entries - 1]`.
    entries: usize,
    frame: u64,
    /// Set once the live attempt has reached its failure, so every later
    /// advance fails the same way rather than skipping past it.
    failed: Option<String>,
}

impl FakeRunner {
    /// A runner scripted for `stage`, one attempt per entry, in order.
    pub fn new(stage: StageSpec, attempts: impl IntoIterator<Item = Attempt>) -> FakeRunner {
        FakeRunner {
            stage,
            attempts: attempts.into_iter().collect(),
            entries: 0,
            frame: 0,
            failed: None,
        }
    }

    /// The stage this runner was scripted for.
    pub fn stage(&self) -> &StageSpec {
        &self.stage
    }

    /// How many times the stage has been entered, the first time included.
    /// What a driver's re-entries are checked against.
    pub fn entries(&self) -> usize {
        self.entries
    }

    /// Scripted attempts that have not been entered yet.
    pub fn attempts_left(&self) -> usize {
        self.attempts.len() - self.entries
    }
}

impl StageRunner for FakeRunner {
    fn enter(&mut self, spec: &StageSpec) -> Result<(), RunnerError> {
        if spec.content != self.stage.content {
            return Err(RunnerError(format!(
                "{} runs on content {} and this runner holds {}; content changes at a \
                 challenge boundary, not inside one",
                spec.stage, spec.content, self.stage.content
            )));
        }
        if spec != &self.stage {
            return Err(RunnerError(format!(
                "this runner was scripted for {} and was entered at {}",
                describe(&self.stage.stage, &self.stage.entry),
                describe(&spec.stage, &spec.entry),
            )));
        }
        if self.entries == self.attempts.len() {
            return Err(RunnerError(format!(
                "{} was scripted for {} attempt(s) and entered again",
                spec.stage,
                self.attempts.len()
            )));
        }
        self.entries += 1;
        self.frame = 0;
        self.failed = None;
        Ok(())
    }

    fn advance(&mut self) -> Result<&[SessionEvent], RunnerError> {
        let Some(attempt) = self.entries.checked_sub(1).map(|i| &self.attempts[i]) else {
            return Err(RunnerError(format!(
                "{} was advanced before it was entered",
                self.stage.stage
            )));
        };
        if let Some(message) = &self.failed {
            return Err(RunnerError(message.clone()));
        }
        if let Some((at, message)) = &attempt.failure {
            if self.frame + 1 >= *at {
                self.failed = Some(message.clone());
                return Err(RunnerError(message.clone()));
            }
        }
        self.frame += 1;
        Ok(attempt.events.get(&self.frame).map_or(&[], Vec::as_slice))
    }

    fn frame(&self) -> u64 {
        self.frame
    }
}

fn describe(stage: &StageId, entry: &EntryPoint) -> String {
    match entry {
        EntryPoint::State(key) => format!("{stage} (state {key})"),
        EntryPoint::Named(name) => format!("{stage} (`{name}`)"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_api::{ChallengeId, ChallengeKind, GameId};

    fn stage() -> StageSpec {
        StageSpec {
            game: GameId("smb3".into()),
            stage: StageId("1-1".into()),
            content: AssetKey("fake:smb3".into()),
            entry: EntryPoint::Named("1-1".into()),
        }
    }

    /// Advances until `frame` and returns what each frame reported.
    fn advance_to(runner: &mut FakeRunner, frame: u64) -> Vec<(u64, Vec<SessionEvent>)> {
        let mut seen = Vec::new();
        while runner.frame() < frame {
            let events = runner.advance().unwrap().to_vec();
            seen.push((runner.frame(), events));
        }
        seen
    }

    #[test]
    fn the_first_frame_reported_is_one_and_entry_resets_it() {
        let mut r = FakeRunner::new(stage(), [Attempt::new(), Attempt::new()]);
        r.enter(&stage()).unwrap();
        assert_eq!(r.frame(), 0);
        r.advance().unwrap();
        assert_eq!(r.frame(), 1);
        advance_to(&mut r, 40);
        r.enter(&stage()).unwrap();
        assert_eq!(r.frame(), 0);
        assert_eq!(r.entries(), 2);
    }

    #[test]
    fn events_arrive_on_their_frame_in_the_order_scripted() {
        // Scripted out of frame order, and a death before an exit on one frame.
        let a = Attempt::new()
            .exited(5, ExitReason::Death)
            .defeated(2, 1)
            .event(SessionEvent::Died { frame: 5 });
        let mut r = FakeRunner::new(stage(), [a]);
        r.enter(&stage()).unwrap();
        let seen = advance_to(&mut r, 5);
        assert_eq!(seen[1], (2, vec![SessionEvent::EnemyDefeated { frame: 2, count: 1 }]));
        assert_eq!(
            seen[4].1,
            [
                SessionEvent::StageExited { frame: 5, reason: ExitReason::Death },
                SessionEvent::Died { frame: 5 },
            ]
        );
        assert!(seen.iter().filter(|(f, _)| ![2, 5].contains(f)).all(|(_, e)| e.is_empty()));
    }

    /// A player who put the pad down is still in the stage.
    #[test]
    fn past_the_end_of_the_script_nothing_happens_forever() {
        let mut r = FakeRunner::new(stage(), [Attempt::new().defeated(3, 1)]);
        r.enter(&stage()).unwrap();
        let seen = advance_to(&mut r, 10_000);
        assert_eq!(seen.iter().filter(|(_, e)| !e.is_empty()).count(), 1);
    }

    #[test]
    fn stage_entered_never_reaches_a_caller() {
        let mut r = FakeRunner::new(stage(), [Attempt::new().cleared(2)]);
        r.enter(&stage()).unwrap();
        let seen = advance_to(&mut r, 2);
        assert!(seen
            .iter()
            .flat_map(|(_, e)| e)
            .all(|e| !matches!(e, SessionEvent::StageEntered { .. })));
    }

    #[test]
    fn other_content_is_refused() {
        let mut r = FakeRunner::new(stage(), [Attempt::new()]);
        let mut other = stage();
        other.content = AssetKey("fake:smb3-hack".into());
        let err = r.enter(&other).unwrap_err();
        assert!(err.0.contains("challenge boundary"), "{err}");
        assert_eq!(r.entries(), 0);
    }

    #[test]
    fn another_stage_on_the_same_content_is_refused() {
        let mut r = FakeRunner::new(stage(), [Attempt::new()]);
        let mut other = stage();
        other.stage = StageId("1-2".into());
        other.entry = EntryPoint::Named("1-2".into());
        let err = r.enter(&other).unwrap_err();
        assert!(err.0.contains("scripted for 1-1"), "{err}");
    }

    #[test]
    fn entering_more_often_than_scripted_is_refused() {
        let mut r = FakeRunner::new(stage(), [Attempt::new().died(1)]);
        r.enter(&stage()).unwrap();
        assert_eq!(r.attempts_left(), 0);
        let err = r.enter(&stage()).unwrap_err();
        assert!(err.0.contains("1 attempt(s)"), "{err}");
    }

    #[test]
    fn advancing_before_entering_is_refused() {
        let mut r = FakeRunner::new(stage(), [Attempt::new()]);
        assert!(r.advance().is_err());
    }

    /// A crash stops the frame counter where it was, stays crashed, and the
    /// next entry plays the next attempt.
    #[test]
    fn a_scripted_failure_holds_until_the_next_entry() {
        let mut r = FakeRunner::new(
            stage(),
            [Attempt::new().cleared(9).fails_at(4, "core crashed"), Attempt::new().cleared(2)],
        );
        r.enter(&stage()).unwrap();
        advance_to(&mut r, 3);
        assert_eq!(r.advance().unwrap_err().0, "core crashed");
        assert_eq!(r.advance().unwrap_err().0, "core crashed");
        assert_eq!(r.frame(), 3);
        r.enter(&stage()).unwrap();
        let seen = advance_to(&mut r, 2);
        assert_eq!(seen[1].1, [SessionEvent::StageCleared { frame: 2 }]);
    }

    #[test]
    #[should_panic(expected = "frame 0 is the entry point")]
    fn an_event_on_the_entry_frame_is_a_script_bug() {
        Attempt::new().died(0);
    }

    #[test]
    fn meeting_reports_every_criterion_and_the_clear_last() {
        let c = Completion {
            cleared: true,
            counts: BTreeMap::from([
                (ENEMY_DEFEATED.to_string(), 4),
                (DAMAGED.to_string(), 2),
                ("kong_letter:1".to_string(), 1),
            ]),
            objectives: vec!["boss-defeated".into()],
            ..Default::default()
        };
        let a = Attempt::meeting(&c, 300);
        assert_eq!(a.last_frame(), 300);
        let events = &a.events[&300];
        assert_eq!(events.last(), Some(&SessionEvent::StageCleared { frame: 300 }));
        let tally = |name: &str| -> u32 {
            events.iter().filter_map(|e| e.tally()).filter(|(n, _)| *n == name).map(|(_, c)| c).sum()
        };
        assert_eq!(tally(ENEMY_DEFEATED), 4);
        assert_eq!(tally(DAMAGED), 2);
        assert_eq!(tally("kong_letter:1"), 1);
        assert!(events.contains(&SessionEvent::ObjectiveReached {
            frame: 300,
            objective: "boss-defeated".into()
        }));
    }

    #[test]
    #[should_panic(expected = "cannot meet its criteria")]
    fn meeting_a_survival_criterion_early_is_a_script_bug() {
        Attempt::meeting(&Completion { survive_frames: Some(100), ..Default::default() }, 99);
    }

    #[test]
    fn a_challenge_gets_a_stable_stage_on_its_game_s_content() {
        let c = |game: &str, id: &str| ChallengeSpec {
            id: ChallengeId(id.into()),
            name: id.into(),
            game: GameId(game.into()),
            stage: StageId("s".into()),
            kind: ChallengeKind::TimeTrial,
            complete_when: Completion { cleared: true, ..Default::default() },
            ends_attempt_on: Default::default(),
        };
        assert_eq!(stage_for(&c("dkc", "a")), stage_for(&c("dkc", "b")));
        assert_ne!(stage_for(&c("dkc", "a")).content, stage_for(&c("doom", "a")).content);
    }
}
