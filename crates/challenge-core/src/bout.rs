//! One head-to-head round: a shared stage, one [`Run`] per slot, and the
//! rules that bind them (ADR 0036).
//!
//! A [`Bout`] is fed each slot's facts as that slot reports them and answers
//! with the verdicts it can settle. Each run is judged exactly as a run is,
//! and the bout adds what a run cannot see on its own:
//!
//! * **The first slot to complete ends the bout.** Every other slot still
//!   live on that frame is [`AttemptEnd::Beaten`] on it. Two completing on
//!   one frame is a draw: both stand.
//! * **Frame order, not arrival order.** Each slot's facts arrive on their
//!   own schedule — on a server, over two sockets — so a completion at frame
//!   F is *held* until every other live slot has reported a frame at or past
//!   F. A slot's report of frame F says every earlier fact of its own is in,
//!   and nothing else does. The bout therefore wants to hear from every slot
//!   now and then, events or not; a driver reports an empty frame regularly.
//! * **Whoever stays wins.** A slot that gives up, or goes out on its own
//!   (leaves the stage, trips a fact the challenge says ends it), leaves the
//!   others to play on; when one is left, it completes on the frame the last
//!   of the others went out — once it has reported past that frame, since it
//!   might yet have gone out itself before then.
//! * **The limit is everyone's.** A slot that timed out hands nobody a win:
//!   every other slot reaches the same limit on the same shared machine.
//! * **A link that died is lost by both.** [`Bout::lost`] says a slot's
//!   reports have ended; [`Bout::abandon`] forfeits everyone still live and
//!   settles the rest on what they had. Nobody wins a bout the network ended;
//!   what to do between the two calls — wait for the other side's give-up, or
//!   its own loss report — is the driver's, since only it has a clock.
//!
//! Pure, like everything here: tested with literal events in both arrival
//! orders.

use session_api::{ChallengeSpec, SessionEvent, Slot};

use crate::run::{AttemptEnd, Ending, Next, Run, RunError, RunResult};

#[derive(Debug)]
struct Seat {
    run: Run,
    /// The highest frame this slot has reported, events or not.
    reported: u64,
    /// This slot's reports have ended (its link died); nothing further will
    /// come, so holds waiting on it are released.
    stopped: bool,
    /// What the slot's own run came to, before the bout's rules.
    own: Option<RunResult>,
    /// The bout's verdict, once settled. Final.
    verdict: Option<RunResult>,
}

#[derive(Debug)]
pub struct Bout {
    seats: Vec<Seat>,
    /// Set by [`Bout::abandon`]: the bout ended by the network, so the
    /// whoever-stays rule is off.
    abandoned: bool,
}

impl Bout {
    /// One run per slot of `spec`, each bounded by `limit` when given.
    pub fn new(spec: ChallengeSpec, limit: Option<u64>) -> Result<Bout, RunError> {
        let slots = spec.slots.max(1);
        let mut seats = Vec::with_capacity(usize::from(slots));
        for _ in 0..slots {
            let run = Run::new(spec.clone())?;
            let run = match limit {
                Some(l) => run.with_limit(l),
                None => run,
            };
            seats.push(Seat { run, reported: 0, stopped: false, own: None, verdict: None });
        }
        Ok(Bout { seats, abandoned: false })
    }

    pub fn slots(&self) -> u8 {
        self.seats.len() as u8
    }

    /// Every slot is at the stage's start and the frame counter is 0.
    pub fn enter(&mut self) {
        for s in &mut self.seats {
            s.run.enter();
        }
    }

    /// The run behind `slot`, for its clock and attempt count.
    pub fn run(&self, slot: Slot) -> &Run {
        &self.seat(slot).run
    }

    /// The settled verdict for `slot`, if the bout has one yet.
    pub fn verdict(&self, slot: Slot) -> Option<&RunResult> {
        self.seat(slot).verdict.as_ref()
    }

    /// Every slot's verdict, in slot order, once all are settled.
    pub fn results(&self) -> Option<Vec<(Slot, RunResult)>> {
        self.seats
            .iter()
            .enumerate()
            .map(|(i, s)| s.verdict.clone().map(|v| (Slot(i as u8), v)))
            .collect()
    }

    pub fn is_over(&self) -> bool {
        self.seats.iter().all(|s| s.verdict.is_some())
    }

    /// `slot` reported `frame`, with the facts on it that are its own or
    /// everyone's. Answers with every verdict this settled, this slot's or
    /// another's. Ignored for a slot already settled or whose reports have
    /// ended.
    pub fn observe(&mut self, slot: Slot, frame: u64, events: &[SessionEvent]) -> Vec<(Slot, RunResult)> {
        let seat = self.seat_mut(slot);
        if seat.verdict.is_some() || seat.stopped {
            return Vec::new();
        }
        seat.reported = seat.reported.max(frame);
        if seat.own.is_none() {
            match seat.run.observe(frame, events) {
                Next::Continue => {}
                Next::Reenter => unreachable!("a head-to-head run never re-enters"),
                Next::Finished(r) => seat.own = Some(r),
            }
        }
        self.settle()
    }

    /// `slot` gave up on `frame`. Whoever stays plays on.
    pub fn forfeit(&mut self, slot: Slot, frame: u64) -> Vec<(Slot, RunResult)> {
        let seat = self.seat_mut(slot);
        if seat.verdict.is_some() {
            return Vec::new();
        }
        seat.reported = seat.reported.max(frame);
        if seat.own.is_none() {
            // The frames up to the give-up count; the limit may have beaten it.
            let r = match seat.run.observe(frame, &[]) {
                Next::Finished(r) => r,
                _ => seat.run.forfeit(),
            };
            seat.own = Some(r);
        }
        self.settle()
    }

    /// `slot`'s reports ended on `frame`: its link to the others died. The
    /// slot stays live until [`Bout::abandon`], or until another slot's
    /// give-up hands it the win.
    pub fn lost(&mut self, slot: Slot, frame: u64) -> Vec<(Slot, RunResult)> {
        let seat = self.seat_mut(slot);
        if seat.verdict.is_some() {
            return Vec::new();
        }
        seat.reported = seat.reported.max(frame);
        seat.stopped = true;
        self.settle()
    }

    /// The bout ended by the network: every slot still live is forfeited on
    /// the last frame it reported, and nobody wins.
    pub fn abandon(&mut self) -> Vec<(Slot, RunResult)> {
        self.abandoned = true;
        for seat in &mut self.seats {
            if seat.verdict.is_none() && seat.own.is_none() {
                seat.own = Some(seat.run.forfeit());
            }
        }
        self.settle()
    }

    fn seat(&self, slot: Slot) -> &Seat {
        self.seats.get(usize::from(slot.0)).unwrap_or_else(|| panic!("{slot} is not in this bout"))
    }

    fn seat_mut(&mut self, slot: Slot) -> &mut Seat {
        self.seats.get_mut(usize::from(slot.0)).unwrap_or_else(|| panic!("{slot} is not in this bout"))
    }

    /// Applies the bout's rules to what every slot has reported so far and
    /// returns the verdicts newly settled.
    fn settle(&mut self) -> Vec<(Slot, RunResult)> {
        let before: Vec<bool> = self.seats.iter().map(|s| s.verdict.is_some()).collect();
        if !self.is_over() {
            self.settle_completion();
        }
        if !self.is_over() {
            self.settle_last_standing();
        }
        if !self.is_over() {
            self.settle_everyone_out();
        }
        self.seats
            .iter()
            .enumerate()
            .filter(|(i, s)| !before[*i] && s.verdict.is_some())
            .map(|(i, s)| (Slot(i as u8), s.verdict.clone().expect("filtered")))
            .collect()
    }

    /// A slot has reported at or past `frame`, or never will.
    fn heard_past(seat: &Seat, frame: u64) -> bool {
        seat.stopped || seat.reported >= frame
    }

    fn settle_completion(&mut self) {
        let unsettled = |s: &Seat| s.verdict.is_none();
        // The earliest completion among the unsettled.
        let Some(first) = self
            .seats
            .iter()
            .filter(|s| unsettled(s))
            .filter_map(|s| s.own.as_ref())
            .filter(|r| r.completed())
            .map(|r| r.frames)
            .min()
        else {
            return;
        };
        // Held until every other unsettled slot has reported past it: one of
        // them may yet report a completion earlier still.
        let all_heard = self
            .seats
            .iter()
            .filter(|s| unsettled(s))
            .filter(|s| !s.own.as_ref().is_some_and(|r| r.completed() && r.frames == first))
            .all(|s| Bout::heard_past(s, first));
        if !all_heard {
            return;
        }
        for seat in self.seats.iter_mut().filter(|s| s.verdict.is_none()) {
            seat.verdict = Some(match &seat.own {
                Some(r) if r.completed() && r.frames == first => r.clone(),
                // Out before the winner finished: what it had stands.
                Some(r) if r.frames < first => r.clone(),
                // Live on the winning frame: the run ends beaten on it.
                None => seat.run.beaten(first),
                // Its own run ended after the winning frame — completed later,
                // or out later — which the bout overrules: beaten on it.
                Some(r) => RunResult {
                    challenge: r.challenge.clone(),
                    kind: r.kind,
                    ending: Ending::Failed(AttemptEnd::Beaten),
                    frames: first,
                    attempts: r.attempts,
                },
            });
        }
    }

    fn settle_last_standing(&mut self) {
        if self.abandoned {
            return;
        }
        let unsettled: Vec<usize> =
            (0..self.seats.len()).filter(|&i| self.seats[i].verdict.is_none()).collect();
        let live: Vec<usize> = unsettled.iter().copied().filter(|&i| self.seats[i].own.is_none()).collect();
        let out: Vec<usize> = unsettled.iter().copied().filter(|&i| self.seats[i].own.is_some()).collect();
        if live.len() != 1 || out.is_empty() {
            return;
        }
        // Everyone else is out. A time-out hands nobody a win: the one left
        // reaches the same limit, and will say so itself.
        if out.iter().any(|&i| self.seats[i].own.as_ref().unwrap().ending == Ending::TimedOut) {
            return;
        }
        let last_out = out.iter().map(|&i| self.seats[i].own.as_ref().unwrap().frames).max().unwrap();
        let survivor = live[0];
        if !Bout::heard_past(&self.seats[survivor], last_out) {
            return;
        }
        for &i in &out {
            self.seats[i].verdict = self.seats[i].own.clone();
        }
        let run = &mut self.seats[survivor].run;
        // The run itself only knows how to end as beaten or by its own
        // criteria; completing by default is the bout's reading, so the
        // result is written here with the run's figures.
        let frames = last_out.max(run.frames().min(last_out));
        self.seats[survivor].verdict = Some(RunResult {
            challenge: run.spec().id.clone(),
            kind: run.spec().kind,
            ending: Ending::Completed,
            frames,
            attempts: run.attempts(),
        });
    }

    fn settle_everyone_out(&mut self) {
        let unsettled: Vec<usize> =
            (0..self.seats.len()).filter(|&i| self.seats[i].verdict.is_none()).collect();
        if unsettled.iter().all(|&i| self.seats[i].own.is_some()) {
            for i in unsettled {
                self.seats[i].verdict = self.seats[i].own.clone();
            }
        }
    }
}

impl AttemptEnd {
    /// Whether this is the bout's own ending, as opposed to the run's.
    pub fn is_beaten(&self) -> bool {
        matches!(self, AttemptEnd::Beaten)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_api::{ChallengeId, ChallengeKind, Completion, ExitReason, GameId, StageId};
    use std::collections::BTreeMap;

    const LIMIT: u64 = 1000;
    const A: Slot = Slot(0);
    const B: Slot = Slot(1);

    fn fight() -> ChallengeSpec {
        ChallengeSpec {
            id: ChallengeId("fight".into()),
            name: "Fight".into(),
            game: GameId("sf2".into()),
            stage: StageId("vs".into()),
            kind: ChallengeKind::HeadToHead,
            complete_when: Completion {
                counts: BTreeMap::from([("rounds_won".to_string(), 2)]),
                ..Default::default()
            },
            ends_attempt_on: Default::default(),
            slots: 2,
        }
    }

    fn bout() -> Bout {
        let mut b = Bout::new(fight(), Some(LIMIT)).unwrap();
        b.enter();
        b
    }

    fn won(frame: u64) -> SessionEvent {
        SessionEvent::Counted { frame, counter: "rounds_won".into(), count: 1 }
    }

    fn ending(v: &[(Slot, RunResult)], slot: Slot) -> Option<(Ending, u64)> {
        v.iter().find(|(s, _)| *s == slot).map(|(_, r)| (r.ending.clone(), r.frames))
    }

    #[test]
    fn the_first_to_complete_wins_and_the_other_is_beaten_on_that_frame() {
        let mut b = bout();
        assert!(b.observe(A, 100, &[won(100)]).is_empty());
        assert!(b.observe(B, 150, &[won(150)]).is_empty());
        // A wins at 300. Held: B has only reported 150.
        let v = b.observe(A, 300, &[won(300)]);
        assert!(v.is_empty(), "held until B reports past 300: {v:?}");
        assert!(b.observe(B, 299, &[]).is_empty());
        let v = b.observe(B, 300, &[]);
        assert_eq!(ending(&v, A), Some((Ending::Completed, 300)));
        assert_eq!(ending(&v, B), Some((Ending::Failed(AttemptEnd::Beaten), 300)));
        assert!(b.is_over());
    }

    /// B's completion at 280 arrives after A's at 300: B wins, A is beaten
    /// at 280, whichever socket was faster.
    #[test]
    fn the_earlier_completion_wins_whichever_arrives_first() {
        let mut b = bout();
        b.observe(A, 100, &[won(100)]);
        b.observe(B, 100, &[won(100)]);
        assert!(b.observe(A, 300, &[won(300)]).is_empty());
        let v = b.observe(B, 280, &[won(280)]);
        assert_eq!(ending(&v, B), Some((Ending::Completed, 280)));
        assert_eq!(ending(&v, A), Some((Ending::Failed(AttemptEnd::Beaten), 280)));

        let mut b = bout();
        b.observe(A, 100, &[won(100)]);
        b.observe(B, 100, &[won(100)]);
        assert!(b.observe(B, 280, &[won(280)]).is_empty());
        let v = b.observe(A, 300, &[won(300)]);
        assert_eq!(ending(&v, B), Some((Ending::Completed, 280)));
        assert_eq!(ending(&v, A), Some((Ending::Failed(AttemptEnd::Beaten), 280)));
    }

    #[test]
    fn completing_on_the_same_frame_is_a_draw() {
        let mut b = bout();
        b.observe(A, 100, &[won(100)]);
        b.observe(B, 100, &[won(100)]);
        assert!(b.observe(A, 300, &[won(300)]).is_empty());
        let v = b.observe(B, 300, &[won(300)]);
        assert_eq!(ending(&v, A), Some((Ending::Completed, 300)));
        assert_eq!(ending(&v, B), Some((Ending::Completed, 300)));
    }

    #[test]
    fn both_at_the_limit_is_a_draw_of_failures() {
        let mut b = bout();
        let v = b.observe(A, LIMIT, &[]);
        assert!(v.is_empty(), "B has not reached the limit yet, as far as anyone knows");
        let v = b.observe(B, LIMIT, &[]);
        assert_eq!(ending(&v, A), Some((Ending::TimedOut, LIMIT)));
        assert_eq!(ending(&v, B), Some((Ending::TimedOut, LIMIT)));
    }

    #[test]
    fn a_give_up_hands_the_win_once_the_other_has_reported_past_it() {
        let mut b = bout();
        b.observe(A, 100, &[won(100)]);
        assert!(b.forfeit(B, 120).is_empty(), "A was last heard at 100");
        let v = b.observe(A, 130, &[]);
        assert_eq!(ending(&v, B), Some((Ending::Forfeited, 120)));
        assert_eq!(ending(&v, A), Some((Ending::Completed, 120)));
    }

    /// A left the fight at 90, B gave up at 120: A was already out, so
    /// nobody is handed anything.
    #[test]
    fn a_slot_out_before_the_other_gave_up_keeps_what_it_had() {
        let mut b = bout();
        assert!(b.forfeit(B, 120).is_empty());
        let left = SessionEvent::StageExited { frame: 90, reason: ExitReason::Quit };
        let v = b.observe(A, 90, &[left]);
        assert_eq!(ending(&v, A), Some((Ending::Failed(AttemptEnd::Exited(ExitReason::Quit)), 90)));
        assert_eq!(ending(&v, B), Some((Ending::Forfeited, 120)));
    }

    #[test]
    fn a_death_does_not_end_a_run_in_a_bout() {
        let mut b = bout();
        assert!(b.observe(A, 50, &[SessionEvent::Died { frame: 50 }]).is_empty());
        assert!(b.verdict(A).is_none());
        assert!(!b.is_over());
    }

    #[test]
    fn a_lost_link_is_lost_by_both_once_abandoned() {
        let mut b = bout();
        b.observe(A, 100, &[won(100)]);
        b.observe(B, 110, &[]);
        assert!(b.lost(A, 100).is_empty());
        assert!(b.lost(B, 110).is_empty(), "both live, nobody out: nothing to settle");
        let v = b.abandon();
        assert_eq!(ending(&v, A), Some((Ending::Forfeited, 100)));
        assert_eq!(ending(&v, B), Some((Ending::Forfeited, 110)));
    }

    /// A's link died at 100 and A reported so; B then gave up at 130. A
    /// stays, and A's reports being over counts as having heard past 130.
    #[test]
    fn a_give_up_after_the_other_lost_the_link_still_hands_the_win() {
        let mut b = bout();
        assert!(b.lost(A, 100).is_empty());
        let v = b.forfeit(B, 130);
        assert_eq!(ending(&v, B), Some((Ending::Forfeited, 130)));
        assert_eq!(ending(&v, A), Some((Ending::Completed, 130)));
    }

    #[test]
    fn a_completion_is_not_held_for_a_slot_that_stopped_reporting() {
        let mut b = bout();
        b.observe(B, 50, &[]);
        b.lost(B, 50);
        let v = b.observe(A, 300, &[won(300), won(300)]);
        assert_eq!(ending(&v, A), Some((Ending::Completed, 300)));
        assert_eq!(ending(&v, B), Some((Ending::Failed(AttemptEnd::Beaten), 300)));
    }

    #[test]
    #[should_panic(expected = "not in this bout")]
    fn a_slot_the_bout_does_not_seat_is_a_caller_bug() {
        let mut b = bout();
        b.observe(Slot(2), 1, &[]);
    }
}
