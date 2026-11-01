//! A shared machine that plays its inputs instead of a game.
//!
//! Where [`FakeRunner`](crate::FakeRunner) replays a script, a
//! [`FakeMachine`] is a tiny deterministic game: each frame, each slot's pad
//! advances that slot's own tally, and the machine says so. Two machines fed
//! the same inputs produce the same facts, the same [`Snapshot`]s and the
//! same [`digest`](SharedRunner::digest) — which is all a rollback session or
//! a judge needs of a game, and what every head-to-head test runs on. A
//! machine can also be told to *diverge* from a frame on, which is what a
//! desync looks like.
//!
//! The pad's bits, as this machine reads them:
//!
//! | bit | the slot... |
//! |---|---|
//! | 0 | wins a round: `Counted { counter: "rounds_won", count: 1 }`, tagged with the slot |
//! | 1 | dies: `Died`, tagged with the slot |
//! | 2 | leaves the stage: `StageExited { reason: Quit }`, everyone's |
//!
//! As a [`Networked`] machine it keeps no state of its own in step with
//! anyone; it hands out what it was asked to send and counts what it was
//! handed, `Counted { counter: "packets" }`, so a relay can be seen to carry
//! bytes both ways.

use std::collections::BTreeSet;

use session_api::{
    ExitReason, Networked, Packet, Presentable, RunnerError, SessionEvent,
    SharedRunner, Slot, SlotEvent, SlotInput, Snapshot, StageRunner, StageSpec, VideoFrame,
};

#[derive(Debug)]
pub struct FakeMachine {
    stage: StageSpec,
    slots: u8,
    entered: bool,
    frame: u64,
    inputs: Vec<SlotInput>,
    tallies: Vec<u32>,
    /// What the last advance reported, tagged.
    events: Vec<SlotEvent>,
    /// The same, untagged, for the single-player contract.
    plain: Vec<SessionEvent>,
    /// From this frame on, the machine's state is wrong: its digest and its
    /// facts differ from an honest machine's.
    diverge_from: Option<u64>,
    /// A picture per frame that was not a replay.
    seq: u64,
    replayed: u64,
    // Networked.
    slot: Option<Slot>,
    peers: BTreeSet<Slot>,
    inbox: Vec<(Slot, Vec<u8>)>,
    outbox: Vec<Packet>,
    packets: u32,
}

impl FakeMachine {
    pub fn new(stage: StageSpec, slots: u8) -> FakeMachine {
        let n = usize::from(slots.max(1));
        FakeMachine {
            stage,
            slots: slots.max(1),
            entered: false,
            frame: 0,
            inputs: vec![SlotInput::default(); n],
            tallies: vec![0; n],
            events: Vec::new(),
            plain: Vec::new(),
            diverge_from: None,
            seq: 0,
            replayed: 0,
            slot: None,
            peers: BTreeSet::new(),
            inbox: Vec::new(),
            outbox: Vec::new(),
            packets: 0,
        }
    }

    pub fn stage(&self) -> &StageSpec {
        &self.stage
    }

    /// From `frame` on, this machine is wrong, the way a desynced core is.
    pub fn diverge_from(mut self, frame: u64) -> FakeMachine {
        self.diverge_from = Some(frame);
        self
    }

    /// Each slot's tally of rounds won, in slot order.
    pub fn tallies(&self) -> &[u32] {
        &self.tallies
    }

    /// How many frames were run as replays after a restore.
    pub fn replayed(&self) -> u64 {
        self.replayed
    }

    /// Queue bytes for another slot, or everyone with `None`.
    pub fn send(&mut self, to: Option<Slot>, bytes: &[u8], reliable: bool) {
        self.outbox.push(Packet { to, bytes: bytes.to_vec(), reliable });
    }

    /// Everything delivered so far, oldest first.
    pub fn received(&self) -> &[(Slot, Vec<u8>)] {
        &self.inbox
    }

    pub fn peers(&self) -> &BTreeSet<Slot> {
        &self.peers
    }

    fn diverged(&self) -> bool {
        self.diverge_from.is_some_and(|f| self.frame >= f)
    }

    fn step(&mut self, replaying: bool) -> Result<(), RunnerError> {
        if !self.entered {
            return Err(RunnerError(format!("{} was advanced before it was entered", self.stage.stage)));
        }
        self.frame += 1;
        self.events.clear();
        let frame = self.frame;
        let mut exited = false;
        for i in 0..self.inputs.len() {
            let bits = self.inputs[i].0;
            let slot = Slot(i as u8);
            if bits & 1 != 0 {
                self.tallies[i] += 1;
                // A diverged machine counts the first slot's rounds twice.
                if self.diverged() && i == 0 {
                    self.tallies[i] += 1;
                }
                self.events.push(SlotEvent {
                    slot: Some(slot),
                    event: SessionEvent::Counted { frame, counter: "rounds_won".into(), count: 1 },
                });
            }
            if bits & 2 != 0 {
                self.events.push(SlotEvent { slot: Some(slot), event: SessionEvent::Died { frame } });
            }
            if bits & 4 != 0 {
                exited = true;
            }
        }
        if exited {
            self.events.push(SlotEvent {
                slot: None,
                event: SessionEvent::StageExited { frame, reason: ExitReason::Quit },
            });
        }
        if replaying {
            self.replayed += 1;
        } else {
            self.seq += 1;
        }
        self.plain = self.events.iter().map(|e| e.event.clone()).collect();
        Ok(())
    }
}

impl StageRunner for FakeMachine {
    fn enter(&mut self, spec: &StageSpec) -> Result<(), RunnerError> {
        if spec != &self.stage {
            return Err(RunnerError(format!(
                "this machine holds {} and was entered at {}",
                self.stage.stage, spec.stage
            )));
        }
        self.entered = true;
        self.frame = 0;
        self.tallies.iter_mut().for_each(|t| *t = 0);
        self.inputs.iter_mut().for_each(|i| *i = SlotInput::default());
        self.events.clear();
        self.plain.clear();
        Ok(())
    }

    fn advance(&mut self) -> Result<&[SessionEvent], RunnerError> {
        self.step(false)?;
        Ok(&self.plain)
    }

    fn frame(&self) -> u64 {
        self.frame
    }
}

impl Presentable for FakeMachine {
    fn present(&self, since: u64, f: &mut dyn FnMut(VideoFrame<'_>)) -> u64 {
        if self.seq > since {
            let pixel = [0u32];
            f(VideoFrame { pixels: &pixel, width: 1, height: 1, seq: self.seq });
        }
        self.seq
    }
}

impl SharedRunner for FakeMachine {
    fn slots(&self) -> u8 {
        self.slots
    }

    fn set_input(&mut self, slot: Slot, input: SlotInput) {
        if let Some(i) = self.inputs.get_mut(usize::from(slot.0)) {
            *i = input;
        }
    }

    fn advance_slots(&mut self, replaying: bool) -> Result<&[SlotEvent], RunnerError> {
        self.step(replaying)?;
        Ok(&self.events)
    }

    fn snapshot(&mut self) -> Result<Snapshot, RunnerError> {
        let mut bytes = self.frame.to_le_bytes().to_vec();
        for t in &self.tallies {
            bytes.extend_from_slice(&t.to_le_bytes());
        }
        Ok(Snapshot(bytes))
    }

    fn restore(&mut self, snapshot: &Snapshot) -> Result<(), RunnerError> {
        let b = &snapshot.0;
        let want = 8 + 4 * self.tallies.len();
        if b.len() != want {
            return Err(RunnerError(format!("snapshot is {} bytes, this machine's are {want}", b.len())));
        }
        self.frame = u64::from_le_bytes(b[..8].try_into().expect("8 bytes"));
        for (i, t) in self.tallies.iter_mut().enumerate() {
            let at = 8 + 4 * i;
            *t = u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"));
        }
        self.events.clear();
        self.plain.clear();
        Ok(())
    }

    fn digest(&self) -> Vec<u8> {
        let mut bytes = self.frame.to_le_bytes().to_vec();
        for t in &self.tallies {
            bytes.extend_from_slice(&t.to_le_bytes());
        }
        bytes
    }
}

impl Networked for FakeMachine {
    fn join(&mut self, slot: Slot, slots: u8) -> Result<(), RunnerError> {
        if slot.0 >= slots {
            return Err(RunnerError(format!("{slot} of {slots}")));
        }
        self.slot = Some(slot);
        Ok(())
    }

    fn deliver(&mut self, from: Slot, bytes: &[u8]) {
        self.inbox.push((from, bytes.to_vec()));
        self.packets += 1;
        let event = SessionEvent::Counted { frame: self.frame, counter: "packets".into(), count: 1 };
        self.plain.push(event.clone());
        self.events.push(SlotEvent { slot: self.slot, event });
    }

    fn outgoing(&mut self) -> Vec<Packet> {
        std::mem::take(&mut self.outbox)
    }

    fn peer_joined(&mut self, slot: Slot) {
        self.peers.insert(slot);
    }

    fn peer_left(&mut self, slot: Slot) {
        self.peers.remove(&slot);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_api::{AssetKey, EntryPoint, GameId, StageId};

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

    #[test]
    fn each_slots_pad_advances_its_own_tally_and_the_facts_say_whose() {
        let mut m = machine();
        m.set_input(Slot(1), SlotInput(1));
        let events = m.advance_slots(false).unwrap().to_vec();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].slot, Some(Slot(1)));
        assert!(matches!(events[0].event, SessionEvent::Counted { frame: 1, .. }));
        assert_eq!(m.tallies(), [0, 1]);
    }

    #[test]
    fn two_machines_on_the_same_inputs_agree_and_a_restore_rewinds() {
        let (mut a, mut b) = (machine(), machine());
        for f in 0..10u32 {
            for m in [&mut a, &mut b] {
                m.set_input(Slot(0), SlotInput(f % 2));
                m.set_input(Slot(1), SlotInput((f % 3 == 0) as u32));
                m.advance_slots(false).unwrap();
            }
        }
        assert_eq!(a.digest(), b.digest());
        let at_ten = a.snapshot().unwrap();
        a.set_input(Slot(0), SlotInput(1));
        a.advance_slots(true).unwrap();
        assert_ne!(a.digest(), b.digest());
        a.restore(&at_ten).unwrap();
        assert_eq!(a.digest(), b.digest());
        assert_eq!((a.frame(), a.replayed()), (10, 1));
    }

    #[test]
    fn a_diverged_machine_differs_from_an_honest_one() {
        let (mut a, mut b) = (machine(), FakeMachine::new(stage(), 2).diverge_from(3));
        b.enter(&stage()).unwrap();
        for _ in 0..5 {
            for m in [&mut a, &mut b] {
                m.set_input(Slot(0), SlotInput(1));
                m.advance_slots(false).unwrap();
            }
        }
        assert_ne!(a.digest(), b.digest());
    }

    #[test]
    fn a_replayed_frame_makes_no_picture() {
        let mut m = machine();
        m.advance_slots(false).unwrap();
        m.advance_slots(true).unwrap();
        let mut shown = 0;
        let seq = m.present(0, &mut |_| shown += 1);
        assert_eq!((seq, shown), (1, 1));
    }

    #[test]
    fn packets_are_handed_out_and_counted_in() {
        let mut m = machine();
        m.join(Slot(0), 2).unwrap();
        m.send(Some(Slot(1)), b"hi", true);
        let out = m.outgoing();
        assert_eq!(out.len(), 1);
        assert!(m.outgoing().is_empty());
        m.deliver(Slot(1), b"yo");
        assert_eq!(m.received(), [(Slot(1), b"yo".to_vec())]);
        assert!(matches!(m.events[0].event, SessionEvent::Counted { ref counter, .. } if counter == "packets"));
    }
}
