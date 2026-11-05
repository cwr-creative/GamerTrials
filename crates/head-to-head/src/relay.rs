//! A machine that brings its own network, relayed (ADR 0036).
//!
//! A [`Networked`] machine keeps itself in step with the other slots'
//! machines — a DOS game's netcode over an emulated IPX driver. All this
//! crate does for it is carry the bytes: what arrived on the link is
//! delivered before the frame, what the machine wants sent goes out after
//! it. The game decides what a late packet costs; nothing is rolled back,
//! and every fact the machine reports is final the frame it is reported.
//!
//! The link is best-effort, and a core may ask for a packet to arrive, and
//! in order (`reliable`). So the relay frames what it sends:
//!
//! | first byte | the rest |
//! |---|---|
//! | `0` | a packet the core said may be dropped |
//! | `1` | a sequence number (u32 LE) and a packet the core said must arrive |
//! | `2` | a sequence number: that reliable packet arrived |
//! | `3` | a hello, before the bout: is anyone there? |
//! | `4` | the answer |
//!
//! Reliable packets are resent until acknowledged and delivered in sequence
//! per peer; a packet arriving twice is dropped by its number. The hello
//! is how a machine knows to start running: the other one answered, so the
//! game's own netcode will find it, and the game starts when it does.

use std::collections::{BTreeMap, HashMap};
use std::time::{Duration, Instant};

use session_api::{Networked, RunnerError, SessionEvent, Slot};

use crate::link::Link;

const UNRELIABLE: u8 = 0;
const RELIABLE: u8 = 1;
const ACK: u8 = 2;
const HELLO: u8 = 3;
const HELLO_BACK: u8 = 4;

/// How often an unacknowledged packet is sent again.
const RESEND: Duration = Duration::from_millis(50);
/// How often a hello goes out while a peer has not answered.
const HELLO_EVERY: Duration = Duration::from_millis(200);

/// Reliable delivery to and from one peer.
#[derive(Default)]
struct Reliable {
    next_out: u32,
    /// Sent and not yet acknowledged, with when last sent.
    unacked: BTreeMap<u32, (Vec<u8>, Instant)>,
    next_in: u32,
    /// Arrived ahead of their turn.
    held: BTreeMap<u32, Vec<u8>>,
}

/// Where the relay stands.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RelayState {
    /// Waiting for every other slot to answer a hello.
    Connecting,
    /// Every slot answered; the machine may join.
    Connected,
    /// The machine has joined the session and is being relayed.
    Live,
}

pub struct Relay<M: Networked, L: Link> {
    machine: M,
    link: L,
    local: Slot,
    slots: u8,
    state: RelayState,
    answered: HashMap<Slot, bool>,
    last_hello: Option<Instant>,
    reliable: HashMap<Slot, Reliable>,
    /// The machine has already joined its session: a DOS core's network
    /// driver must be up before the game boots and looks for it.
    joined: bool,
}

impl<M: Networked, L: Link> Relay<M, L> {
    /// A relay for `local` of `slots`, over `link`, not yet joined: it
    /// says hello to every other slot until each answers.
    pub fn connect(machine: M, link: L, local: Slot, slots: u8) -> Relay<M, L> {
        let answered = (0..slots).filter(|s| *s != local.0).map(|s| (Slot(s), false)).collect();
        Relay {
            machine,
            link,
            local,
            slots,
            state: RelayState::Connecting,
            answered,
            last_hello: None,
            reliable: HashMap::new(),
            joined: false,
        }
    }

    /// [`connect`](Relay::connect) for a machine the caller has already
    /// joined to its session, with every other slot present, before
    /// booting the game: [`join`](Relay::join) then only goes live.
    pub fn connect_joined(machine: M, link: L, local: Slot, slots: u8) -> Relay<M, L> {
        let mut r = Relay::connect(machine, link, local, slots);
        r.joined = true;
        r
    }

    /// [`connect`](Relay::connect) and [`join`](Relay::join) at once, for a
    /// link known to be up.
    pub fn join_now(machine: M, link: L, local: Slot, slots: u8) -> Result<Relay<M, L>, RunnerError> {
        let mut r = Relay::connect(machine, link, local, slots);
        for a in r.answered.values_mut() {
            *a = true;
        }
        r.state = RelayState::Connected;
        r.join()?;
        Ok(r)
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

    pub fn state(&self) -> &RelayState {
        &self.state
    }

    /// Hear from the link without advancing: hellos out and answered, and,
    /// once live, the core's packets delivered. Call it every few
    /// milliseconds until the machine is running.
    pub fn poll(&mut self) -> &RelayState {
        if self.state == RelayState::Connecting
            && self.last_hello.map_or(true, |t| t.elapsed() >= HELLO_EVERY)
        {
            for (slot, answered) in &self.answered {
                if !answered {
                    self.link.send(*slot, &[HELLO], false);
                }
            }
            self.last_hello = Some(Instant::now());
        }
        self.receive();
        self.resend();
        if self.state == RelayState::Connecting && self.answered.values().all(|a| *a) {
            self.state = RelayState::Connected;
        }
        &self.state
    }

    /// Joins the machine to the session as `local` of `slots`, with every
    /// other slot present, and from here [`pump`](Relay::pump) carries its
    /// packets. Done before the bout goes: the game has to start for the bout
    /// to be prepared.
    pub fn join(&mut self) -> Result<(), RunnerError> {
        if self.state == RelayState::Connecting {
            return Err(RunnerError("the other slots have not answered yet".into()));
        }
        if self.state == RelayState::Live {
            return Ok(());
        }
        if self.joined {
            // What a machine joined before its boot said on the way up, it
            // said to nobody: there was no link yet. It is stale, and a burst
            // of it can cost a game's setup the packet it was waiting for
            // (docs/games/doom.md), so it is never sent.
            let _ = self.machine.outgoing();
        } else {
            self.machine.join(self.local, self.slots)?;
            for i in 0..self.slots {
                if i != self.local.0 {
                    self.machine.peer_joined(Slot(i));
                }
            }
            self.joined = true;
        }
        self.state = RelayState::Live;
        // The other machine may have gone live first. What it said must
        // arrive has been waiting for this.
        let peers: Vec<Slot> = self.reliable.keys().copied().collect();
        for from in peers {
            self.deliver_reliable(from);
        }
        Ok(())
    }

    /// One frame: deliver what arrived, advance, send what the machine
    /// wants sent. The frame's facts, final.
    pub fn pump(&mut self) -> Result<Vec<SessionEvent>, RunnerError> {
        self.receive();
        let events = self.machine.advance()?.to_vec();
        for p in self.machine.outgoing() {
            match p.to {
                Some(to) => self.send(to, &p.bytes, p.reliable),
                None => {
                    for i in 0..self.slots {
                        if i != self.local.0 {
                            self.send(Slot(i), &p.bytes, p.reliable);
                        }
                    }
                }
            }
        }
        self.resend();
        Ok(events)
    }

    fn send(&mut self, to: Slot, bytes: &[u8], reliable: bool) {
        if !reliable {
            let mut frame = Vec::with_capacity(bytes.len() + 1);
            frame.push(UNRELIABLE);
            frame.extend_from_slice(bytes);
            self.link.send(to, &frame, false);
            return;
        }
        let r = self.reliable.entry(to).or_default();
        let seq = r.next_out;
        r.next_out = r.next_out.wrapping_add(1);
        let mut frame = Vec::with_capacity(bytes.len() + 5);
        frame.push(RELIABLE);
        frame.extend_from_slice(&seq.to_le_bytes());
        frame.extend_from_slice(bytes);
        r.unacked.insert(seq, (frame.clone(), Instant::now()));
        self.link.send(to, &frame, true);
    }

    fn resend(&mut self) {
        let now = Instant::now();
        let mut again: Vec<(Slot, Vec<u8>)> = Vec::new();
        for (slot, r) in &mut self.reliable {
            for (frame, sent) in r.unacked.values_mut() {
                if now.duration_since(*sent) >= RESEND {
                    *sent = now;
                    again.push((*slot, frame.clone()));
                }
            }
        }
        for (slot, frame) in again {
            self.link.send(slot, &frame, true);
        }
    }

    /// Takes everything on the link: hellos and acks are the relay's, the
    /// rest is the machine's, reliable packets in their order.
    fn receive(&mut self) {
        for (from, frame) in self.link.receive() {
            let Some((&kind, rest)) = frame.split_first() else { continue };
            match kind {
                HELLO => self.link.send(from, &[HELLO_BACK], false),
                HELLO_BACK => {
                    if let Some(a) = self.answered.get_mut(&from) {
                        *a = true;
                    }
                }
                UNRELIABLE => {
                    if self.state == RelayState::Live {
                        self.machine.deliver(from, rest);
                    }
                }
                RELIABLE => {
                    if rest.len() < 4 {
                        continue;
                    }
                    let seq = u32::from_le_bytes(rest[..4].try_into().expect("4 bytes"));
                    let data = &rest[4..];
                    let mut ack = vec![ACK];
                    ack.extend_from_slice(&seq.to_le_bytes());
                    self.link.send(from, &ack, false);
                    let r = self.reliable.entry(from).or_default();
                    if seq.wrapping_sub(r.next_in) < u32::MAX / 2 {
                        r.held.entry(seq).or_insert_with(|| data.to_vec());
                    }
                    if self.state == RelayState::Live {
                        self.deliver_reliable(from);
                    }
                }
                ACK => {
                    if rest.len() < 4 {
                        continue;
                    }
                    let seq = u32::from_le_bytes(rest[..4].try_into().expect("4 bytes"));
                    if let Some(r) = self.reliable.get_mut(&from) {
                        r.unacked.remove(&seq);
                    }
                }
                _ => {}
            }
        }
    }

    /// Everything from `from` that is now in sequence.
    fn deliver_reliable(&mut self, from: Slot) {
        let Some(r) = self.reliable.get_mut(&from) else { return };
        let mut due = Vec::new();
        while let Some(d) = r.held.remove(&r.next_in) {
            due.push(d);
            r.next_in = r.next_in.wrapping_add(1);
        }
        for d in due {
            self.machine.deliver(from, &d);
        }
    }

    /// Reliable packets sent and not yet acknowledged, over every peer.
    pub fn in_flight(&self) -> usize {
        self.reliable.values().map(|r| r.unacked.len()).sum()
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
        let mut ra = Relay::join_now(machine(), a, Slot(0), 2).unwrap();
        let mut rb = Relay::join_now(machine(), b, Slot(1), 2).unwrap();
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
        ra.pump().unwrap();
        assert_eq!(ra.in_flight(), 0, "the reliable packet was acknowledged");
    }

    #[test]
    fn the_hello_connects_before_the_bout_goes() {
        let (a, b) = Channels::pair();
        let mut ra = Relay::connect(machine(), a, Slot(0), 2);
        let mut rb = Relay::connect(machine(), b, Slot(1), 2);
        assert!(ra.join().is_err(), "not before the other answers");
        for _ in 0..4 {
            ra.poll();
            rb.poll();
        }
        assert_eq!(ra.state(), &RelayState::Connected);
        assert_eq!(rb.state(), &RelayState::Connected);
        ra.join().unwrap();
        rb.join().unwrap();
        assert_eq!(ra.state(), &RelayState::Live);
    }

    /// The machines go live a round trip apart, and now run before the bout
    /// goes (ADR 0039). What the first said must arrive waits for the second,
    /// acknowledged, and arrives once, in order.
    #[test]
    fn a_reliable_packet_waits_for_a_machine_not_yet_live() {
        let (a, b) = Channels::pair();
        let mut ra = Relay::connect(machine(), a, Slot(0), 2);
        let mut rb = Relay::connect(machine(), b, Slot(1), 2);
        for _ in 0..4 {
            ra.poll();
            rb.poll();
        }
        ra.join().unwrap();
        for i in 0..3u8 {
            ra.machine_mut().send(Some(Slot(1)), &[i], true);
            ra.pump().unwrap();
            rb.poll();
        }
        assert!(rb.machine().received().is_empty(), "not live yet");
        ra.poll();
        assert_eq!(ra.in_flight(), 0, "held, and acknowledged");
        rb.join().unwrap();
        let got: Vec<u8> = rb.machine().received().iter().map(|(_, d)| d[0]).collect();
        assert_eq!(got, [0, 1, 2]);
    }

    /// A machine joined before its boot talks to nobody until the link is up.
    /// That goes unsent; what it says once live arrives.
    #[test]
    fn what_a_machine_said_before_going_live_is_never_sent() {
        let (a, b) = Channels::pair();
        let mut joined = machine();
        joined.join(Slot(0), 2).unwrap();
        joined.send(None, b"said while booting", true);
        joined.send(Some(Slot(1)), b"and this", false);
        let mut ra = Relay::connect_joined(joined, a, Slot(0), 2);
        let mut rb = Relay::connect(machine(), b, Slot(1), 2);
        for _ in 0..4 {
            ra.poll();
            rb.poll();
        }
        ra.join().unwrap();
        rb.join().unwrap();
        ra.machine_mut().send(None, b"said live", true);
        ra.pump().unwrap();
        rb.pump().unwrap();
        assert_eq!(rb.machine().received(), [(Slot(0), b"said live".to_vec())]);
    }

    /// A quarter of everything is lost: every reliable packet still arrives,
    /// once, in order; the unreliable ones mostly do not matter.
    #[test]
    fn reliable_packets_arrive_in_order_over_a_lossy_link() {
        let mut ends = Channels::mesh(2, 0, 0, 250, 5);
        let b = ends.pop().unwrap();
        let a = ends.pop().unwrap();
        let mut ra = Relay::join_now(machine(), a, Slot(0), 2).unwrap();
        let mut rb = Relay::join_now(machine(), b, Slot(1), 2).unwrap();
        for i in 0..50u32 {
            ra.machine_mut().send(Some(Slot(1)), &i.to_le_bytes(), true);
            ra.pump().unwrap();
            rb.pump().unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while (ra.in_flight() > 0 || rb.machine().received().len() < 50) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            ra.pump().unwrap();
            rb.pump().unwrap();
        }
        let got: Vec<u32> =
            rb.machine().received().iter().map(|(_, d)| u32::from_le_bytes(d[..4].try_into().unwrap())).collect();
        assert_eq!(got, (0..50).collect::<Vec<u32>>());
        assert_eq!(ra.in_flight(), 0);
    }
}
