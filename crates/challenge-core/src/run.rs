//! One player's go at one challenge.
//!
//! A [`Run`] is fed what a runner reported, one frame at a time, and answers
//! with what should happen next: keep going, re-enter the stage, or stop. It
//! never touches the runner itself.
//!
//! ```text
//! let mut run = Run::new(challenge)?;
//! runner.enter(&stage)?;  run.enter();
//! loop {
//!     let events = runner.advance()?;
//!     match run.observe(runner.frame(), events) {
//!         Next::Continue => {}
//!         Next::Reenter => { runner.enter(&stage)?; run.enter(); }
//!         Next::Finished(result) => break result,
//!     }
//! }
//! ```
//!
//! Four rules decide everything, and each is a choice rather than a
//! consequence, so each is written down here (ADR 0023):
//!
//! * **A death ends the attempt, whether or not the stage is left.** The two
//!   kinds are defined by what a death costs, so a death is the event that
//!   matters — not a [`StageExited`](SessionEvent::StageExited) that may or
//!   may not follow it. Doom announces a death and stays in the level; waiting
//!   for an exit there would leave a time trial waiting forever.
//! * **Criteria are met within one attempt.** Re-entering puts the stage back
//!   as it was, enemies and items included, so a count carried across attempts
//!   could be farmed by dying. A time trial's *frames* carry over; its
//!   *progress* does not.
//! * **A run completes the moment its criteria hold**, not at the end of the
//!   stage. "Stomp four enemies" stops the clock on the fourth stomp; reaching
//!   the goal is only required when `cleared` says so.
//! * **The bound is the caller's.** ADR 0017 owes a failure bound and does not
//!   say where it comes from. A run takes a frame limit and a forfeit, which
//!   serves a per-challenge limit, a per-round cap and a give-up button
//!   equally; choosing among them is a session decision this crate does not
//!   make.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use session_api::{ChallengeId, ChallengeKind, ChallengeSpec, Completion, ExitReason, SessionEvent};

/// Why a [`Run`] could not be built.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RunError {
    /// A challenge with no criteria holds on its first frame. Loaders refuse
    /// it already; this is the same refusal, made where it would bite.
    NoCriteria(ChallengeId),
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RunError::NoCriteria(id) => {
                write!(f, "challenge `{id}` declares no criteria, so it cannot be judged")
            }
        }
    }
}

impl std::error::Error for RunError {}

/// How an attempt ended without completing the run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AttemptEnd {
    Died,
    /// The game said the stage was finished, but the criteria did not hold —
    /// the player reached the goal without the four stomps.
    Cleared,
    /// The player left the stage some other way. `Unknown` is SMB3 leaving a
    /// level and not saying how, and it is reported as exactly that.
    Exited(ExitReason),
}

impl fmt::Display for AttemptEnd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AttemptEnd::Died => f.write_str("died"),
            AttemptEnd::Cleared => f.write_str("cleared the stage without meeting the criteria"),
            AttemptEnd::Exited(reason) => write!(f, "left the stage ({reason})"),
        }
    }
}

/// How a run ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ending {
    /// Every criterion held within one attempt.
    Completed,
    /// A survival run's one attempt ended first. A time trial never ends this
    /// way: an attempt that ends short is a re-entry, not a result.
    Failed(AttemptEnd),
    /// The caller's frame limit was reached first.
    TimedOut,
    /// The caller gave up on the run.
    Forfeited,
}

impl fmt::Display for Ending {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Ending::Completed => f.write_str("completed"),
            Ending::Failed(end) => write!(f, "failed: {end}"),
            Ending::TimedOut => f.write_str("timed out"),
            Ending::Forfeited => f.write_str("forfeited"),
        }
    }
}

/// What a finished run was worth. Everything [`rank`](crate::rank) needs, and
/// nothing about who ran it — that is the caller's to attach.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RunResult {
    pub challenge: ChallengeId,
    pub kind: ChallengeKind,
    pub ending: Ending,
    /// Emulated frames across every attempt, up to the frame the run ended
    /// on. A time trial is scored on this; a survival run carries it for
    /// display only.
    pub frames: u64,
    /// How many times the stage was entered, the first time included.
    pub attempts: u32,
}

impl RunResult {
    pub fn completed(&self) -> bool {
        self.ending == Ending::Completed
    }
}

impl fmt::Display for RunResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = if self.attempts == 1 { "" } else { "s" };
        write!(
            f,
            "{} ({}): {} in {} frames over {} attempt{s}",
            self.challenge, self.kind, self.ending, self.frames, self.attempts
        )
    }
}

/// What the caller should do after [`Run::observe`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Next {
    /// Advance another frame.
    Continue,
    /// The attempt ended and the run goes on: enter the stage again, then call
    /// [`Run::enter`]. Only a time trial asks for this.
    Reenter,
    /// The run is over. Stop advancing.
    Finished(RunResult),
}

#[derive(Debug)]
enum State {
    /// Built, or between attempts: the caller owes an [`Run::enter`].
    AwaitingEntry,
    Live,
    Over(RunResult),
}

/// Progress inside one attempt. Reset on every entry.
#[derive(Default, Debug)]
struct Progress {
    cleared: bool,
    enemies: u32,
    items: BTreeMap<String, u32>,
    objectives: BTreeSet<String>,
}

impl Progress {
    /// The conjunction. Every criterion present must hold, and an absent one
    /// holds trivially; [`Completion::is_empty`] is what stops that making an
    /// empty challenge free.
    fn meets(&self, c: &Completion, frame: u64) -> bool {
        (!c.cleared || self.cleared)
            && c.survive_frames.map_or(true, |n| frame >= n)
            && c.enemies_defeated.map_or(true, |n| self.enemies >= n)
            && c.items_collected
                .iter()
                .all(|(item, n)| self.items.get(item).copied().unwrap_or(0) >= *n)
            && c.objectives.iter().all(|o| self.objectives.contains(o))
    }
}

/// One player's go at one challenge. See the module docs for the rules.
#[derive(Debug)]
pub struct Run {
    spec: ChallengeSpec,
    limit: Option<u64>,
    state: State,
    progress: Progress,
    /// Frames from attempts that have already ended.
    banked: u64,
    /// The current attempt's frame, as last observed.
    frame: u64,
    attempts: u32,
}

impl Run {
    pub fn new(spec: ChallengeSpec) -> Result<Run, RunError> {
        if spec.complete_when.is_empty() {
            return Err(RunError::NoCriteria(spec.id));
        }
        Ok(Run {
            spec,
            limit: None,
            state: State::AwaitingEntry,
            progress: Progress::default(),
            banked: 0,
            frame: 0,
            attempts: 0,
        })
    }

    /// End the run as [`Ending::TimedOut`] once this many frames have passed
    /// across every attempt. Without one a run can go on forever, which is the
    /// gap ADR 0017 says must close before either kind ships.
    pub fn with_limit(mut self, frames: u64) -> Run {
        self.limit = Some(frames);
        self
    }

    pub fn spec(&self) -> &ChallengeSpec {
        &self.spec
    }

    /// Frames across every attempt so far, the current one included. What a
    /// time trial's clock shows.
    pub fn frames(&self) -> u64 {
        match &self.state {
            State::Over(r) => r.frames,
            _ => self.banked + self.frame,
        }
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn result(&self) -> Option<&RunResult> {
        match &self.state {
            State::Over(r) => Some(r),
            _ => None,
        }
    }

    /// The caller has just put the player at the start of the stage, and the
    /// runner's frame counter is 0. Starts an attempt.
    ///
    /// # Panics
    ///
    /// If an attempt is already live or the run is over. Either means the
    /// caller and the run disagree about what the player is doing, and
    /// carrying on would score the wrong thing.
    pub fn enter(&mut self) {
        match self.state {
            State::AwaitingEntry => {}
            State::Live => panic!("Run::enter during a live attempt of `{}`", self.spec.id),
            State::Over(_) => panic!("Run::enter after `{}` finished", self.spec.id),
        }
        self.state = State::Live;
        self.progress = Progress::default();
        self.frame = 0;
        self.attempts += 1;
    }

    /// One advanced frame: the runner's frame counter after the advance, and
    /// the events it reported for that frame, in the order it reported them.
    ///
    /// Events are taken in order and the first one that settles the attempt
    /// wins; anything after it on the same frame is ignored. Order matters on
    /// exactly one kind of frame — a fourth stomp and a death together — and
    /// the runner's order is the only fact there is to go on.
    ///
    /// # Panics
    ///
    /// If no attempt is live: before the first [`enter`](Run::enter), after a
    /// [`Next::Reenter`] that was not followed by one, or after the run
    /// finished.
    pub fn observe(&mut self, frame: u64, events: &[SessionEvent]) -> Next {
        match self.state {
            State::Live => {}
            State::AwaitingEntry => {
                panic!("Run::observe on `{}` with no attempt live; call enter first", self.spec.id)
            }
            State::Over(_) => panic!("Run::observe after `{}` finished", self.spec.id),
        }
        self.frame = frame;

        for event in events {
            let end = match event {
                SessionEvent::StageEntered { .. } | SessionEvent::Damaged { .. } => None,
                SessionEvent::Died { .. } => Some(AttemptEnd::Died),
                SessionEvent::StageExited { reason, .. } => Some(AttemptEnd::Exited(*reason)),
                SessionEvent::StageCleared { .. } => {
                    self.progress.cleared = true;
                    Some(AttemptEnd::Cleared)
                }
                SessionEvent::EnemyDefeated { count, .. } => {
                    self.progress.enemies = self.progress.enemies.saturating_add(*count);
                    None
                }
                SessionEvent::ItemCollected { item, count, .. } => {
                    let held = self.progress.items.entry(item.clone()).or_default();
                    *held = held.saturating_add(*count);
                    None
                }
                SessionEvent::ObjectiveReached { objective, .. } => {
                    self.progress.objectives.insert(objective.clone());
                    None
                }
            };
            // A death is never what completes a run, even when the criteria
            // would otherwise hold on its frame: surviving a frame means being
            // alive at the end of it.
            if end != Some(AttemptEnd::Died) && self.progress.meets(&self.spec.complete_when, frame)
            {
                return self.finish(Ending::Completed, event.frame());
            }
            if let Some(end) = end {
                return self.end_attempt(end, event.frame());
            }
        }

        // No event this frame, or none that settled anything: this is where a
        // survival criterion with nothing to announce it completes.
        if self.progress.meets(&self.spec.complete_when, frame) {
            return self.finish(Ending::Completed, frame);
        }
        if self.limit.is_some_and(|limit| self.banked + frame >= limit) {
            return self.finish(Ending::TimedOut, frame);
        }
        Next::Continue
    }

    /// Give up. Frames up to the last observed frame count; nothing after.
    /// Returns the result; a run that already finished keeps the result it had.
    pub fn forfeit(&mut self) -> RunResult {
        if let State::Over(r) = &self.state {
            return r.clone();
        }
        match self.finish(Ending::Forfeited, self.frame) {
            Next::Finished(r) => r,
            _ => unreachable!("finish always finishes"),
        }
    }

    fn end_attempt(&mut self, end: AttemptEnd, frame: u64) -> Next {
        match self.spec.kind {
            ChallengeKind::Survival => self.finish(Ending::Failed(end), frame),
            ChallengeKind::TimeTrial => {
                self.banked += frame;
                self.frame = 0;
                if self.limit.is_some_and(|limit| self.banked >= limit) {
                    return self.finish(Ending::TimedOut, 0);
                }
                self.state = State::AwaitingEntry;
                Next::Reenter
            }
        }
    }

    /// `frame` is the current attempt's frame the run ended on.
    fn finish(&mut self, ending: Ending, frame: u64) -> Next {
        let mut frames = self.banked + frame;
        // A limit is a ceiling: a run that overshoots it by finishing an
        // attempt past the line is scored at the line.
        if ending == Ending::TimedOut {
            if let Some(limit) = self.limit {
                frames = frames.min(limit);
            }
        }
        let result = RunResult {
            challenge: self.spec.id.clone(),
            kind: self.spec.kind,
            ending,
            frames,
            attempts: self.attempts,
        };
        self.state = State::Over(result.clone());
        Next::Finished(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_api::{GameId, StageId};

    fn spec(kind: ChallengeKind, complete_when: Completion) -> ChallengeSpec {
        ChallengeSpec {
            id: ChallengeId("c".into()),
            name: "C".into(),
            game: GameId("g".into()),
            stage: StageId("s".into()),
            kind,
            complete_when,
        }
    }

    fn reach_the_end() -> Completion {
        Completion { cleared: true, ..Default::default() }
    }

    fn stomps(n: u32) -> Completion {
        Completion { enemies_defeated: Some(n), ..Default::default() }
    }

    fn survive(frames: u64) -> Completion {
        Completion { survive_frames: Some(frames), ..Default::default() }
    }

    /// Feeds `script` — `(frame, events)` pairs — advancing frame by frame
    /// in between, re-entering whenever asked. Returns the result, or panics if
    /// the script runs out first. What a driver does, minus the runner.
    fn play(mut run: Run, attempts: &[&[(u64, Vec<SessionEvent>)]]) -> RunResult {
        for script in attempts {
            run.enter();
            let last = script.last().map_or(0, |(f, _)| *f);
            for frame in 1..=last {
                let events = script
                    .iter()
                    .find(|(f, _)| *f == frame)
                    .map(|(_, e)| e.as_slice())
                    .unwrap_or(&[]);
                match run.observe(frame, events) {
                    Next::Continue => {}
                    Next::Reenter => break,
                    Next::Finished(r) => return r,
                }
            }
        }
        panic!("the script ended with the run still going");
    }

    fn died(frame: u64) -> SessionEvent {
        SessionEvent::Died { frame }
    }
    fn cleared(frame: u64) -> SessionEvent {
        SessionEvent::StageCleared { frame }
    }
    fn stomp(frame: u64) -> SessionEvent {
        SessionEvent::EnemyDefeated { frame, count: 1 }
    }

    #[test]
    fn a_challenge_with_no_criteria_is_refused() {
        let err = Run::new(spec(ChallengeKind::TimeTrial, Completion::default())).unwrap_err();
        assert_eq!(err, RunError::NoCriteria(ChallengeId("c".into())));
    }

    #[test]
    fn a_clean_time_trial_is_worth_the_frame_it_cleared_on() {
        let run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end())).unwrap();
        let r = play(run, &[&[(900, vec![cleared(900)])]]);
        assert_eq!(r.ending, Ending::Completed);
        assert_eq!((r.frames, r.attempts), (900, 1));
    }

    /// The clock keeps running through a death: the run is worth every
    /// attempt's frames, not the last one's.
    #[test]
    fn a_time_trial_death_re_enters_and_banks_the_frames() {
        let run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end())).unwrap();
        let r = play(
            run,
            &[
                &[(400, vec![died(400)])],
                &[(250, vec![died(250)])],
                &[(1000, vec![cleared(1000)])],
            ],
        );
        assert_eq!(r.ending, Ending::Completed);
        assert_eq!((r.frames, r.attempts), (1650, 3));
    }

    #[test]
    fn a_survival_death_ends_the_run() {
        let run = Run::new(spec(ChallengeKind::Survival, reach_the_end())).unwrap();
        let r = play(run, &[&[(400, vec![died(400)])]]);
        assert_eq!(r.ending, Ending::Failed(AttemptEnd::Died));
        assert_eq!((r.frames, r.attempts), (400, 1));
        assert!(!r.completed());
    }

    /// Doom: a death with no exit after it. Waiting for the exit would leave
    /// the time trial waiting forever.
    #[test]
    fn a_death_ends_the_attempt_without_waiting_for_the_stage_to_be_left() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end())).unwrap();
        run.enter();
        assert_eq!(run.observe(1, &[]), Next::Continue);
        assert_eq!(run.observe(2, &[died(2)]), Next::Reenter);
    }

    /// DKC: a death and an exit on the same frame are one ending, not two.
    #[test]
    fn whatever_follows_the_ending_on_the_same_frame_is_ignored() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end())).unwrap();
        run.enter();
        let exit = SessionEvent::StageExited { frame: 5, reason: ExitReason::Death };
        assert_eq!(run.observe(5, &[died(5), exit]), Next::Reenter);
        run.enter();
        assert_eq!(run.attempts(), 2);
        assert_eq!(run.frames(), 5);
    }

    /// SMB3: leaving a level without saying how. Not a clear, so not a
    /// completion; a time trial goes again.
    #[test]
    fn an_exit_of_unknown_cause_is_an_attempt_that_ended_short() {
        let run = Run::new(spec(ChallengeKind::Survival, reach_the_end())).unwrap();
        let exit = SessionEvent::StageExited { frame: 70, reason: ExitReason::Unknown };
        let r = play(run, &[&[(70, vec![exit])]]);
        assert_eq!(r.ending, Ending::Failed(AttemptEnd::Exited(ExitReason::Unknown)));
    }

    /// "The clock stops on the fourth stomp; reaching the card is not
    /// required."
    #[test]
    fn a_run_completes_the_moment_its_criteria_hold() {
        let run = Run::new(spec(ChallengeKind::TimeTrial, stomps(4))).unwrap();
        let r = play(
            run,
            &[&[
                (100, vec![stomp(100)]),
                (200, vec![stomp(200)]),
                (300, vec![stomp(300)]),
                (450, vec![stomp(450)]),
                (900, vec![cleared(900)]),
            ]],
        );
        assert_eq!(r.ending, Ending::Completed);
        assert_eq!(r.frames, 450);
    }

    #[test]
    fn reaching_the_goal_without_the_criteria_does_not_complete() {
        let run = Run::new(spec(ChallengeKind::Survival, stomps(4))).unwrap();
        let r = play(run, &[&[(100, vec![stomp(100)]), (900, vec![cleared(900)])]]);
        assert_eq!(r.ending, Ending::Failed(AttemptEnd::Cleared));
    }

    /// Re-entry restores the stage, so progress is per attempt: three stomps,
    /// a death, and one more stomp is one stomp, not four.
    #[test]
    fn progress_does_not_carry_across_attempts() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, stomps(4))).unwrap();
        run.enter();
        for f in 1..=3 {
            assert_eq!(run.observe(f, &[stomp(f)]), Next::Continue);
        }
        assert_eq!(run.observe(4, &[died(4)]), Next::Reenter);
        run.enter();
        assert_eq!(run.observe(1, &[stomp(1)]), Next::Continue);
        assert_eq!(run.frames(), 5);
    }

    /// Counts sum, and every criterion must hold at once.
    #[test]
    fn every_criterion_must_hold_together() {
        let c = Completion {
            cleared: true,
            items_collected: BTreeMap::from([("banana".to_string(), 50)]),
            objectives: vec!["boss-defeated".into()],
            ..Default::default()
        };
        let mut run = Run::new(spec(ChallengeKind::Survival, c)).unwrap();
        run.enter();
        let bananas = |frame, count| SessionEvent::ItemCollected {
            frame,
            item: "banana".into(),
            count,
        };
        let boss = SessionEvent::ObjectiveReached { frame: 3, objective: "boss-defeated".into() };
        assert_eq!(run.observe(1, &[bananas(1, 30)]), Next::Continue);
        assert_eq!(run.observe(2, &[bananas(2, 20)]), Next::Continue);
        assert_eq!(run.observe(3, &[boss]), Next::Continue);
        let Next::Finished(r) = run.observe(4, &[cleared(4)]) else {
            panic!("all three criteria hold on frame 4");
        };
        assert_eq!((r.ending, r.frames), (Ending::Completed, 4));
    }

    /// A survival criterion has no event to announce it. It completes on the
    /// frame it is reached, with nothing reported at all.
    #[test]
    fn surviving_completes_on_the_frame_it_is_reached() {
        let mut run = Run::new(spec(ChallengeKind::Survival, survive(3))).unwrap();
        run.enter();
        assert_eq!(run.observe(1, &[]), Next::Continue);
        assert_eq!(run.observe(2, &[]), Next::Continue);
        let Next::Finished(r) = run.observe(3, &[]) else { panic!("frame 3 is survived") };
        assert_eq!((r.ending, r.frames), (Ending::Completed, 3));
    }

    /// Surviving a frame means being alive at the end of it.
    #[test]
    fn dying_on_the_last_frame_is_not_surviving_it() {
        let mut run = Run::new(spec(ChallengeKind::Survival, survive(3))).unwrap();
        run.enter();
        run.observe(1, &[]);
        run.observe(2, &[]);
        let Next::Finished(r) = run.observe(3, &[died(3)]) else { panic!("the run is over") };
        assert_eq!(r.ending, Ending::Failed(AttemptEnd::Died));
    }

    /// The runner's order is the only fact there is: a stomp reported before
    /// the death on the same frame counts.
    #[test]
    fn on_a_shared_frame_the_first_event_to_settle_the_attempt_wins() {
        let mut run = Run::new(spec(ChallengeKind::Survival, stomps(1))).unwrap();
        run.enter();
        let Next::Finished(r) = run.observe(9, &[stomp(9), died(9)]) else { panic!() };
        assert_eq!(r.ending, Ending::Completed);

        let mut run = Run::new(spec(ChallengeKind::Survival, stomps(1))).unwrap();
        run.enter();
        let Next::Finished(r) = run.observe(9, &[died(9), stomp(9)]) else { panic!() };
        assert_eq!(r.ending, Ending::Failed(AttemptEnd::Died));
    }

    /// The failure bound. Counted across attempts, because a time trial's
    /// clock is.
    #[test]
    fn a_limit_ends_a_run_that_never_finishes() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end()))
            .unwrap()
            .with_limit(10);
        run.enter();
        for f in 1..=6 {
            assert_eq!(run.observe(f, &[]), Next::Continue);
        }
        assert_eq!(run.observe(7, &[died(7)]), Next::Reenter);
        run.enter();
        assert_eq!(run.observe(1, &[]), Next::Continue);
        assert_eq!(run.observe(2, &[]), Next::Continue);
        let Next::Finished(r) = run.observe(3, &[]) else { panic!("7 + 3 reaches the limit") };
        assert_eq!(r.ending, Ending::TimedOut);
        assert_eq!((r.frames, r.attempts), (10, 2));
    }

    /// Completing on the limit's own frame is completing.
    #[test]
    fn completing_on_the_last_allowed_frame_counts() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end()))
            .unwrap()
            .with_limit(5);
        run.enter();
        for f in 1..=4 {
            run.observe(f, &[]);
        }
        let Next::Finished(r) = run.observe(5, &[cleared(5)]) else { panic!() };
        assert_eq!(r.ending, Ending::Completed);
    }

    /// A death that lands past the limit is scored at the limit, not at the
    /// death.
    #[test]
    fn a_death_past_the_limit_times_out_at_the_limit() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end()))
            .unwrap()
            .with_limit(10);
        run.enter();
        // Frames are observed one at a time in practice, but nothing stops a
        // replay handing over a jump.
        let Next::Finished(r) = run.observe(12, &[died(12)]) else { panic!() };
        assert_eq!((r.ending, r.frames), (Ending::TimedOut, 10));
    }

    #[test]
    fn a_forfeit_counts_up_to_the_last_observed_frame() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end())).unwrap();
        run.enter();
        run.observe(1, &[]);
        run.observe(2, &[died(2)]);
        run.enter();
        run.observe(1, &[]);
        let r = run.forfeit();
        assert_eq!(r.ending, Ending::Forfeited);
        assert_eq!((r.frames, r.attempts), (3, 2));
        assert_eq!(run.forfeit(), r, "a finished run keeps its result");
    }

    #[test]
    fn damage_is_not_an_ending() {
        let mut run = Run::new(spec(ChallengeKind::Survival, reach_the_end())).unwrap();
        run.enter();
        assert_eq!(run.observe(1, &[SessionEvent::Damaged { frame: 1 }]), Next::Continue);
    }

    #[test]
    #[should_panic(expected = "call enter first")]
    fn observing_before_entering_is_a_caller_bug() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end())).unwrap();
        run.observe(1, &[]);
    }

    #[test]
    #[should_panic(expected = "call enter first")]
    fn a_reentry_must_be_acknowledged_before_observing_again() {
        let mut run = Run::new(spec(ChallengeKind::TimeTrial, reach_the_end())).unwrap();
        run.enter();
        assert_eq!(run.observe(1, &[died(1)]), Next::Reenter);
        run.observe(2, &[]);
    }
}
