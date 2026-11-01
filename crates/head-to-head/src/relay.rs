//! A machine that brings its own network, relayed (ADR 0036).
//!
//! A [`Networked`] machine keeps itself in step with the other slots'
//! machines — a DOS game's netcode over an emulated IPX driver. All this
//! crate does for it is carry the bytes: what arrived on the link is
//! delivered before the frame, what the machine wants sent goes out after
//! it. The game decides what a late packet costs; nothing is rolled back,
//! and every fact the machine reports is final the frame it is reported.

use session_api::{Networked, RunnerError, SessionEvent, Slot};

use crate::link::Link;

pub struct Relay<M: Networked, L: Link> {
    machine: M,
    link: L,
    local: Slot,
    slots: u8,
}

impl<M: Networked, L: Link> Relay<M, L> {
    /// Joins the machine to the session as `local` of `slots`, with every
    /// other slot taken to be present: a bout goes only once every seat
    /// is prepared.
    pub fn join(mut machine: M, link: L, local: Slot, slots: u8) -> Result<Relay<M, L>, RunnerError> {
        machine.join(local, slots)?;
        for i in 0..slots {
            if i != local.0 {
                machine.peer_joined(Slot(i));
            }
        }
        Ok(Relay { machine, link, local, slots })
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

    /// One frame: deliver what arrived, advance, send what the machine
    /// wants sent. The frame's facts, final.
    pub fn pump(&mut self) -> Result<Vec<SessionEvent>, RunnerError> {
        for (from, bytes) in self.link.receive() {
            self.machine.deliver(from, &bytes);
        }
        let events = self.machine.advance()?.to_vec();
        for p in self.machine.outgoing() {
            match p.to {
                Some(to) => self.link.send(to, &p.bytes, p.reliable),
                None => {
                    for i in 0..self.slots {
                        if i != self.local.0 {
                            self.link.send(Slot(i), &p.bytes, p.reliable);
                        }
                    }
                }
            }
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::link::Channels;
    use session_api::{AssetKey, EntryPoint, GameId, StageId, StageRunner, StageSpec};
    use session_fake::FakeMachine;

    fn stage() -> StageSpec {
        StageSpec {
            game: GameId("doom".into()),
            stage: StageId("dm".into()),
            content: AssetKey("fake:doom".into()),
            entry: EntryPoint::Named("dm".into()),
        }
    }

    fn machine() -> FakeMachine {
        let mut m = FakeMachine::new(stage(), 2);
        m.enter(&stage()).unwrap();
        m
    }

    #[test]
    fn packets_cross_both_ways_and_are_counted_as_facts() {
        let (a, b) = Channels::pair();
        let mut ra = Relay::join(machine(), a, Slot(0), 2).unwrap();
        let mut rb = Relay::join(machine(), b, Slot(1), 2).unwrap();
        assert!(ra.machine().peers().contains(&Slot(1)));
        ra.machine_mut().send(None, b"tic 1", true);
        rb.machine_mut().send(Some(Slot(0)), b"tic 1 back", false);
        assert!(ra.pump().unwrap().is_empty(), "nothing has arrived yet");
        let eb = rb.pump().unwrap();
        assert_eq!(eb.len(), 1, "{eb:?}");
        assert_eq!(rb.machine().received(), [(Slot(0), b"tic 1".to_vec())]);
        let ea = ra.pump().unwrap();
        assert_eq!(ea.len(), 1, "{ea:?}");
        assert_eq!(ra.machine().received(), [(Slot(1), b"tic 1 back".to_vec())]);
        assert_eq!(ra.machine().frame(), 2);
    }
}
