//! Lobbies over real sockets: the client's half played on `session-fake`, as
//! `server`'s own tests play it, but through WebSocket frames.

use std::net::{SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use challenge_core::{Next, Run};
use protocol::{FileReport, Format, Liveness, LobbySettings, LobbyState, ToClient, ToServer, Wire};
use session_api::{ChallengeSpec, StageRunner};
use session_fake::{stage_for, Attempt, FakeRunner};
use transport::Rules;
use tungstenite::{Message, WebSocket};

fn manifests() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests")
}

fn start() -> (SocketAddr, String) {
    start_with(Rules::DEFAULT)
}

fn start_with(rules: Rules) -> (SocketAddr, String) {
    let catalog = server_net::load_catalog(&manifests()).unwrap();
    let hash = format!("{:016x}", catalog.hash());
    (server_net::spawn("127.0.0.1:0", catalog, rules).unwrap(), hash)
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

    fn hello(&mut self, name: &str, hash: &str, lobby: Option<&str>) {
        self.send(&ToServer::Hello {
            name: name.into(),
            library_hash: hash.into(),
            lobby: lobby.map(Into::into),
        });
    }

    /// Reads until a message matches, keeping everything read. Panics after
    /// a few seconds, naming what did arrive.
    fn until<T>(&mut self, mut want: impl FnMut(&ToClient) -> Option<T>) -> T {
        if let Some(t) = self.inbox.iter().find_map(&mut want) {
            return t;
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let read = match self.ws.read() {
                Ok(Message::Text(t)) => Ok(protocol::decode::<ToClient>(t.as_str()).unwrap()),
                Ok(Message::Binary(b)) => Ok(protocol::decode_binary::<ToClient>(&b).unwrap()),
                other => Err(other),
            };
            match read {
                Ok(m) => {
                    let hit = want(&m);
                    self.inbox.push(m);
                    if let Some(t) = hit {
                        return t;
                    }
                }
                Err(Ok(_)) => {}
                Err(Err(tungstenite::Error::Io(e)))
                    if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                Err(Err(e)) => panic!("socket: {e}; got {:?}", self.inbox),
            }
        }
        panic!("timed out; got {:?}", self.inbox);
    }

    fn take(&mut self) -> Vec<ToClient> {
        std::mem::take(&mut self.inbox)
    }

    fn welcome(&mut self) -> LobbyState {
        self.until(|m| match m {
            ToClient::Welcome { lobby, .. } => Some(lobby.clone()),
            _ => None,
        })
    }

    fn refused(&mut self) -> String {
        self.until(|m| match m {
            ToClient::Refused { reason } => Some(reason.clone()),
            _ => None,
        })
    }
}

/// A host and a guest in one lobby, both with their files reported.
fn table() -> (Peer, Peer) {
    table_on(start())
}

fn table_on((addr, hash): (SocketAddr, String)) -> (Peer, Peer) {
    let mut host = Peer::connect(addr);
    host.hello("host", &hash, None);
    let code = host.welcome().code;
    let mut guest = Peer::connect(addr);
    guest.hello("guest", &hash, Some(&code));
    guest.welcome();
    for p in [&mut host, &mut guest] {
        p.send(&ToServer::Files(FileReport::default()));
    }
    host.until(|m| match m {
        ToClient::Lobby(l) if l.can_start && l.players.len() == 2 => Some(()),
        _ => None,
    });
    host.take();
    guest.take();
    (host, guest)
}

#[test]
fn a_host_makes_a_lobby_and_a_guest_joins_it_by_code() {
    let (addr, hash) = start();
    let mut host = Peer::connect(addr);
    host.hello("host", &hash, None);
    let lobby = host.welcome();
    assert_eq!(lobby.code.len(), 5);
    let mut guest = Peer::connect(addr);
    // Codes are read without regard to case.
    guest.hello("guest", &hash, Some(&lobby.code.to_lowercase()));
    assert_eq!(guest.welcome().code, lobby.code);
    let players = host.until(|m| match m {
        ToClient::Lobby(l) if l.players.len() == 2 => Some(l.players.clone()),
        _ => None,
    });
    assert!(players[0].host && !players[1].host);
    assert_eq!(players[1].id, "guest");
}

#[test]
fn an_unknown_code_is_refused() {
    let (addr, hash) = start();
    let mut p = Peer::connect(addr);
    p.hello("x", &hash, Some("ZZZZZ"));
    assert!(p.refused().contains("no lobby ZZZZZ"));
}

#[test]
fn another_library_is_refused() {
    let (addr, _) = start();
    let mut p = Peer::connect(addr);
    p.hello("x", "0000000000000000", None);
    assert!(p.refused().contains("library"), "{:?}", p.inbox);
}

#[test]
fn a_lobby_closes_when_everyone_has_left() {
    let (addr, hash) = start();
    let mut host = Peer::connect(addr);
    host.hello("host", &hash, None);
    let code = host.welcome().code;
    drop(host);
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let mut p = Peer::connect(addr);
        p.hello("late", &hash, Some(&code));
        let got = p.until(|m| match m {
            ToClient::Refused { reason } => Some(Err(reason.clone())),
            ToClient::Welcome { .. } => Some(Ok(())),
            _ => None,
        });
        if got.is_err() {
            break;
        }
        assert!(Instant::now() < deadline, "the lobby outlived its last connection");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The client's loop on a fake runner that meets the criteria.
fn play(peer: &mut Peer, round: u64, spec: &ChallengeSpec, limit: u64) {
    let stage = stage_for(spec);
    let at = spec.complete_when.survive_frames.map_or(600, |n| n.max(600));
    let mut runner = FakeRunner::new(stage.clone(), vec![Attempt::meeting(&spec.complete_when, at)]);
    let mut run = Run::new(spec.clone()).unwrap().with_limit(limit);
    runner.enter(&stage).unwrap();
    run.enter();
    loop {
        let events = runner.advance().unwrap().to_vec();
        let frame = runner.frame();
        let next = run.observe(frame, &events);
        if !events.is_empty() || next != Next::Continue {
            let wire = events.iter().map(Into::into).collect();
            peer.send(&ToServer::Frame { round, frame, events: wire });
        }
        match next {
            Next::Continue => {}
            Next::Reenter => {
                runner.enter(&stage).unwrap();
                run.enter();
            }
            Next::Finished(result) => {
                peer.send(&ToServer::Finished { round, outcome: (&result).into() });
                return;
            }
        }
    }
}

#[test]
fn two_players_play_a_round_over_the_wire() {
    let (mut host, mut guest) = table();
    host.send(&ToServer::Settings(LobbySettings { format: Format::Challenges(1), ..Default::default() }));
    host.send(&ToServer::Start);
    let dealt = |p: &mut Peer| {
        p.until(|m| match m {
            ToClient::Dealt { round, challenge, seed, limit_frames, players, .. } => {
                Some((*round, challenge.clone(), *seed, *limit_frames, players.clone()))
            }
            _ => None,
        })
    };
    let (round, cref, seed, limit, players) = dealt(&mut host);
    assert_eq!(dealt(&mut guest), (round, cref.clone(), seed, limit, players.clone()), "one deal, one seed");
    assert_eq!(players, ["host", "guest"], "in player order, which is slot order in a bout");

    for p in [&mut host, &mut guest] {
        p.send(&ToServer::Prepared { round, artifact: "same".into() });
    }
    for p in [&mut host, &mut guest] {
        p.until(|m| matches!(m, ToClient::Go { round: r } if *r == round).then_some(()));
    }
    let catalog = server_net::load_catalog(&manifests()).unwrap();
    let lib = catalog.game(&cref.game).unwrap();
    let spec = lib.challenge(&cref.challenge).unwrap().spec(&lib.game.id);
    play(&mut host, round, &spec, limit);
    play(&mut guest, round, &spec, limit);

    for p in [&mut host, &mut guest] {
        let entries = p.until(|m| match m {
            ToClient::RoundResult { entries, .. } => Some(entries.clone()),
            _ => None,
        });
        assert_eq!(entries.len(), 2);
        assert!(entries.iter().all(|e| e.completed), "{entries:?}");
        p.until(|m| matches!(m, ToClient::MatchOver { .. }).then_some(()));
        assert!(!p.inbox.iter().any(|m| matches!(m, ToClient::Error { .. })), "{:?}", p.inbox);
    }
}

#[test]
fn a_guest_leaving_mid_match_ends_it_for_the_host() {
    let (mut host, guest) = table();
    host.send(&ToServer::Start);
    host.until(|m| matches!(m, ToClient::Dealt { .. }).then_some(()));
    drop(guest);
    let message = host.until(|m| match m {
        ToClient::Error { message } => Some(message.clone()),
        _ => None,
    });
    assert_eq!(message, "guest left the match");
    host.until(|m| matches!(m, ToClient::MatchOver { .. }).then_some(()));
}

#[test]
fn a_guest_gone_silent_mid_match_ends_it_for_the_host() {
    // A link that drops without a close: the guest's socket stays open but
    // nothing reads it, so the server's pings go unanswered.
    let quick = Rules {
        liveness: Liveness { ping_every: Duration::from_millis(100), silence: Duration::from_secs(1) },
        ..Rules::DEFAULT
    };
    let (mut host, _guest) = table_on(start_with(quick));
    host.send(&ToServer::Start);
    host.until(|m| matches!(m, ToClient::Dealt { .. }).then_some(()));
    let message = host.until(|m| match m {
        ToClient::Error { message } => Some(message.clone()),
        _ => None,
    });
    assert_eq!(message, "guest left the match", "and the host, who answers its pings, stays");
}

#[test]
fn a_finished_player_watches_the_other_over_the_wire() {
    let (mut host, mut guest) = table();
    host.send(&ToServer::Settings(LobbySettings { format: Format::Challenges(1), ..Default::default() }));
    host.send(&ToServer::Start);
    let (round, cref, limit) = host.until(|m| match m {
        ToClient::Dealt { round, challenge, limit_frames, .. } => Some((*round, challenge.clone(), *limit_frames)),
        _ => None,
    });
    guest.until(|m| matches!(m, ToClient::Dealt { .. }).then_some(()));
    for p in [&mut host, &mut guest] {
        p.send(&ToServer::Prepared { round, artifact: "same".into() });
    }
    for p in [&mut host, &mut guest] {
        p.until(|m| matches!(m, ToClient::Go { round: r } if *r == round).then_some(()));
    }
    let catalog = server_net::load_catalog(&manifests()).unwrap();
    let lib = catalog.game(&cref.game).unwrap();
    let spec = lib.challenge(&cref.challenge).unwrap().spec(&lib.game.id);
    play(&mut host, round, &spec, limit);
    host.until(|m| matches!(m, ToClient::PlayerFinished { .. }).then_some(()));

    host.send(&ToServer::Watch { round, player: Some("guest".into()) });
    let watchers = guest.until(|m| match m {
        ToClient::Watched { watchers, .. } => Some(*watchers),
        _ => None,
    });
    assert_eq!(watchers, 1);
    guest.send(&ToServer::Video {
        round, frame: 42, key: true, width: 256, height: 240, aspect: 1.0, data: vec![0, 0, 1].into(),
    });
    guest.send(&ToServer::Audio { round, frame: 42, rate: 32040, channels: 2, data: vec![0].into() });
    let (player, frame) = host.until(|m| match m {
        ToClient::Video { player, frame, .. } => Some((player.clone(), *frame)),
        _ => None,
    });
    assert_eq!((player.as_str(), frame), ("guest", 42));
    host.until(|m| matches!(m, ToClient::Audio { player, .. } if player == "guest").then_some(()));

    play(&mut guest, round, &spec, limit);
    for p in [&mut host, &mut guest] {
        p.until(|m| matches!(m, ToClient::RoundResult { .. }).then_some(()));
        assert!(!p.inbox.iter().any(|m| matches!(m, ToClient::Error { .. })), "{:?}", p.inbox);
    }
    assert!(!guest.inbox.iter().any(|m| matches!(m, ToClient::Video { .. })), "a stream never comes back to its sender");
}
