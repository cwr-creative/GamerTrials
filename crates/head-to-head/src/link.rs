//! Bytes between slots.
//!
//! A [`Link`] is what a bout's machines talk over: send bytes to a slot,
//! take what arrived. It is addressed by [`Slot`], never by anything a
//! network knows, so the same session runs over [`Udp`] between two players'
//! machines and over [`Channels`] inside one test.
//!
//! Delivery is best-effort, like UDP: a message may be lost or reordered,
//! and a session on top of it expects that. `reliable` is a request a link
//! may honour later; nothing here does yet.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

use session_api::Slot;

/// Bytes to and from the other slots of a bout.
pub trait Link: 'static {
    /// Send `bytes` to `to`. Never blocks; a link that cannot send drops.
    fn send(&mut self, to: Slot, bytes: &[u8], reliable: bool);

    /// Everything that arrived since the last call, with the slot each came
    /// from.
    fn receive(&mut self) -> Vec<(Slot, Vec<u8>)>;
}

// ----------------------------------------------------------------- channels

/// Bytes and the slot they came from.
type Packet = (Slot, Vec<u8>);

/// A link inside one process, for tests: each end delivers what the other
/// sent after a number of polls, with a jitter and a loss drawn from a seed,
/// so a rollback can be made to happen on purpose and reproduced.
pub struct Channels {
    me: Slot,
    polls: u64,
    delay: u64,
    jitter: u64,
    /// Out of 1000.
    loss: u64,
    rng: u64,
    rx: Receiver<Packet>,
    txs: HashMap<Slot, Sender<Packet>>,
    /// Arrived and waiting for its poll.
    held: BTreeMap<u64, Vec<Packet>>,
    cut: Arc<AtomicBool>,
}

impl Channels {
    /// One end per slot, all connected, delivering after `delay` polls plus
    /// up to `jitter` more, losing `loss` in a thousand.
    pub fn mesh(slots: u8, delay: u64, jitter: u64, loss: u64, seed: u64) -> Vec<Channels> {
        let ends: Vec<(Sender<Packet>, Receiver<Packet>)> = (0..slots).map(|_| mpsc::channel()).collect();
        let txs: Vec<Sender<Packet>> = ends.iter().map(|(tx, _)| tx.clone()).collect();
        let cut = Arc::new(AtomicBool::new(false));
        ends.into_iter()
            .enumerate()
            .map(|(i, (_, rx))| Channels {
                me: Slot(i as u8),
                polls: 0,
                delay,
                jitter,
                loss,
                rng: seed ^ (i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                rx,
                txs: txs.iter().enumerate().map(|(j, tx)| (Slot(j as u8), tx.clone())).collect(),
                held: BTreeMap::new(),
                cut: Arc::clone(&cut),
            })
            .collect()
    }

    /// A pair with no delay, jitter or loss.
    pub fn pair() -> (Channels, Channels) {
        let mut v = Channels::mesh(2, 0, 0, 0, 1);
        let b = v.pop().unwrap();
        (v.pop().unwrap(), b)
    }

    /// Nothing sent from now on arrives anywhere, as if the link died.
    pub fn cut(&self) {
        self.cut.store(true, Ordering::Relaxed);
    }

    /// A handle that can [`cut`](Channels::cut) the mesh after the ends
    /// have been handed to sessions.
    pub fn cutter(&self) -> Cutter {
        Cutter(Arc::clone(&self.cut))
    }

    fn next_random(&mut self) -> u64 {
        self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.rng;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

/// Cuts a [`Channels`] mesh from outside it.
pub struct Cutter(Arc<AtomicBool>);

impl Cutter {
    pub fn cut(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

impl Link for Channels {
    fn send(&mut self, to: Slot, bytes: &[u8], _reliable: bool) {
        if self.cut.load(Ordering::Relaxed) {
            return;
        }
        if let Some(tx) = self.txs.get(&to) {
            let _ = tx.send((self.me, bytes.to_vec()));
        }
    }

    fn receive(&mut self) -> Vec<(Slot, Vec<u8>)> {
        self.polls += 1;
        while let Ok(m) = self.rx.try_recv() {
            let lost = self.loss > 0 && self.next_random() % 1000 < self.loss;
            if lost {
                continue;
            }
            let extra = if self.jitter > 0 { self.next_random() % (self.jitter + 1) } else { 0 };
            let due = self.polls + self.delay + extra;
            self.held.entry(due).or_default().push(m);
        }
        let later = self.held.split_off(&(self.polls + 1));
        let due = std::mem::replace(&mut self.held, later);
        due.into_values().flatten().collect()
    }
}

// ---------------------------------------------------------------------- udp

/// The link between two players' machines (ADR 0036): one UDP socket, and
/// for each other slot the addresses it might be at. Until a slot answers,
/// everything for it goes to every candidate; the first address a packet
/// comes back from is the one, and the rest are forgotten. A packet from a
/// candidate's IP on another port is taken as that slot's too, when only
/// one slot still waiting has that IP: a carrier's NAT gives each
/// destination its own port, so the port the server saw is not the one the
/// packets arrive from. That is what
/// makes two clients on one machine, two on a LAN and two across the
/// internet connect without a STUN service: the server hands over the
/// address it saw the player's probes come from and the one the player's
/// own machine reports, and whichever works, works.
pub struct Udp {
    socket: UdpSocket,
    candidates: HashMap<Slot, Vec<SocketAddr>>,
    locked: HashMap<Slot, SocketAddr>,
    buf: Vec<u8>,
    /// This socket's own port: a candidate on loopback at it is this socket,
    /// as when a peer on another machine listed its own loopback address on
    /// the same fixed port.
    port: u16,
}

impl Udp {
    /// Bound and non-blocking. `0.0.0.0:0` takes any port; the server
    /// learns which from the probes sent through it.
    pub fn bind(addr: SocketAddr) -> io::Result<Udp> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        let port = socket.local_addr()?.port();
        Ok(Udp { socket, candidates: HashMap::new(), locked: HashMap::new(), buf: vec![0; 65536], port })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Where `slot` might be. Replaces what was known; a slot already
    /// answering keeps its address. A candidate that is this socket itself
    /// is left out.
    pub fn peer(&mut self, slot: Slot, candidates: Vec<SocketAddr>) {
        let candidates = candidates.into_iter().filter(|c| !self.is_me(*c)).collect();
        self.candidates.insert(slot, candidates);
    }

    /// Loopback at this socket's own port: nobody else on this machine can
    /// hold it.
    fn is_me(&self, addr: SocketAddr) -> bool {
        addr.ip().is_loopback() && addr.port() == self.port
    }

    /// The address `slot` answered from, once it has.
    pub fn locked(&self, slot: Slot) -> Option<SocketAddr> {
        self.locked.get(&slot).copied()
    }

    /// A probe to the server's UDP side: eight bytes of token, so the server
    /// sees where this socket reaches the world from.
    pub fn probe(&self, server: SocketAddr, token: u64) -> io::Result<()> {
        self.socket.send_to(&token.to_le_bytes(), server).map(|_| ())
    }

    fn slot_of(&mut self, from: SocketAddr) -> Option<Slot> {
        if let Some((slot, _)) = self.locked.iter().find(|(_, a)| **a == from) {
            return Some(*slot);
        }
        let exact = self.candidates.iter().find(|(_, c)| c.contains(&from)).map(|(s, _)| *s);
        let slot = match exact {
            Some(slot) => slot,
            None => {
                // The same machine on a port nobody listed: a NAT that maps
                // each destination to its own port. Taken only when exactly
                // one slot not yet answering could be it.
                let mut waiting = self
                    .candidates
                    .iter()
                    .filter(|(s, c)| !self.locked.contains_key(s) && c.iter().any(|a| a.ip() == from.ip()))
                    .map(|(s, _)| *s);
                let slot = waiting.next()?;
                if waiting.next().is_some() {
                    return None;
                }
                slot
            }
        };
        self.locked.insert(slot, from);
        Some(slot)
    }
}

/// Where a peer might be, with one more guess: a peer the server saw only
/// at private addresses reached it from the server's own network, so from
/// outside that network it is at the server's public address — the one this
/// machine dialled — on the same port, if the router there keeps the port or
/// forwards it. Unchanged when the server is itself private (everyone on one
/// LAN) or the peer has a public address already.
pub fn reach(candidates: Vec<SocketAddr>, server: Option<SocketAddr>) -> Vec<SocketAddr> {
    let Some(server) = server.filter(|s| is_public(s.ip())) else { return candidates };
    if candidates.is_empty() || candidates.iter().any(|c| is_public(c.ip())) {
        return candidates;
    }
    let mut out = candidates.clone();
    for c in &candidates {
        let guess = SocketAddr::new(server.ip(), c.port());
        if !out.contains(&guess) {
            out.push(guess);
        }
    }
    out
}

/// An address the internet routes: not loopback, not a private or
/// link-local range, not a carrier's shared NAT space (100.64.0.0/10).
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || (o[0] == 100 && (o[1] & 0xC0) == 64))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments()[0];
            !(v6.is_loopback() || v6.is_unspecified() || (s & 0xFE00) == 0xFC00 || (s & 0xFFC0) == 0xFE80)
        }
    }
}

impl Link for Udp {
    fn send(&mut self, to: Slot, bytes: &[u8], _reliable: bool) {
        match self.locked.get(&to) {
            Some(addr) => {
                let _ = self.socket.send_to(bytes, addr);
            }
            None => {
                for addr in self.candidates.get(&to).map(Vec::as_slice).unwrap_or(&[]) {
                    let _ = self.socket.send_to(bytes, addr);
                }
            }
        }
    }

    fn receive(&mut self) -> Vec<(Slot, Vec<u8>)> {
        let mut out = Vec::new();
        loop {
            match self.socket.recv_from(&mut self.buf) {
                Ok((n, from)) => {
                    if self.is_me(from) {
                        continue;
                    }
                    if let Some(slot) = self.slot_of(from) {
                        out.push((slot, self.buf[..n].to_vec()));
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                // A candidate with nothing listening answers with a reset
                // on Windows; the socket is fine and the other candidates
                // may still answer.
                Err(e) if e.kind() == io::ErrorKind::ConnectionReset => continue,
                Err(_) => break,
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channels_deliver_after_the_delay_in_both_directions() {
        let mut ends = Channels::mesh(2, 2, 0, 0, 7);
        let (mut b, mut a) = (ends.pop().unwrap(), ends.pop().unwrap());
        a.send(Slot(1), b"hi", false);
        assert!(b.receive().is_empty(), "poll 1: held");
        assert!(b.receive().is_empty(), "poll 2: held");
        assert_eq!(b.receive(), [(Slot(0), b"hi".to_vec())], "poll 3: due");
        b.send(Slot(0), b"yo", false);
        a.receive();
        a.receive();
        assert_eq!(a.receive(), [(Slot(1), b"yo".to_vec())]);
    }

    #[test]
    fn a_cut_link_delivers_nothing_more() {
        let (mut a, mut b) = Channels::pair();
        a.send(Slot(1), b"1", false);
        a.cut();
        a.send(Slot(1), b"2", false);
        assert_eq!(b.receive(), [(Slot(0), b"1".to_vec())]);
    }

    #[test]
    fn lossy_channels_lose_about_the_share_asked_for() {
        let mut ends = Channels::mesh(2, 0, 0, 250, 3);
        let (mut b, mut a) = (ends.pop().unwrap(), ends.pop().unwrap());
        for _ in 0..1000 {
            a.send(Slot(1), b"x", false);
        }
        let got = b.receive().len();
        assert!((650..850).contains(&got), "{got}");
    }

    #[test]
    fn udp_locks_onto_the_candidate_that_answers() {
        let mut a = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let mut b = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let (aa, ba) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        // A port nobody listens on, listed first on both sides.
        let dead: SocketAddr = {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.local_addr().unwrap()
        };
        a.peer(Slot(1), vec![dead, ba]);
        b.peer(Slot(0), vec![dead, aa]);
        a.send(Slot(1), b"ping", false);
        let got = wait_for(&mut b, 1);
        assert_eq!(got, [(Slot(0), b"ping".to_vec())]);
        assert_eq!(b.locked(Slot(0)), Some(aa));
        b.send(Slot(0), b"pong", false);
        let got = wait_for(&mut a, 1);
        assert_eq!(got, [(Slot(1), b"pong".to_vec())]);
        assert_eq!(a.locked(Slot(1)), Some(ba));
        // Locked: one copy, to the right place.
        b.send(Slot(0), b"again", false);
        assert_eq!(wait_for(&mut a, 1), [(Slot(1), b"again".to_vec())]);
    }

    #[test]
    fn udp_takes_a_remapped_port_from_the_one_waiting_slot_at_that_ip() {
        let mut a = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        // The server saw b at some other port of the same machine.
        let seen: SocketAddr = {
            let s = UdpSocket::bind("127.0.0.1:0").unwrap();
            s.local_addr().unwrap()
        };
        a.peer(Slot(1), vec![seen]);
        b.send_to(b"hi", a.local_addr().unwrap()).unwrap();
        assert_eq!(wait_for(&mut a, 1), [(Slot(1), b"hi".to_vec())]);
        assert_eq!(a.locked(Slot(1)), Some(b.local_addr().unwrap()));
    }

    #[test]
    fn udp_refuses_a_remapped_port_two_slots_could_own() {
        let mut a = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let b = UdpSocket::bind("127.0.0.1:0").unwrap();
        a.peer(Slot(1), vec!["127.0.0.1:9".parse().unwrap()]);
        a.peer(Slot(2), vec!["127.0.0.1:11".parse().unwrap()]);
        b.send_to(b"?", a.local_addr().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(a.receive().is_empty());
    }

    #[test]
    fn reach_guesses_the_servers_public_address_for_a_peer_on_its_network() {
        let addr = |s: &str| s.parse::<SocketAddr>().unwrap();
        let home = vec![addr("192.168.1.20:7879"), addr("127.0.0.1:7879")];
        assert_eq!(
            reach(home.clone(), Some(addr("203.0.113.5:7878"))),
            [addr("192.168.1.20:7879"), addr("127.0.0.1:7879"), addr("203.0.113.5:7879")]
        );
        // Everyone on one LAN: nothing to guess.
        assert_eq!(reach(home.clone(), Some(addr("192.168.1.20:7878"))), home);
        // A peer the server saw from outside already has its address.
        let away = vec![addr("192.168.43.7:51000"), addr("198.51.100.9:40001")];
        assert_eq!(reach(away.clone(), Some(addr("203.0.113.5:7878"))), away);
        assert_eq!(reach(home.clone(), None), home);
        assert!(!is_public("100.72.1.1".parse().unwrap()));
        assert!(is_public("100.128.1.1".parse().unwrap()));
    }

    #[test]
    fn udp_never_takes_itself_for_a_peer() {
        let mut a = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let b = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let (aa, ba) = (a.local_addr().unwrap(), b.local_addr().unwrap());
        // The peer listed a loopback address on the port this socket holds.
        a.peer(Slot(1), vec![aa, ba]);
        a.send(Slot(1), b"x", false);
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(a.receive().is_empty());
        assert_eq!(a.locked(Slot(1)), None);
    }

    #[test]
    fn udp_drops_what_comes_from_a_stranger() {
        let mut a = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let stranger = UdpSocket::bind("127.0.0.1:0").unwrap();
        stranger.send_to(b"?", a.local_addr().unwrap()).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        assert!(a.receive().is_empty());
    }

    fn wait_for(link: &mut Udp, n: usize) -> Vec<(Slot, Vec<u8>)> {
        let mut out = Vec::new();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while out.len() < n && std::time::Instant::now() < deadline {
            out.extend(link.receive());
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        out
    }
}
