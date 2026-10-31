//! A player on a slow link, as the first match over the internet had: a
//! phone's 0.2 Mbit/s down and a few hundred kbit/s up (ADR 0034). Each
//! client runs `transport`'s loop, as `app-client`'s link does, and one of
//! them reaches the server through a relay that holds its link to a phone's
//! rates. A watched player streams about what the client sends — 0.5 Mbit/s
//! of picture and 64 kbit/s of sound — while still playing.
//!
//! What must hold: the match is never lost to the stream, a watcher hears
//! where the run stands within a second or two, and the picture they do get
//! is fresh rather than minutes behind.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use challenge_core::{Next as RunNext, Run};
use protocol::{FileReport, Format, LobbySettings, ToClient, ToServer};
use session_api::{ChallengeSpec, StageRunner};
use session_fake::{stage_for, Attempt, FakeRunner};
use transport::{Ends, Next, Rules};

/// A phone's link, as measured on the one that failed: bytes a second.
const PHONE_DOWN: u64 = 25_000;
const PHONE_UP: u64 = 50_000;

/// How long the watched player streams for.
const STREAM_FOR: Duration = Duration::from_secs(6);

fn manifests() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests")
}

/// A client as the app runs one: `transport`'s loop on a thread of its own.
struct Wired {
    tx: Sender<ToServer>,
    rx: Receiver<(Instant, ToClient)>,
    inbox: Vec<(Instant, ToClient)>,
}

impl Wired {
    fn connect(addr: SocketAddr) -> Wired {
        let stream = TcpStream::connect(addr).unwrap();
        let (mut ws, _) = tungstenite::client(format!("ws://{addr}/"), stream).unwrap();
        transport::prepare(ws.get_ref()).unwrap();
        let (tx, out) = mpsc::channel::<ToServer>();
        let (heard, rx) = mpsc::channel();
        thread::spawn(move || {
            let mut incoming = |m: ToClient| match heard.send((Instant::now(), m)) {
                Ok(()) => Next::Go,
                Err(_) => Next::Stop,
            };
            transport::run(&mut ws, Rules::DEFAULT, &out, Ends { incoming: &mut incoming, key_wanted: &mut || {}, label: "test" })
        });
        Wired { tx, rx, inbox: Vec::new() }
    }

    fn send(&self, m: ToServer) {
        self.tx.send(m).unwrap();
    }

    fn drain(&mut self) {
        self.inbox.extend(self.rx.try_iter());
    }

    fn until<T>(&mut self, mut want: impl FnMut(&ToClient) -> Option<T>) -> T {
        if let Some(t) = self.inbox.iter().find_map(|(_, m)| want(m)) {
            return t;
        }
        let deadline = Instant::now() + Duration::from_secs(15);
        while let Some(left) = deadline.checked_duration_since(Instant::now()) {
            let Ok((at, m)) = self.rx.recv_timeout(left) else { break };
            let hit = want(&m);
            self.inbox.push((at, m));
            if let Some(t) = hit {
                return t;
            }
        }
        panic!("timed out; got {:?}", self.inbox.iter().map(|(_, m)| m).collect::<Vec<_>>());
    }

    fn errors(&self) -> Vec<String> {
        self.inbox
            .iter()
            .filter_map(|(_, m)| match m {
                ToClient::Error { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }
}

/// A relay holding the link between a client and `server` to `down` and
/// `up` bytes a second. What it cannot pass on yet waits in its sockets'
/// buffers, as it would in a phone network's.
fn throttled(server: SocketAddr, down: u64, up: u64) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    thread::spawn(move || {
        for client in listener.incoming() {
            let client = client.unwrap();
            let upstream = TcpStream::connect(server).unwrap();
            pipe(client.try_clone().unwrap(), upstream.try_clone().unwrap(), up);
            pipe(upstream, client, down);
        }
    });
    addr
}

fn pipe(mut from: TcpStream, mut to: TcpStream, rate: u64) {
    thread::spawn(move || {
        let _ = to.set_nodelay(true);
        let mut buf = [0u8; 512];
        let start = Instant::now();
        let mut passed = 0u64;
        loop {
            let n = match from.read(&mut buf) {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            if to.write_all(&buf[..n]).is_err() {
                break;
            }
            passed += n as u64;
            let due = start + Duration::from_secs_f64(passed as f64 / rate as f64);
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                thread::sleep(wait);
            }
        }
        let _ = to.shutdown(Shutdown::Both);
    });
}

/// The client's loop on a fake runner that meets the criteria. Its events
/// fall on frame 600 or later, so frames before that are free for the
/// stream's own reports.
fn play(c: &Wired, round: u64, spec: &ChallengeSpec, limit: u64) {
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
        if !events.is_empty() || next != RunNext::Continue {
            c.send(ToServer::Frame { round, frame, events: events.iter().map(Into::into).collect() });
        }
        match next {
            RunNext::Continue => {}
            RunNext::Reenter => {
                runner.enter(&stage).unwrap();
                run.enter();
            }
            RunNext::Finished(result) => {
                c.send(ToServer::Finished { round, outcome: (&result).into() });
                return;
            }
        }
    }
}

/// Microseconds since `epoch`, at the head of every payload, so the watcher
/// can tell how old a picture is.
fn stamped(epoch: Instant, len: usize) -> Vec<u8> {
    let mut v = (epoch.elapsed().as_micros() as u64).to_le_bytes().to_vec();
    v.resize(len.max(8), 0x5A);
    v
}

fn age(epoch: Instant, at: Instant, data: &[u8]) -> Duration {
    let sent = Duration::from_micros(u64::from_le_bytes(data[..8].try_into().unwrap()));
    at.duration_since(epoch).saturating_sub(sent)
}

/// One match: `guest` finishes, then watches `host` stream while still
/// playing. `slow_host` puts the host behind the phone's link, else the
/// guest is behind it.
fn watch_over_a_slow_link(slow_host: bool) {
    let catalog = server_net::load_catalog(&manifests()).unwrap();
    let hash = format!("{:016x}", catalog.hash());
    let server = server_net::spawn("127.0.0.1:0", catalog.clone(), Rules::DEFAULT).unwrap();
    let phone = throttled(server, PHONE_DOWN, PHONE_UP);
    let (host_at, guest_at) = if slow_host { (phone, server) } else { (server, phone) };

    let mut host = Wired::connect(host_at);
    host.send(ToServer::Hello { name: "host".into(), library_hash: hash.clone(), lobby: None });
    let code = host.until(|m| match m {
        ToClient::Welcome { lobby, .. } => Some(lobby.code.clone()),
        _ => None,
    });
    let mut guest = Wired::connect(guest_at);
    guest.send(ToServer::Hello { name: "guest".into(), library_hash: hash, lobby: Some(code) });
    guest.until(|m| matches!(m, ToClient::Welcome { .. }).then_some(()));
    for c in [&host, &guest] {
        c.send(ToServer::Files(FileReport::default()));
    }
    host.until(|m| matches!(m, ToClient::Lobby(l) if l.can_start && l.players.len() == 2).then_some(()));
    host.send(ToServer::Settings(LobbySettings { format: Format::Challenges(1), ..Default::default() }));
    host.send(ToServer::Start);
    let (round, cref, limit) = host.until(|m| match m {
        ToClient::Dealt { round, challenge, limit_frames, .. } => Some((*round, challenge.clone(), *limit_frames)),
        _ => None,
    });
    for c in [&host, &guest] {
        c.send(ToServer::Prepared { round, artifact: "same".into() });
    }
    for c in [&mut host, &mut guest] {
        c.until(|m| matches!(m, ToClient::Go { round: r } if *r == round).then_some(()));
    }
    let lib = catalog.game(&cref.game).unwrap();
    let spec = lib.challenge(&cref.challenge).unwrap().spec(&lib.game.id);

    play(&guest, round, &spec, limit);
    host.until(|m| matches!(m, ToClient::PlayerFinished { player, .. } if player == "guest").then_some(()));
    guest.send(ToServer::Watch { round, player: Some("host".into()) });
    host.until(|m| matches!(m, ToClient::Watched { watchers: 1, .. }).then_some(()));

    // The host streams while playing on, reporting a frame twice a second
    // as the client does while watched.
    let epoch = Instant::now();
    let mut reported: HashMap<u64, Instant> = HashMap::new();
    let mut sent = 0u32;
    for frame in 1..=(STREAM_FOR.as_secs() * 60) {
        let key = frame % 120 == 1;
        host.send(ToServer::Video {
            round, frame, key, width: 256, height: 240, aspect: 1.0,
            data: stamped(epoch, if key { 6_000 } else { 1_000 }).into(),
        });
        host.send(ToServer::Audio { round, frame, rate: 16_000, channels: 1, data: stamped(epoch, 140).into() });
        sent += 1;
        if frame % 30 == 0 {
            reported.insert(frame, Instant::now());
            host.send(ToServer::Frame { round, frame, events: Vec::new() });
        }
        let due = epoch + Duration::from_micros(16_667 * frame);
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait);
        }
    }
    play(&host, round, &spec, limit);
    for c in [&mut host, &mut guest] {
        c.until(|m| matches!(m, ToClient::RoundResult { .. }).then_some(()));
    }
    guest.drain();

    // The match was never the stream's to lose.
    assert!(host.errors().is_empty() && guest.errors().is_empty(), "{:?} {:?}", host.errors(), guest.errors());

    // Where the run stood reached the watcher promptly, every time.
    let mut late = Vec::new();
    for (at, m) in &guest.inbox {
        if let ToClient::Progress { frame, .. } = m {
            if let Some(sent_at) = reported.get(frame) {
                late.push(at.duration_since(*sent_at));
            }
        }
    }
    let worst_progress = late.iter().max().copied().unwrap_or_default();

    // The picture the watcher got was fresh, and some were dropped for it.
    let pictures: Vec<(Duration, bool)> = guest
        .inbox
        .iter()
        .filter_map(|(at, m)| match m {
            ToClient::Video { data, key, .. } => Some((age(epoch, *at, &data.0), *key)),
            _ => None,
        })
        .collect();
    let worst_picture = pictures.iter().map(|(a, _)| *a).max().unwrap_or_default();
    let keys = pictures.iter().filter(|(_, k)| *k).count();
    println!(
        "slow {}: progress {} of {} reports, worst {} ms; pictures {} of {} sent, {} key, worst {} ms",
        if slow_host { "uplink" } else { "downlink" },
        late.len(),
        reported.len(),
        worst_progress.as_millis(),
        pictures.len(),
        sent,
        keys,
        worst_picture.as_millis(),
    );
    assert!(late.len() == reported.len(), "every report reached the watcher: {} of {}", late.len(), reported.len());
    assert!(worst_progress < Duration::from_millis(2_000), "progress waited {worst_progress:?} behind the stream");
    assert!(!pictures.is_empty() && keys >= 1, "the watcher saw something");
    assert!(pictures.len() < sent as usize, "a link this slow cannot carry every picture");
    assert!(worst_picture < Duration::from_millis(2_500), "a picture arrived {worst_picture:?} late");
}

#[test]
fn a_watcher_on_a_slow_downlink_gets_a_fresh_picture_and_the_whole_match() {
    watch_over_a_slow_link(false);
}

#[test]
fn a_player_streaming_up_a_slow_uplink_keeps_their_run() {
    watch_over_a_slow_link(true);
}
