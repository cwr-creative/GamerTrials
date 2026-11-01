//! A bout brokered over real sockets (ADR 0036): two players' probes reach
//! the server's UDP side, each is told where the other is, and the round
//! is judged from both players' frames. The first test sends the facts as
//! a rollback session would confirm them; the second opens the players'
//! own link — two `Rollback` sessions on fake machines over real UDP, found
//! through the real rendezvous — and plays the bout through both judges.

use std::net::{SocketAddr, TcpStream, UdpSocket};
use std::path::Path;
use std::time::{Duration, Instant};

use challenge_core::Bout;
use challenge_library::Catalog;
use head_to_head::{Rollback, Settings, State, Udp};
use protocol::{Ending, FileReport, Format, LobbySettings, Mode, RunOutcome, ToClient, ToServer, Wire};
use session_api::{AssetKey, EntryPoint, GameId, Slot, SlotInput, StageId, StageRunner, StageSpec};
use session_fake::FakeMachine;
use transport::Rules;
use tungstenite::{Message, WebSocket};

const FIGHT: &str = r#"
schema_version = 3
[game]
id = "fight"
title = "Fight"
console = "snes"
framerate = 60.0
[[rom]]
id = "base"
[[rule]]
field = "p1_rounds"
edge = "increases"
emit = "count"
counter = "rounds_won"
slot = 0
[[rule]]
field = "p2_rounds"
edge = "increases"
emit = "count"
counter = "rounds_won"
slot = 1
[[stage]]
id = "vs"
rom = "base"
[[challenge]]
id = "fight-best-of-three"
stage = "vs"
kind = "head_to_head"
slots = 2
complete_when = { counts = { rounds_won = 2 } }
limit_seconds = 100
"#;

fn catalog() -> Catalog {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mut texts = vec![FIGHT.to_string()];
    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    for f in files {
        texts.push(std::fs::read_to_string(f).unwrap());
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    Catalog::parse_all(&refs).unwrap()
}

struct Peer {
    ws: WebSocket<TcpStream>,
    inbox: Vec<ToClient>,
}

impl Peer {
    fn connect(addr: SocketAddr) -> Peer {
        let stream = TcpStream::connect(addr).unwrap();
        stream.set_read_timeout(Some(Duration::from_millis(20))).unwrap();
        let (ws, _) = tungstenite::client(format!("ws://{addr}/"), stream).unwrap();
        Peer { ws, inbox: Vec::new() }
    }

    fn send(&mut self, m: &ToServer) {
        let frame = match protocol::to_wire(m.clone()) {
            Wire::Text(t) => Message::Text(t.into()),
            Wire::Binary(b) => Message::Binary(b.into()),
        };
        self.ws.send(frame).unwrap();
    }

    fn until<T>(&mut self, mut want: impl FnMut(&ToClient) -> Option<T>) -> T {
        if let Some(t) = self.inbox.iter().find_map(&mut want) {
            return t;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            match self.ws.read() {
                Ok(Message::Text(t)) => {
                    let m = protocol::decode::<ToClient>(t.as_str()).unwrap();
                    let hit = want(&m);
                    self.inbox.push(m);
                    if let Some(t) = hit {
                        return t;
                    }
                }
                Ok(_) => {}
                Err(tungstenite::Error::Io(e))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(e) => panic!("socket: {e}; got {:?}", self.inbox),
            }
        }
        panic!("timed out; got {:?}", self.inbox);
    }
}

fn won(frame: u64) -> protocol::Event {
    (&session_api::SessionEvent::Counted { frame, counter: "rounds_won".into(), count: 1 }).into()
}

fn outcome(ending: Ending, frames: u64) -> RunOutcome {
    RunOutcome { challenge: "fight-best-of-three".into(), kind: protocol::Kind::HeadToHead, ending, frames, attempts: 1 }
}

/// A lobby of two with the fight enabled and dealt: the server's UDP
/// address, the two players in slot order with their names, and the round.
/// Who sits in which slot is the seed's to say, so the test takes what it
/// is dealt.
fn dealt() -> (SocketAddr, Vec<Peer>, Vec<String>, u64) {
    let catalog = catalog();
    let hash = format!("{:016x}", catalog.hash());
    let (addr, udp) = server_net::spawn_with_rendezvous("127.0.0.1:0", catalog, Rules::DEFAULT).unwrap();

    let mut host = Peer::connect(addr);
    host.send(&ToServer::Hello { name: "host".into(), library_hash: hash.clone(), lobby: None });
    let (code, rendezvous) = host.until(|m| match m {
        ToClient::Welcome { lobby, rendezvous, .. } => Some((lobby.code.clone(), rendezvous.clone())),
        _ => None,
    });
    assert_eq!(rendezvous.as_deref(), Some(format!(":{}", udp.port()).as_str()), "told where to probe");
    let mut guest = Peer::connect(addr);
    guest.send(&ToServer::Hello { name: "guest".into(), library_hash: hash, lobby: Some(code) });
    guest.until(|m| matches!(m, ToClient::Welcome { .. }).then_some(()));
    for p in [&mut host, &mut guest] {
        p.send(&ToServer::Files(FileReport::default()));
    }
    host.send(&ToServer::Settings(LobbySettings {
        mode: Mode::Combo,
        format: Format::Challenges(1),
        allow_head_to_head: true,
        ..Default::default()
    }));
    host.send(&ToServer::Enable {
        challenges: vec![protocol::ChallengeRef { game: "fight".into(), challenge: "fight-best-of-three".into() }],
    });
    host.until(|m| match m {
        ToClient::Lobby(l) if l.can_start && l.players.len() == 2 => Some(()),
        _ => None,
    });
    host.send(&ToServer::Start);
    let (round, slots) = host.until(|m| match m {
        ToClient::Dealt { round, players, kind: protocol::Kind::HeadToHead, .. } => Some((*round, players.clone())),
        _ => None,
    });
    assert_eq!(slots.len(), 2);
    let mut peers = vec![host, guest];
    if slots[0] == "guest" {
        peers.reverse();
    }
    (udp, peers, slots, round)
}

#[test]
fn two_players_are_brokered_and_judged_over_the_wire() {
    let (udp, mut peers, slots, round) = dealt();

    // Each player probes the server from the socket it will play on, and
    // says where it thinks it is.
    let sockets: Vec<UdpSocket> = (0..2).map(|_| UdpSocket::bind("127.0.0.1:0").unwrap()).collect();
    for (i, p) in peers.iter_mut().enumerate() {
        let token = 7000 + i as u64;
        sockets[i].send_to(&token.to_le_bytes(), udp).unwrap();
        p.send(&ToServer::Endpoint { round, token, local: format!("192.168.1.{i}:5000") });
    }
    for (i, p) in peers.iter_mut().enumerate() {
        let got = p.until(|m| match m {
            ToClient::Peers { round: r, peers } if *r == round => Some(peers.clone()),
            _ => None,
        });
        assert_eq!(got.len(), 2);
        for (j, peer) in got.iter().enumerate() {
            assert_eq!((peer.slot, &peer.player), (j as u8, &slots[j]));
            assert_eq!(peer.candidates[0], format!("192.168.1.{j}:5000"));
            assert_eq!(peer.candidates[1], sockets[j].local_addr().unwrap().to_string(), "the address the probe came from");
        }
        let _ = i;
        p.send(&ToServer::Prepared { round, artifact: "same".into() });
    }
    for p in peers.iter_mut() {
        p.until(|m| matches!(m, ToClient::Go { round: r } if *r == round).then_some(()));
    }

    // Slot 0 wins at 100 and 300; slot 1 at 200. The loser's claim beats
    // the winner's last frame to the server.
    peers[0].send(&ToServer::Frame { round, frame: 100, events: vec![won(100)] });
    peers[1].send(&ToServer::Frame { round, frame: 200, events: vec![won(200)] });
    peers[1].send(&ToServer::Frame { round, frame: 300, events: vec![] });
    peers[1].send(&ToServer::Finished { round, outcome: outcome(Ending::Beaten, 300) });
    peers[0].send(&ToServer::Frame { round, frame: 300, events: vec![won(300)] });
    peers[0].send(&ToServer::Finished { round, outcome: outcome(Ending::Completed, 300) });

    for p in peers.iter_mut() {
        let entries = p.until(|m| match m {
            ToClient::RoundResult { entries, .. } => Some(entries.clone()),
            _ => None,
        });
        assert_eq!(entries.len(), 2);
        let of = |name: &str| entries.iter().find(|e| e.player == name).unwrap().clone();
        assert_eq!((of(&slots[0]).place, of(&slots[0]).completed), (1, true));
        assert_eq!(of(&slots[1]).outcome.ending, Ending::Beaten);
        p.until(|m| matches!(m, ToClient::MatchOver { .. }).then_some(()));
        assert!(!p.inbox.iter().any(|m| matches!(m, ToClient::Error { .. })), "{:?}", p.inbox);
    }
}

fn stage() -> StageSpec {
    StageSpec {
        game: GameId("fight".into()),
        stage: StageId("vs".into()),
        content: AssetKey("fake:fight".into()),
        entry: EntryPoint::Named("vs".into()),
    }
}

/// Slot 0 wins rounds on frames 100 and 300, slot 1 on 200: a pad press
/// on the frame before each, with no input delay.
fn script(slot: Slot, next_frame: u64) -> SlotInput {
    let wins: &[u64] = if slot.0 == 0 { &[100, 300] } else { &[200] };
    SlotInput(u32::from(wins.contains(&next_frame)))
}

/// The whole of a bout, the way the client will play it: the players'
/// machines found through the rendezvous, kept in step by rollback over
/// UDP, each client judging provisionally from the confirmed facts and
/// claiming what it saw, and the server agreeing with both.
#[test]
fn a_bout_is_played_over_udp_between_two_machines() {
    let (server_udp, mut peers, slots, round) = dealt();
    let spec = {
        let catalog = catalog();
        let lib = catalog.game("fight").unwrap();
        lib.challenge("fight-best-of-three").unwrap().spec(&lib.game.id)
    };

    // Probe from the socket the bout will run on; the server's answer
    // names each machine by what its probe came from and what it said.
    let mut links: Vec<Option<Udp>> = Vec::new();
    for (i, p) in peers.iter_mut().enumerate() {
        let udp = Udp::bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let token = 9000 + i as u64;
        udp.probe(server_udp, token).unwrap();
        p.send(&ToServer::Endpoint { round, token, local: udp.local_addr().unwrap().to_string() });
        links.push(Some(udp));
    }
    for (i, p) in peers.iter_mut().enumerate() {
        let got = p.until(|m| match m {
            ToClient::Peers { round: r, peers } if *r == round => Some(peers.clone()),
            _ => None,
        });
        let udp = links[i].as_mut().unwrap();
        for peer in got.iter().filter(|peer| usize::from(peer.slot) != i) {
            let candidates = peer.candidates.iter().map(|c| c.parse().unwrap()).collect();
            udp.peer(Slot(peer.slot), candidates);
        }
    }

    let settings = Settings { input_delay: 0, check_every: 10, ..Default::default() };
    let mut sessions: Vec<Rollback<FakeMachine>> = links
        .into_iter()
        .enumerate()
        .map(|(i, udp)| {
            let mut m = FakeMachine::new(stage(), 2);
            m.enter(&stage()).unwrap();
            Rollback::connect(m, udp.unwrap(), Slot(i as u8), 2, settings).unwrap()
        })
        .collect();

    // Prepared only once the machines have found each other (ADR 0036).
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let states: Vec<State> = sessions.iter_mut().map(|s| s.poll().clone()).collect();
        if states.iter().all(|s| *s == State::Live) {
            break;
        }
        assert!(Instant::now() < deadline, "never synchronized: {states:?}");
        std::thread::sleep(Duration::from_millis(5));
    }
    for p in peers.iter_mut() {
        p.send(&ToServer::Prepared { round, artifact: "same".into() });
    }
    for p in peers.iter_mut() {
        p.until(|m| matches!(m, ToClient::Go { round: r } if *r == round).then_some(()));
    }

    let mut bouts: Vec<Bout> = (0..2).map(|_| {
        let mut b = Bout::new(spec.clone(), Some(6000)).unwrap();
        b.enter();
        b
    }).collect();
    let mut claimed = [false, false];
    for _ in 0..20_000 {
        if claimed.iter().all(|c| *c) {
            break;
        }
        for i in 0..2 {
            if claimed[i] {
                continue;
            }
            let me = Slot(i as u8);
            let next = sessions[i].frame() + 1;
            let confirmed = sessions[i].pump(&[(me, script(me, next))]).unwrap();
            for c in confirmed {
                // My own facts go to the server; every slot's to my judge.
                let mine: Vec<protocol::Event> = c
                    .events
                    .iter()
                    .filter(|e| e.slot.is_none() || e.slot == Some(me))
                    .map(|e| (&e.event).into())
                    .collect();
                if !mine.is_empty() || c.frame % 15 == 0 {
                    peers[i].send(&ToServer::Frame { round, frame: c.frame, events: mine });
                }
                let mut verdicts = Vec::new();
                for s in 0..2u8 {
                    let theirs: Vec<session_api::SessionEvent> = c
                        .events
                        .iter()
                        .filter(|e| e.slot.is_none() || e.slot == Some(Slot(s)))
                        .map(|e| e.event.clone())
                        .collect();
                    verdicts.extend(bouts[i].observe(Slot(s), c.frame, &theirs));
                }
                if let Some((_, result)) = verdicts.into_iter().find(|(s, _)| *s == me) {
                    peers[i].send(&ToServer::Finished { round, outcome: (&result).into() });
                    claimed[i] = true;
                }
            }
        }
        std::thread::sleep(Duration::from_micros(200));
    }
    assert!(claimed.iter().all(|c| *c), "{:?}", sessions.iter().map(|s| s.stats()).collect::<Vec<_>>());

    for (i, p) in peers.iter_mut().enumerate() {
        let entries = p.until(|m| match m {
            ToClient::RoundResult { entries, .. } => Some(entries.clone()),
            _ => None,
        });
        let of = |name: &str| entries.iter().find(|e| e.player == name).unwrap().clone();
        assert_eq!(of(&slots[0]).outcome, outcome(Ending::Completed, 300), "{i}: {entries:?}");
        assert_eq!(of(&slots[1]).outcome, outcome(Ending::Beaten, 300), "{i}: {entries:?}");
        p.until(|m| matches!(m, ToClient::MatchOver { .. }).then_some(()));
        assert!(!p.inbox.iter().any(|m| matches!(m, ToClient::Error { .. })), "{:?}", p.inbox);
    }
    let stats: Vec<_> = sessions.iter().map(|s| s.stats()).collect();
    println!("bout over udp: {stats:?}");
}
