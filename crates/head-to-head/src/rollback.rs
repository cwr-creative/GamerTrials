//! Rollback over a shared machine, on `ggrs` (ADR 0036).
//!
//! The session asks for three things, in order, every frame: save the
//! machine, restore it to an earlier frame, advance it with every slot's
//! input. [`Rollback::pump`] answers each with the
//! [`SharedRunner`](session_api::SharedRunner) and keeps two things the
//! engine does not:
//!
//! * **The facts, held until confirmed.** What a frame reported goes into a
//!   buffer under its frame number. A restore throws away every buffered
//!   frame after the one restored — they were guessed — and the frames are
//!   handed out only once `ggrs` says every slot's input for them is final.
//!   A judge fed from [`Confirmed`] never sees a fact that was rolled back.
//! * **Which advance is a replay.** After a restore every advance up to the
//!   last one in the batch runs a frame the player already saw and heard;
//!   the machine is told so and makes no picture or sound for it. The last
//!   advance is the new live frame.
//!
//! A checksum over the machine's [`digest`](session_api::SharedRunner::digest)
//! goes with every save, and `ggrs` compares them between the machines every
//! few frames: a mismatch is [`State::Desynced`], and the bout is over.
//!
//! [`Rollback::synctest`] runs the same driver with no network: `ggrs`
//! restores and replays a few frames on every advance and checks that the
//! checksums come out the same, which is how a core is proved to roll back
//! exactly before anyone plays on it.

use std::collections::BTreeMap;
use std::fmt;
use std::time::Duration;

use ggrs::{
    Config, DesyncDetection, GgrsError, GgrsEvent, GgrsRequest, Message, NonBlockingSocket,
    P2PSession, PlayerType, PredictRepeatLast, SessionBuilder, SessionState, SyncTestSession,
};
use serde::{Deserialize, Serialize};
use session_api::{RunnerError, SharedRunner, Slot, SlotEvent, SlotInput, Snapshot};

use crate::link::Link;

/// A slot's pad on the wire: the bits of [`SlotInput`], as they are.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, Serialize, Deserialize)]
pub struct Pad(pub u32);

/// The session's types, in `ggrs`'s terms.
pub struct Game;

impl Config for Game {
    type Input = Pad;
    type InputPredictor = PredictRepeatLast;
    type State = Snapshot;
    type Address = Slot;
}

/// A [`Link`] as `ggrs` wants it: its messages as JSON, to a slot.
struct Socket<L: Link>(L);

impl<L: Link> NonBlockingSocket<Slot> for Socket<L> {
    fn send_to(&mut self, msg: &Message, addr: &Slot) {
        if let Ok(bytes) = serde_json::to_vec(msg) {
            self.0.send(*addr, &bytes, false);
        }
    }

    fn receive_all_messages(&mut self) -> Vec<(Slot, Message)> {
        self.0
            .receive()
            .into_iter()
            .filter_map(|(slot, bytes)| serde_json::from_slice(&bytes).ok().map(|m| (slot, m)))
            .collect()
    }
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Settings {
    /// Frames the local input is delayed, so the remote's has time to
    /// arrive and fewer frames are guessed. Felt on one's own buttons.
    pub input_delay: usize,
    /// The machine's rate, for the engine's clock.
    pub fps: usize,
    /// How far ahead of the slowest slot the engine will run on guesses
    /// before it stalls. Zero is lockstep.
    pub prediction_window: usize,
    /// Every this many frames the machines compare checksums.
    pub check_every: u32,
    pub disconnect_timeout: Duration,
    pub disconnect_notify_delay: Duration,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            input_delay: 2,
            fps: 60,
            prediction_window: 8,
            check_every: 30,
            disconnect_timeout: Duration::from_secs(5),
            disconnect_notify_delay: Duration::from_secs(1),
        }
    }
}

/// Where the session stands.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum State {
    /// Finding the other slots. Nothing advances yet.
    Synchronizing,
    Live,
    /// This slot stopped hearing from that one, for longer than the
    /// timeout. The bout is over on this side.
    Lost(Slot),
    /// The machines disagreed on `frame`. The bout is over.
    Desynced { frame: u64 },
}

/// A frame every slot's input is final for, and what it reported.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Confirmed {
    pub frame: u64,
    pub events: Vec<SlotEvent>,
}

/// Numbers for a log line or the ADR: how the link and the engine did.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Stats {
    pub frame: u64,
    pub confirmed: u64,
    /// Restores so far.
    pub rollbacks: u64,
    /// Frames run again after a restore, so far.
    pub replayed: u64,
    /// Round trip to each remote slot, in milliseconds, as the engine
    /// measures it; zero until it has measured.
    pub ping_ms: u64,
    /// Frames this slot runs ahead of the others, as the engine estimates.
    pub frames_ahead: i32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Error(pub String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<GgrsError> for Error {
    fn from(e: GgrsError) -> Self {
        Error(e.to_string())
    }
}

impl From<RunnerError> for Error {
    fn from(e: RunnerError) -> Self {
        Error(e.0)
    }
}

enum Session {
    P2p(Box<P2PSession<Game>>),
    SyncTest(Box<SyncTestSession<Game>>),
}

pub struct Rollback<M: SharedRunner> {
    machine: M,
    session: Session,
    slots: u8,
    state: State,
    /// What each frame reported, by frame, until confirmed.
    pending: BTreeMap<u64, Vec<SlotEvent>>,
    /// Frames to sit out, on the engine's recommendation, so the others
    /// catch up.
    skip: u32,
    /// The newest frame handed out. A sync test replays frames already
    /// handed out; they are not handed out twice.
    released: u64,
    rollbacks: u64,
    replayed: u64,
}

impl<M: SharedRunner> Rollback<M> {
    /// A session with the other slots over `link`, playing `local`. The
    /// machine is at the stage's entry and its frame is 0; so is the
    /// session's. Nothing advances until every slot is found.
    pub fn connect<L: Link>(
        machine: M,
        link: L,
        local: Slot,
        slots: u8,
        settings: Settings,
    ) -> Result<Rollback<M>, Error> {
        if slots < 2 || local.0 >= slots {
            return Err(Error(format!("{local} of {slots} slots")));
        }
        let mut b = SessionBuilder::<Game>::new()
            .with_num_players(usize::from(slots))?
            .with_fps(settings.fps.max(1))?
            .with_input_delay(settings.input_delay)
            .with_max_prediction_window(settings.prediction_window)
            .with_desync_detection_mode(DesyncDetection::On { interval: settings.check_every.max(1) })
            .with_disconnect_timeout(settings.disconnect_timeout)
            .with_disconnect_notify_delay(settings.disconnect_notify_delay);
        for i in 0..slots {
            let kind = if i == local.0 { PlayerType::Local } else { PlayerType::Remote(Slot(i)) };
            b = b.add_player(kind, usize::from(i))?;
        }
        let session = b.start_p2p_session(Socket(link))?;
        Ok(Rollback {
            machine,
            session: Session::P2p(Box::new(session)),
            slots,
            state: State::Synchronizing,
            pending: BTreeMap::new(),
            skip: 0,
            released: 0,
            rollbacks: 0,
            replayed: 0,
        })
    }

    /// No network: every slot is local, and the engine restores and replays
    /// `check_distance` frames on every advance, comparing checksums. What
    /// a core is proved on before anyone plays it.
    pub fn synctest(machine: M, slots: u8, check_distance: usize, settings: Settings) -> Result<Rollback<M>, Error> {
        if slots < 1 {
            return Err(Error("a sync test needs a slot".into()));
        }
        let mut b = SessionBuilder::<Game>::new()
            .with_num_players(usize::from(slots))?
            .with_fps(settings.fps.max(1))?
            .with_input_delay(settings.input_delay)
            .with_max_prediction_window(settings.prediction_window.max(check_distance + 1))
            .with_check_distance(check_distance);
        for i in 0..slots {
            b = b.add_player(PlayerType::Local, usize::from(i))?;
        }
        let session = b.start_synctest_session()?;
        Ok(Rollback {
            machine,
            session: Session::SyncTest(Box::new(session)),
            slots,
            state: State::Live,
            pending: BTreeMap::new(),
            skip: 0,
            released: 0,
            rollbacks: 0,
            replayed: 0,
        })
    }

    pub fn machine(&self) -> &M {
        &self.machine
    }

    pub fn machine_mut(&mut self) -> &mut M {
        &mut self.machine
    }

    pub fn into_machine(self) -> M {
        self.machine
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn slots(&self) -> u8 {
        self.slots
    }

    /// The machine's frame: the newest, guessed or not.
    pub fn frame(&self) -> u64 {
        self.machine.frame()
    }

    /// The newest frame every slot's input is final for.
    pub fn confirmed_frame(&self) -> u64 {
        match &self.session {
            Session::P2p(s) => s.confirmed_frame().max(0) as u64,
            Session::SyncTest(s) => s.current_frame().max(0) as u64,
        }
    }

    pub fn stats(&self) -> Stats {
        let (ping_ms, frames_ahead) = match &self.session {
            Session::P2p(s) => {
                let ping = s
                    .remote_player_handles()
                    .into_iter()
                    .filter_map(|h| s.network_stats(h).ok())
                    .map(|n| n.ping as u64)
                    .max()
                    .unwrap_or(0);
                (ping, s.frames_ahead())
            }
            Session::SyncTest(_) => (0, 0),
        };
        Stats {
            frame: self.frame(),
            confirmed: self.confirmed_frame(),
            rollbacks: self.rollbacks,
            replayed: self.replayed,
            ping_ms,
            frames_ahead,
        }
    }

    /// Hear from the link and the engine without advancing. Call this every
    /// few milliseconds from the moment the session exists until the bout
    /// goes, and [`pump`](Rollback::pump) does it too: the engine's own
    /// keep-alives only leave on a poll, and a peer silent for the timeout
    /// is dropped.
    pub fn poll(&mut self) -> &State {
        let Session::P2p(s) = &mut self.session else { return &self.state };
        s.poll_remote_clients();
        let events: Vec<GgrsEvent<Game>> = s.events().collect();
        for e in events {
            match e {
                GgrsEvent::Synchronizing { .. } | GgrsEvent::Synchronized { .. } => {}
                GgrsEvent::NetworkInterrupted { .. } | GgrsEvent::NetworkResumed { .. } => {}
                GgrsEvent::WaitRecommendation { skip_frames } => self.skip = self.skip.max(skip_frames),
                GgrsEvent::Disconnected { addr } => {
                    if matches!(self.state, State::Synchronizing | State::Live) {
                        self.state = State::Lost(addr);
                    }
                }
                GgrsEvent::DesyncDetected { frame, .. } => {
                    if matches!(self.state, State::Synchronizing | State::Live) {
                        self.state = State::Desynced { frame: frame.max(0) as u64 };
                    }
                }
            }
        }
        if self.state == State::Synchronizing && s.current_state() == SessionState::Running {
            self.state = State::Live;
        }
        &self.state
    }

    /// One frame: the local slots' inputs in (one for a session over a
    /// link, every slot for a sync test), the engine's requests answered,
    /// and every frame newly confirmed out, oldest first. Empty while
    /// synchronizing, on a frame the engine says to sit out, or once the
    /// session is lost or desynced; the state says which.
    pub fn pump(&mut self, inputs: &[(Slot, SlotInput)]) -> Result<Vec<Confirmed>, Error> {
        self.poll();
        if !matches!(self.state, State::Live) {
            return Ok(Vec::new());
        }
        if self.skip > 0 {
            self.skip -= 1;
            return Ok(Vec::new());
        }
        for (slot, input) in inputs {
            let r = match &mut self.session {
                Session::P2p(s) => s.add_local_input(usize::from(slot.0), Pad(input.0)),
                Session::SyncTest(s) => s.add_local_input(usize::from(slot.0), Pad(input.0)),
            };
            r?;
        }
        let requests = match &mut self.session {
            Session::P2p(s) => s.advance_frame(),
            Session::SyncTest(s) => s.advance_frame(),
        };
        let requests = match requests {
            Ok(r) => r,
            // Too far ahead of a slot whose input has not come: sit this
            // frame out; the input is read again next frame.
            Err(GgrsError::PredictionThreshold) => return Ok(Vec::new()),
            Err(GgrsError::MismatchedChecksum { current_frame, .. }) => {
                self.state = State::Desynced { frame: current_frame.max(0) as u64 };
                return Ok(Vec::new());
            }
            Err(e) => return Err(e.into()),
        };
        self.handle(requests)?;
        Ok(self.release())
    }

    fn handle(&mut self, requests: Vec<GgrsRequest<Game>>) -> Result<(), Error> {
        let last_advance = requests
            .iter()
            .rposition(|r| matches!(r, GgrsRequest::AdvanceFrame { .. }))
            .unwrap_or(usize::MAX);
        for (i, request) in requests.into_iter().enumerate() {
            match request {
                GgrsRequest::SaveGameState { cell, frame } => {
                    let snapshot = self.machine.snapshot()?;
                    let checksum = fnv1a(&self.machine.digest());
                    cell.save(frame, Some(snapshot), Some(checksum));
                }
                GgrsRequest::LoadGameState { cell, frame } => {
                    let snapshot = cell.load().ok_or_else(|| Error(format!("no snapshot for frame {frame}")))?;
                    self.machine.restore(&snapshot)?;
                    let frame = frame.max(0) as u64;
                    if self.machine.frame() != frame {
                        return Err(Error(format!(
                            "the machine restored to frame {} where frame {frame} was asked",
                            self.machine.frame()
                        )));
                    }
                    // Everything after it was guessed.
                    let _guessed = self.pending.split_off(&(frame + 1));
                    self.rollbacks += 1;
                }
                GgrsRequest::AdvanceFrame { inputs } => {
                    for (slot, (pad, _status)) in inputs.iter().enumerate() {
                        self.machine.set_input(Slot(slot as u8), SlotInput(pad.0));
                    }
                    let replaying = i != last_advance;
                    if replaying {
                        self.replayed += 1;
                    }
                    let events = self.machine.advance_slots(replaying)?.to_vec();
                    self.pending.insert(self.machine.frame(), events);
                }
            }
        }
        Ok(())
    }

    /// Hands out every pending frame up to the confirmed one, once each.
    fn release(&mut self) -> Vec<Confirmed> {
        let confirmed = self.confirmed_frame();
        let later = self.pending.split_off(&(confirmed + 1));
        let done = std::mem::replace(&mut self.pending, later);
        let out: Vec<Confirmed> = done
            .into_iter()
            .filter(|(frame, _)| *frame > self.released)
            .map(|(frame, events)| Confirmed { frame, events })
            .collect();
        if let Some(last) = out.last() {
            self.released = last.frame;
        }
        out
    }
}

/// FNV-1a over the digest, widened: what two machines compare.
fn fnv1a(bytes: &[u8]) -> u128 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    u128::from(h)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Channels;
    use session_api::{AssetKey, EntryPoint, GameId, SessionEvent, StageId, StageRunner, StageSpec};
    use session_fake::FakeMachine;

    fn stage() -> StageSpec {
        StageSpec {
            game: GameId("sf2".into()),
            stage: StageId("vs".into()),
            content: AssetKey("fake:sf2".into()),
            entry: EntryPoint::Named("vs".into()),
        }
    }

    fn machine() -> FakeMachine {
        let mut m = FakeMachine::new(stage(), 2);
        m.enter(&stage()).unwrap();
        m
    }

    /// A scripted pad for `slot` on `frame`: wins a round now and then,
    /// differently per slot, so guesses go wrong.
    fn script(slot: Slot, frame: u64) -> SlotInput {
        let f = frame + u64::from(slot.0) * 7;
        SlotInput(u32::from(f % 5 == 0))
    }

    fn fast() -> Settings {
        Settings {
            input_delay: 0,
            check_every: 5,
            disconnect_timeout: Duration::from_millis(300),
            disconnect_notify_delay: Duration::from_millis(100),
            ..Default::default()
        }
    }

    /// The facts a plain machine reports for the same script.
    fn plain(frames: u64) -> Vec<(u64, Vec<SlotEvent>)> {
        let mut m = machine();
        (1..=frames)
            .map(|f| {
                for s in 0..2 {
                    m.set_input(Slot(s), script(Slot(s), f));
                }
                (f, m.advance_slots(false).unwrap().to_vec())
            })
            .collect()
    }

    #[test]
    fn a_sync_test_rolls_back_every_frame_and_the_facts_come_out_as_played_straight() {
        let mut r = Rollback::synctest(machine(), 2, 3, fast()).unwrap();
        let mut got = Vec::new();
        for f in 1..=60 {
            let inputs = [(Slot(0), script(Slot(0), f)), (Slot(1), script(Slot(1), f))];
            got.extend(r.pump(&inputs).unwrap().into_iter().map(|c| (c.frame, c.events)));
        }
        assert_eq!(r.state(), &State::Live);
        assert_eq!(got, plain(60));
        assert!(r.stats().rollbacks > 50, "{:?}", r.stats());
        assert!(r.machine().replayed() > 0);
    }

    /// Two sessions over a slow, jittery link confirm the same facts on the
    /// same frames, and both machines end in the same state.
    #[test]
    fn two_sessions_over_a_jittery_link_agree_frame_for_frame() {
        let mut ends = Channels::mesh(2, 2, 2, 0, 11);
        let b = ends.pop().unwrap();
        let a = ends.pop().unwrap();
        let mut ra = Rollback::connect(machine(), a, Slot(0), 2, fast()).unwrap();
        let mut rb = Rollback::connect(machine(), b, Slot(1), 2, fast()).unwrap();
        let (mut ca, mut cb) = (Vec::new(), Vec::new());
        let mut fa = 0;
        let mut fb = 0;
        for _ in 0..2000 {
            if ra.state() == &State::Live {
                fa += 1;
            }
            if rb.state() == &State::Live {
                fb += 1;
            }
            ca.extend(ra.pump(&[(Slot(0), script(Slot(0), fa))]).unwrap());
            cb.extend(rb.pump(&[(Slot(1), script(Slot(1), fb))]).unwrap());
            if ca.len() >= 100 && cb.len() >= 100 {
                break;
            }
        }
        assert_eq!(ra.state(), &State::Live, "{:?}", ra.stats());
        assert_eq!(rb.state(), &State::Live, "{:?}", rb.stats());
        let n = ca.len().min(cb.len());
        assert!(n >= 100, "a {} b {}", ca.len(), cb.len());
        assert_eq!(ca[..n], cb[..n]);
        assert_eq!(ca[0].frame, 1);
        assert!(ca.windows(2).all(|w| w[1].frame == w[0].frame + 1), "in order, no gaps");
        assert!(ra.stats().rollbacks > 0 || rb.stats().rollbacks > 0, "{:?} {:?}", ra.stats(), rb.stats());
        // The script wins rounds; the facts say so, tagged.
        assert!(ca.iter().any(|c| c.events.iter().any(|e| e.slot == Some(Slot(1)))));
    }

    #[test]
    fn a_machine_that_diverges_is_caught() {
        let (a, b) = Channels::pair();
        let mut ra = Rollback::connect(machine(), a, Slot(0), 2, fast()).unwrap();
        let mut rb = Rollback::connect(FakeMachine::new(stage(), 2).diverge_from(20).entered(), b, Slot(1), 2, fast()).unwrap();
        for f in 1..=400 {
            ra.pump(&[(Slot(0), script(Slot(0), f))]).unwrap();
            rb.pump(&[(Slot(1), script(Slot(1), f))]).unwrap();
            if matches!(ra.state(), State::Desynced { .. }) || matches!(rb.state(), State::Desynced { .. }) {
                return;
            }
        }
        panic!("no desync reported: {:?} {:?}", ra.state(), rb.state());
    }

    #[test]
    fn a_dead_link_is_reported_as_lost() {
        let (a, b) = Channels::pair();
        let cutter = b.cutter();
        let mut ra = Rollback::connect(machine(), a, Slot(0), 2, fast()).unwrap();
        let mut rb = Rollback::connect(machine(), b, Slot(1), 2, fast()).unwrap();
        for f in 1..=30 {
            ra.pump(&[(Slot(0), script(Slot(0), f))]).unwrap();
            rb.pump(&[(Slot(1), script(Slot(1), f))]).unwrap();
        }
        assert_eq!(ra.state(), &State::Live);
        cutter.cut();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            ra.pump(&[(Slot(0), SlotInput(0))]).unwrap();
            if let State::Lost(slot) = ra.state() {
                assert_eq!(*slot, Slot(1));
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("never lost: {:?}", ra.state());
    }

    #[test]
    fn a_frame_sat_out_is_not_a_frame() {
        let mut r = Rollback::synctest(machine(), 1, 1, fast()).unwrap();
        r.skip = 2;
        assert!(r.pump(&[(Slot(0), SlotInput(1))]).unwrap().is_empty());
        assert!(r.pump(&[(Slot(0), SlotInput(1))]).unwrap().is_empty());
        let c = r.pump(&[(Slot(0), SlotInput(1))]).unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].frame, 1);
        assert!(matches!(c[0].events[0].event, SessionEvent::Counted { frame: 1, .. }));
    }
}
