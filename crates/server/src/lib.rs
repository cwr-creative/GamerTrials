//! The server half of a match (ADR 0009): a lobby, then a match dealt, judged
//! and ranked by `challenge-core`, driven entirely by [`protocol`] messages.
//!
//! This is a state machine, not a service. [`Server::handle`] takes one
//! message from one connection and returns the messages to send; whoever
//! owns the sockets delivers them. Solo play drives it over an in-process
//! channel and `server-net` drives it over WebSockets (ADR 0030), and
//! neither changes a rule here, because there is nothing else here.
//!
//! What it never does: run a game, read a ROM, hold a path. Its `Cargo.toml`
//! is the fence (ADR 0024).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use challenge_core::{Bout, Deal, Entry, Match, Next, Run, RunResult, Settings};
use challenge_library::{Catalog, Challenge, Library};
use protocol::{
    ChallengeRef, FileReport, LobbyPlayer, LobbySettings, LobbyState, Peer, RunOutcome,
    ToClient, ToServer, MAX_THUMBS,
};
use session_api::{PlayerId, Slot};

/// How long the players not seated in a bout have to back a slot before
/// one is assigned to them (ADR 0036).
pub const BACKING_SECONDS: u32 = 15;
/// After a seated player reports its link lost, how long the server waits
/// for the other side's own report or give-up before calling the bout.
pub const LOST_GRACE: Duration = Duration::from_secs(2);

/// One connection, as the transport names it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ConnId(pub u64);

/// What to send, and to whom.
pub type Outbox = Vec<(ConnId, ToClient)>;

struct Seat {
    conn: ConnId,
    id: PlayerId,
    name: String,
    files: Option<FileReport>,
}

/// Where a seated player's machine can be reached, as it comes in.
#[derive(Default)]
struct Endpoint {
    token: Option<u64>,
    local: Option<String>,
    public: Option<String>,
}

struct SeatRun {
    /// This player's own run. `None` in a bout, where the [`Bout`] holds it.
    run: Option<Run>,
    artifact: Option<String>,
    /// The server's verdict.
    verdict: Option<RunResult>,
    /// The client's provisional one, to be checked against the verdict.
    claimed: Option<RunOutcome>,
    /// Bout: the player said its link died on this frame. No claim will
    /// come; the server's ruling stands alone.
    lost: Option<u64>,
    endpoint: Endpoint,
}

struct RoundState {
    deal: Deal,
    /// One per seated player: everyone, or a bout's slots.
    seats: BTreeMap<PlayerId, SeatRun>,
    /// The judge of a head-to-head round (ADR 0036).
    bout: Option<Bout>,
    /// `Go` has been sent: runs are live.
    started: bool,
    /// Who watches whom (ADR 0032). Dropped with the round, so nothing is
    /// watched across one.
    watching: BTreeMap<PlayerId, PlayerId>,
    /// Whom each player shows in their sidebar (ADR 0038). Likewise.
    thumbs: BTreeMap<PlayerId, Vec<PlayerId>>,
    /// When the deal was first seen by the clock, for the backing window.
    dealt_at: Option<Instant>,
    peers_sent: bool,
    /// When the first lost link was first seen by the clock.
    lost_at: Option<Instant>,
}

struct Live {
    m: Match,
    round: Option<RoundState>,
    /// Everyone who began this match and has not left. They have a table
    /// open, and a score card on it once the match is over; a player who
    /// joined after that has neither.
    table: BTreeSet<ConnId>,
}

impl Live {
    /// Its `MatchOver` has gone out: all that is left of it is score cards.
    fn finished(&self) -> bool {
        self.round.is_none()
    }
}

pub struct Server {
    catalog: Catalog,
    code: String,
    settings: LobbySettings,
    enabled: BTreeSet<ChallengeRef>,
    seats: Vec<Seat>,
    /// Connected and not yet said hello.
    unnamed: BTreeSet<ConnId>,
    next_conn: u64,
    /// The seed the next match takes. Stepped after each.
    seed: u64,
    live: Option<Live>,
    /// Where a head-to-head player's probes go, told at hello. `None` when
    /// this server has no UDP side and deals no bouts.
    rendezvous: Option<String>,
    /// Probes seen before the `Endpoint` naming their token arrived.
    probes: BTreeMap<u64, String>,
}

impl Server {
    /// A lobby on this catalog. `seed` seeds the first match; the caller
    /// picks it, so a test can pick a known one and the client a fresh one.
    pub fn new(catalog: Catalog, code: impl Into<String>, seed: u64) -> Server {
        let enabled = catalog
            .challenges()
            .map(|(lib, c)| ChallengeRef { game: lib.game.id.0.clone(), challenge: c.id.0.clone() })
            .collect();
        Server {
            catalog,
            code: code.into(),
            settings: LobbySettings::default(),
            enabled,
            seats: Vec::new(),
            unnamed: BTreeSet::new(),
            next_conn: 1,
            seed,
            live: None,
            rendezvous: None,
            probes: BTreeMap::new(),
        }
    }

    /// Where head-to-head probes go (ADR 0036): `host:port`, or `:port` for
    /// this connection's own host. Without one, no bout is dealable.
    pub fn with_rendezvous(mut self, addr: impl Into<String>) -> Server {
        self.rendezvous = Some(addr.into());
        self
    }

    pub fn library_hash(&self) -> String {
        format!("{:016x}", self.catalog.hash())
    }

    /// A new connection. It is nobody until it says hello.
    pub fn connect(&mut self) -> ConnId {
        let id = ConnId(self.next_conn);
        self.next_conn += 1;
        self.unnamed.insert(id);
        id
    }

    /// A connection went away. A player leaving mid-match ends the match: a
    /// run with nobody to report it cannot be judged. Leaving one already
    /// over disturbs nobody, and once nobody is left on a score card the
    /// lobby is free, since only they could have gone back to it.
    pub fn disconnect(&mut self, conn: ConnId) -> Outbox {
        self.unnamed.remove(&conn);
        let Some(i) = self.seats.iter().position(|s| s.conn == conn) else {
            return Vec::new();
        };
        let seat = self.seats.remove(i);
        let mut out = Vec::new();
        if self.live.as_ref().is_some_and(|l| !l.finished()) {
            out.extend(self.fail(format!("{} left the match", seat.name)));
        }
        if let Some(live) = self.live.as_mut() {
            live.table.remove(&conn);
            if live.table.is_empty() {
                self.live = None;
            }
        }
        out.extend(self.broadcast(ToClient::Lobby(self.lobby_state())));
        out
    }

    pub fn handle(&mut self, conn: ConnId, msg: ToServer) -> Outbox {
        if let ToServer::Hello { name, library_hash, .. } = msg {
            return self.hello(conn, name, library_hash);
        }
        let Some(seat) = self.seats.iter().position(|s| s.conn == conn) else {
            return vec![(conn, ToClient::Refused { reason: "say hello first".into() })];
        };
        let is_host = seat == 0;
        match msg {
            ToServer::Hello { .. } => unreachable!("handled above"),
            ToServer::Settings(settings) if is_host => {
                self.settings = settings;
                self.broadcast(ToClient::Lobby(self.lobby_state()))
            }
            ToServer::Enable { challenges } if is_host => {
                self.enabled = challenges.into_iter().collect();
                self.broadcast(ToClient::Lobby(self.lobby_state()))
            }
            ToServer::Files(report) => {
                self.seats[seat].files = Some(report);
                self.broadcast(ToClient::Lobby(self.lobby_state()))
            }
            ToServer::Start if is_host => self.start(conn),
            ToServer::Prepared { round, artifact } => {
                let player = self.seats[seat].id.clone();
                self.prepared(conn, player, round, artifact)
            }
            ToServer::Unprepared { round, reason } => {
                let player = self.seats[seat].id.clone();
                self.unprepared(conn, player, round, reason)
            }
            ToServer::Frame { round, frame, events } => {
                let player = self.seats[seat].id.clone();
                self.frame(conn, player, round, frame, events)
            }
            ToServer::Finished { round, outcome } => {
                let player = self.seats[seat].id.clone();
                self.finished(conn, player, round, outcome)
            }
            ToServer::Forfeit { round, frame } => {
                let player = self.seats[seat].id.clone();
                self.forfeit(conn, player, round, frame)
            }
            ToServer::Back { round, player: target } => {
                let player = self.seats[seat].id.clone();
                self.back(conn, player, round, PlayerId(target))
            }
            ToServer::Endpoint { round, token, local } => {
                let player = self.seats[seat].id.clone();
                self.endpoint(player, round, token, local)
            }
            ToServer::PeerLost { round, frame } => {
                let player = self.seats[seat].id.clone();
                self.peer_lost(conn, player, round, frame)
            }
            ToServer::Watch { round, player: target } => {
                let player = self.seats[seat].id.clone();
                self.watch(conn, player, round, target.map(PlayerId))
            }
            ToServer::Thumbs { round, players } => {
                let player = self.seats[seat].id.clone();
                self.thumbs(conn, player, round, players.into_iter().map(PlayerId).collect())
            }
            ToServer::Video { round, frame, key, width, height, aspect, data, thumb } => {
                let player = self.seats[seat].id.clone();
                let msg = |player| ToClient::Video {
                    round,
                    player,
                    frame,
                    key,
                    width,
                    height,
                    aspect,
                    data: data.clone(),
                    thumb,
                };
                if thumb {
                    self.relay_thumb(&player, round, msg)
                } else {
                    self.relay(&player, round, msg)
                }
            }
            ToServer::Audio { round, frame, rate, channels, data } => {
                let player = self.seats[seat].id.clone();
                self.relay(&player, round, |player| ToClient::Audio {
                    round,
                    player,
                    frame,
                    rate,
                    channels,
                    data: data.clone(),
                })
            }
            ToServer::EndMatch if is_host => self.end_match(),
            ToServer::PlayAgain if is_host => self.play_again(conn),
            ToServer::BackToLobby if is_host => {
                self.live = None;
                self.broadcast(ToClient::Lobby(self.lobby_state()))
            }
            ToServer::Settings(_)
            | ToServer::Enable { .. }
            | ToServer::Start
            | ToServer::EndMatch
            | ToServer::PlayAgain
            | ToServer::BackToLobby => {
                vec![(conn, ToClient::Refused { reason: "only the host may do that".into() })]
            }
        }
    }

    // ----------------------------------------------------------------- lobby

    fn hello(&mut self, conn: ConnId, name: String, library_hash: String) -> Outbox {
        if !self.unnamed.remove(&conn) {
            return vec![(conn, ToClient::Refused { reason: "already said hello".into() })];
        }
        if library_hash != self.library_hash() {
            return vec![(
                conn,
                ToClient::Refused {
                    reason: format!(
                        "this lobby plays library {}, you hold {library_hash}",
                        self.library_hash()
                    ),
                },
            )];
        }
        if self.live.as_ref().is_some_and(|l| !l.finished()) {
            return vec![(conn, ToClient::Refused { reason: "a match is in progress".into() })];
        }
        // Ids are names made unique: "Chris", then "Chris (2)".
        let mut id = name.clone();
        let mut n = 1;
        while self.seats.iter().any(|s| s.id.0 == id) {
            n += 1;
            id = format!("{name} ({n})");
        }
        self.seats.push(Seat { conn, id: PlayerId(id.clone()), name, files: None });
        let lobby = self.lobby_state();
        let mut out = vec![(
            conn,
            ToClient::Welcome { player: id, lobby: lobby.clone(), rendezvous: self.rendezvous.clone() },
        )];
        for s in &self.seats {
            if s.conn != conn {
                out.push((s.conn, ToClient::Lobby(lobby.clone())));
            }
        }
        out
    }

    /// Whether the settings, and for a bout the lobby, admit `c`: the mode;
    /// the custom toggle; and for a head-to-head challenge the toggle, a
    /// rendezvous to broker it with, and enough players to fill its slots
    /// (ADR 0036).
    fn admits(&self, c: &Challenge) -> bool {
        let mode: challenge_core::Mode = self.settings.mode.into();
        let bout_ok = !c.is_head_to_head()
            || (self.settings.allow_head_to_head
                && self.rendezvous.is_some()
                && self.seats.len() >= usize::from(c.slots));
        mode.admits(c.kind) && (self.settings.allow_custom || !c.is_custom()) && bout_ok
    }

    /// Games some player cannot run.
    fn blocked_games(&self) -> BTreeSet<String> {
        self.seats
            .iter()
            .filter_map(|s| s.files.as_ref())
            .flat_map(|f| f.missing.iter().map(|m| m.game.clone()))
            .collect()
    }

    /// What a match started now would deal from, as library entries.
    fn pool(&self) -> Vec<(&Library, &Challenge)> {
        let blocked = self.blocked_games();
        let mut pool = Vec::new();
        for r in &self.enabled {
            let Some(lib) = self.catalog.game(&r.game) else { continue };
            let Some(c) = lib.challenge(&r.challenge) else { continue };
            if self.admits(c) && !blocked.contains(&r.game) {
                pool.push((lib, c));
            }
        }
        pool
    }

    /// Why no match could be made now, whatever is at the table: what Start
    /// and Play again both refuse on.
    fn unready(&self) -> Vec<String> {
        let mut blocking = Vec::new();
        if self.seats.is_empty() {
            blocking.push("nobody is in the lobby".to_string());
        }
        for s in &self.seats {
            if s.files.is_none() {
                blocking.push(format!("{} has not checked their files yet", s.name));
            }
        }
        if self.pool().is_empty() {
            blocking.push(
                "no enabled challenge fits the settings and everyone's files".to_string(),
            );
        }
        blocking
    }

    pub fn lobby_state(&self) -> LobbyState {
        let dealable: Vec<ChallengeRef> = self
            .pool()
            .into_iter()
            .map(|(lib, c)| ChallengeRef { game: lib.game.id.0.clone(), challenge: c.id.0.clone() })
            .collect();
        let mut blocking = self.unready();
        match &self.live {
            Some(l) if l.finished() => {
                blocking.push("the last match's score card is still up".to_string());
            }
            Some(_) => blocking.push("a match is in progress".to_string()),
            None => {}
        }
        LobbyState {
            code: self.code.clone(),
            settings: self.settings.clone(),
            players: self
                .seats
                .iter()
                .enumerate()
                .map(|(i, s)| LobbyPlayer {
                    id: s.id.0.clone(),
                    name: s.name.clone(),
                    host: i == 0,
                    files: s.files.clone(),
                })
                .collect(),
            enabled: self.enabled.iter().cloned().collect(),
            dealable,
            can_start: blocking.is_empty(),
            blocking,
            in_match: self.live.is_some(),
        }
    }

    // ----------------------------------------------------------------- match

    fn start(&mut self, host: ConnId) -> Outbox {
        let lobby = self.lobby_state();
        if !lobby.can_start {
            return vec![(host, ToClient::Refused { reason: lobby.blocking.join("; ") })];
        }
        match self.new_match() {
            Ok(m) => {
                let table = self.seats.iter().map(|s| s.conn).collect();
                self.live = Some(Live { m, round: None, table });
                self.deal_next()
            }
            Err(e) => vec![(host, ToClient::Refused { reason: e })],
        }
    }

    fn new_match(&mut self) -> Result<Match, String> {
        let default_seconds = self.settings.default_limit_seconds;
        if default_seconds <= 0.0 || default_seconds.is_nan() {
            return Err("the default limit must be more than zero seconds".into());
        }
        // Every entry carries its own limit in its game's frames, because a
        // match crosses framerates and `Match` counts frames; the match-level
        // default is a stand-in that no entry falls back to.
        let pool: Vec<Entry> = self
            .pool()
            .into_iter()
            .map(|(lib, c)| Entry {
                spec: c.spec(&lib.game.id),
                limit: Some(
                    c.limit_frames
                        .unwrap_or_else(|| lib.game.framerate.frames(default_seconds).max(1)),
                ),
            })
            .collect();
        let players: Vec<PlayerId> = self.seats.iter().map(|s| s.id.clone()).collect();
        let settings = Settings {
            mode: self.settings.mode.into(),
            format: self.settings.format.into(),
            default_limit: (default_seconds * 60.0).round().max(1.0) as u64,
        };
        let seed = self.seed;
        self.seed = step(seed);
        Match::new(pool, players, settings, seed).map_err(|e| e.to_string())
    }

    fn deal_next(&mut self) -> Outbox {
        let Some(live) = self.live.as_mut() else { return Vec::new() };
        match live.m.deal() {
            None => {
                live.round = None;
                let standings = standings_of(&live.m);
                self.broadcast(ToClient::MatchOver { standings })
            }
            Some(deal) => {
                let mut seats = BTreeMap::new();
                let bout = if deal.is_bout() {
                    match Bout::new(deal.challenge.clone(), Some(deal.limit)) {
                        Ok(b) => Some(b),
                        Err(e) => return self.fail(e.to_string()),
                    }
                } else {
                    None
                };
                for p in &deal.slots {
                    let run = if bout.is_some() {
                        None
                    } else {
                        match Run::new(deal.challenge.clone()) {
                            Ok(r) => Some(r.with_limit(deal.limit)),
                            Err(e) => return self.fail(e.to_string()),
                        }
                    };
                    seats.insert(
                        p.clone(),
                        SeatRun {
                            run,
                            artifact: None,
                            verdict: None,
                            claimed: None,
                            lost: None,
                            endpoint: Endpoint::default(),
                        },
                    );
                }
                let players: Vec<String> = deal.slots.iter().map(|p| p.0.clone()).collect();
                let msg = ToClient::Dealt {
                    round: deal.index,
                    challenge: ChallengeRef {
                        game: deal.challenge.game.0.clone(),
                        challenge: deal.challenge.id.0.clone(),
                    },
                    kind: deal.challenge.kind.into(),
                    seed: deal.seed,
                    limit_frames: deal.limit,
                    players,
                    backing_seconds: bout.as_ref().map(|_| BACKING_SECONDS),
                };
                live.round = Some(RoundState {
                    deal,
                    seats,
                    bout,
                    started: false,
                    watching: BTreeMap::new(),
                    thumbs: BTreeMap::new(),
                    dealt_at: None,
                    peers_sent: false,
                    lost_at: None,
                });
                self.probes.clear();
                self.broadcast(msg)
            }
        }
    }

    /// The pending round's slot for `player`, if seated in it.
    fn slot_of(rs: &RoundState, player: &PlayerId) -> Option<Slot> {
        rs.deal.slots.iter().position(|p| p == player).map(|i| Slot(i as u8))
    }

    // ------------------------------------------------------------ head-to-head

    /// A player not seated in the bout backs `target`, who is (ADR 0036).
    fn back(&mut self, conn: ConnId, player: PlayerId, round: u64, target: PlayerId) -> Outbox {
        let Some(live) = self.live.as_mut() else { return Vec::new() };
        let pending = live.round.as_ref().is_some_and(|r| r.deal.index == round && !r.started);
        if !pending {
            return vec![(conn, ToClient::Refused { reason: "the round is not open for backing".into() })];
        }
        if let Err(e) = live.m.back(player, target) {
            return vec![(conn, ToClient::Refused { reason: e.to_string() })];
        }
        let mut out = self.sides();
        out.extend(self.maybe_go());
        out
    }

    fn sides(&self) -> Outbox {
        let Some(live) = self.live.as_ref() else { return Vec::new() };
        let Some(rs) = live.round.as_ref() else { return Vec::new() };
        let backing = live.m.backing().iter().map(|(b, t)| (b.0.clone(), t.0.clone())).collect();
        self.broadcast(ToClient::Sides { round: rs.deal.index, backing })
    }

    /// A seated player says where its machine is: the token its probes
    /// carry and its own address.
    fn endpoint(&mut self, player: PlayerId, round: u64, token: u64, local: String) -> Outbox {
        let public = self.probes.remove(&token);
        let Some(seat) = self.seat_run(round, &player) else { return Vec::new() };
        seat.endpoint.token = Some(token);
        seat.endpoint.local = Some(local);
        if public.is_some() {
            seat.endpoint.public = public;
        }
        self.maybe_peers()
    }

    /// A probe carrying `token` reached the server's UDP side from `addr`.
    /// Not a protocol message: whoever owns the socket calls this.
    pub fn observed(&mut self, token: u64, addr: String) -> Outbox {
        let seat = self
            .live
            .as_mut()
            .and_then(|l| l.round.as_mut())
            .and_then(|rs| rs.seats.values_mut().find(|s| s.endpoint.token == Some(token)));
        match seat {
            Some(seat) => {
                seat.endpoint.public = Some(addr);
                self.maybe_peers()
            }
            None => {
                // Keep a few: the `Endpoint` naming this token may be on its way.
                if self.probes.len() < 64 {
                    self.probes.insert(token, addr);
                }
                Vec::new()
            }
        }
    }

    /// Once every seated player's machine is known, every seated player
    /// hears where the others are.
    fn maybe_peers(&mut self) -> Outbox {
        let Some(rs) = self.live.as_mut().and_then(|l| l.round.as_mut()) else { return Vec::new() };
        if rs.bout.is_none() || rs.peers_sent {
            return Vec::new();
        }
        let known = rs
            .seats
            .values()
            .all(|s| s.endpoint.local.is_some() && s.endpoint.public.is_some());
        if !known {
            return Vec::new();
        }
        rs.peers_sent = true;
        let round = rs.deal.index;
        let peers: Vec<Peer> = rs
            .deal
            .slots
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let e = &rs.seats[p].endpoint;
                let mut candidates = vec![e.local.clone().unwrap()];
                let public = e.public.clone().unwrap();
                if !candidates.contains(&public) {
                    candidates.push(public);
                }
                Peer { player: p.0.clone(), slot: i as u8, candidates }
            })
            .collect();
        let msg = ToClient::Peers { round, peers };
        self.seats
            .iter()
            .filter(|s| rs_seated(self.live.as_ref().unwrap().round.as_ref().unwrap(), &s.id))
            .map(|s| (s.conn, msg.clone()))
            .collect()
    }

    /// `Go`, once every seated player is prepared on one artifact and, for
    /// a bout, every backer has picked or the window has closed.
    fn maybe_go(&mut self) -> Outbox {
        let Some(live) = self.live.as_mut() else { return Vec::new() };
        let Some(rs) = live.round.as_mut() else { return Vec::new() };
        if rs.started || rs.seats.values().any(|s| s.artifact.is_none()) {
            return Vec::new();
        }
        if rs.bout.is_some() && !live.m.unbacked().is_empty() {
            return Vec::new();
        }
        rs.started = true;
        let round = rs.deal.index;
        match rs.bout.as_mut() {
            Some(b) => b.enter(),
            None => {
                for s in rs.seats.values_mut() {
                    if let Some(run) = s.run.as_mut() {
                        run.enter();
                    }
                }
            }
        }
        self.broadcast(ToClient::Go { round })
    }

    /// The clock, from whoever drives this server: closes a bout's backing
    /// window and calls a bout whose link died. Nothing a player did is
    /// measured against it (ADR 0005).
    pub fn tick(&mut self, now: Instant) -> Outbox {
        let Some(live) = self.live.as_mut() else { return Vec::new() };
        let Some(rs) = live.round.as_mut() else { return Vec::new() };
        if rs.bout.is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        let dealt_at = *rs.dealt_at.get_or_insert(now);
        if !rs.started
            && now.duration_since(dealt_at) >= Duration::from_secs(u64::from(BACKING_SECONDS))
            && !live.m.unbacked().is_empty()
        {
            let seed = step(self.seed ^ rs.deal.index);
            live.m.assign_backers(seed);
            out.extend(self.sides());
            out.extend(self.maybe_go());
        }
        let Some(live) = self.live.as_mut() else { return out };
        let Some(rs) = live.round.as_mut() else { return out };
        if rs.seats.values().any(|s| s.lost.is_some() && s.verdict.is_none()) {
            let lost_at = *rs.lost_at.get_or_insert(now);
            if now.duration_since(lost_at) >= LOST_GRACE {
                let verdicts = rs.bout.as_mut().map(|b| b.abandon()).unwrap_or_default();
                let round = rs.deal.index;
                out.extend(self.settle(round, verdicts));
            }
        }
        out
    }

    /// A seated player's link to the other slots died on its side.
    fn peer_lost(&mut self, conn: ConnId, player: PlayerId, round: u64, frame: u64) -> Outbox {
        let Some(rs) = self.live.as_mut().and_then(|l| l.round.as_mut()) else { return Vec::new() };
        if rs.deal.index != round || !rs.started {
            return vec![(conn, ToClient::Refused { reason: "no round is live".into() })];
        }
        let Some(slot) = Server::slot_of(rs, &player) else { return Vec::new() };
        let seat = rs.seats.get_mut(&player).expect("seated");
        if seat.verdict.is_some() {
            return Vec::new();
        }
        seat.lost = Some(frame);
        let verdicts = rs.bout.as_mut().map(|b| b.lost(slot, frame)).unwrap_or_default();
        self.settle(round, verdicts)
    }

    /// Verdicts a bout settled: each is its player's, announced, and the
    /// round recorded once every slot has one.
    fn settle(&mut self, round: u64, verdicts: Vec<(Slot, RunResult)>) -> Outbox {
        let mut out = Vec::new();
        for (slot, result) in verdicts {
            let Some(rs) = self.live.as_mut().and_then(|l| l.round.as_mut()) else { break };
            let player = rs.deal.slots[usize::from(slot.0)].clone();
            if let Some(seat) = rs.seats.get_mut(&player) {
                seat.verdict = Some(result.clone());
            }
            out.extend(self.broadcast(ToClient::PlayerFinished {
                round,
                player: player.0,
                outcome: (&result).into(),
            }));
        }
        out.extend(self.maybe_record());
        out
    }

    /// The seat's run in the pending round, if the message is for that round
    /// and the player is in it. An eliminated player prepares and spectates;
    /// their messages are ignored rather than refused.
    fn seat_run(&mut self, round: u64, player: &PlayerId) -> Option<&mut SeatRun> {
        let rs = self.live.as_mut()?.round.as_mut()?;
        if rs.deal.index != round {
            return None;
        }
        rs.seats.get_mut(player)
    }

    fn prepared(&mut self, conn: ConnId, player: PlayerId, round: u64, artifact: String) -> Outbox {
        let Some(seat) = self.seat_run(round, &player) else { return Vec::new() };
        seat.artifact = Some(artifact);
        let rs = self.live.as_ref().unwrap().round.as_ref().unwrap();
        if rs.started || rs.seats.values().any(|s| s.artifact.is_none()) {
            return Vec::new();
        }
        // Everyone is prepared. Every player derived the same bytes, or the
        // round cannot be compared (ADR 0013).
        let mut hashes: Vec<(&PlayerId, &str)> =
            rs.seats.iter().map(|(p, s)| (p, s.artifact.as_deref().unwrap())).collect();
        hashes.sort();
        if hashes.windows(2).any(|w| w[0].1 != w[1].1) {
            let list: Vec<String> = hashes.iter().map(|(p, h)| format!("{p}: {h}")).collect();
            return self.fail(format!("artifact hashes differ: {}", list.join(", ")));
        }
        let _ = conn;
        self.maybe_go()
    }

    /// A player cannot run the round. Nobody plays it, its challenge is
    /// withdrawn, and the next one is dealt (ADR 0029). Only before `Go`: a
    /// run under way is given up with `Forfeit`, not voided.
    fn unprepared(&mut self, conn: ConnId, player: PlayerId, round: u64, reason: String) -> Outbox {
        let started = self
            .live
            .as_ref()
            .and_then(|l| l.round.as_ref())
            .is_some_and(|r| r.deal.index == round && r.started);
        if started {
            return vec![(conn, ToClient::Refused { reason: "the round is under way".into() })];
        }
        if self.seat_run(round, &player).is_none() {
            return Vec::new();
        }
        let live = self.live.as_mut().expect("a seat run means a live match");
        live.round = None;
        live.m.void();
        let mut out = self.broadcast(ToClient::Voided { round, player: player.0, reason });
        out.extend(self.deal_next());
        out
    }

    fn frame(
        &mut self,
        conn: ConnId,
        player: PlayerId,
        round: u64,
        frame: u64,
        events: Vec<protocol::Event>,
    ) -> Outbox {
        let started = self
            .live
            .as_ref()
            .and_then(|l| l.round.as_ref())
            .is_some_and(|r| r.deal.index == round && r.started);
        if !started {
            return vec![(conn, ToClient::Refused { reason: "no round is live".into() })];
        }
        let rs = self.live.as_mut().unwrap().round.as_mut().unwrap();
        let Some(slot) = Server::slot_of(rs, &player) else { return Vec::new() };
        let seat = rs.seats.get_mut(&player).expect("seated");
        if seat.verdict.is_some() || seat.lost.is_some() {
            return Vec::new();
        }
        let session: Vec<_> = events.iter().map(|e| e.to_session()).collect();
        // A bout settles in frame order across every slot (ADR 0036); a run
        // of one settles itself.
        let (settled, attempt, clock) = match (rs.bout.as_mut(), seat.run.as_mut()) {
            (Some(b), _) => {
                let v = b.observe(slot, frame, &session);
                let run = b.run(slot);
                (v, run.attempts(), run.frames())
            }
            (None, Some(run)) => {
                let next = run.observe(frame, &session);
                if matches!(next, Next::Reenter) {
                    run.enter();
                }
                let settled = match next {
                    Next::Finished(r) => vec![(slot, r)],
                    _ => Vec::new(),
                };
                (settled, run.attempts(), run.frames())
            }
            (None, None) => unreachable!("a seat without a bout has a run"),
        };
        // Whoever watches hears where the run stands, in a few bytes that
        // reach them when the picture cannot (ADR 0034).
        let mut out = self.relay(&player, round, |p| ToClient::Progress {
            round,
            player: p,
            frame,
            attempt,
            clock,
            events: events.clone(),
        });
        if !settled.is_empty() {
            out.extend(self.settle(round, settled));
        }
        out
    }

    fn forfeit(&mut self, conn: ConnId, player: PlayerId, round: u64, frame: u64) -> Outbox {
        let started = self
            .live
            .as_ref()
            .and_then(|l| l.round.as_ref())
            .is_some_and(|r| r.deal.index == round && r.started);
        if !started {
            return vec![(conn, ToClient::Refused { reason: "no round is live".into() })];
        }
        let rs = self.live.as_mut().unwrap().round.as_mut().unwrap();
        let Some(slot) = Server::slot_of(rs, &player) else { return Vec::new() };
        let seat = rs.seats.get_mut(&player).expect("seated");
        if seat.verdict.is_some() {
            return Vec::new();
        }
        let settled = match (rs.bout.as_mut(), seat.run.as_mut()) {
            (Some(b), _) => b.forfeit(slot, frame),
            (None, Some(run)) => {
                // The frames up to the give-up count; the limit may have beaten it.
                let result = match run.observe(frame, &[]) {
                    Next::Finished(r) => r,
                    Next::Reenter => unreachable!("no event, no re-entry"),
                    Next::Continue => run.forfeit(),
                };
                vec![(slot, result)]
            }
            (None, None) => unreachable!("a seat without a bout has a run"),
        };
        self.settle(round, settled)
    }

    fn finished(&mut self, conn: ConnId, player: PlayerId, round: u64, claimed: RunOutcome) -> Outbox {
        let _ = conn;
        let bout = self.live.as_ref().and_then(|l| l.round.as_ref()).is_some_and(|r| r.bout.is_some());
        let Some(seat) = self.seat_run(round, &player) else { return Vec::new() };
        let verdict = seat.verdict.as_ref().map(RunOutcome::from);
        seat.claimed = Some(claimed.clone());
        match verdict {
            // In a bout the beaten client's claim can beat the winner's
            // completing frame to the server by a socket; the claims are
            // compared once every slot has a verdict (ADR 0036).
            None if bout => Vec::new(),
            None => self.fail(format!(
                "{player} reports a finished run the server has not seen finish: {claimed:?}"
            )),
            Some(v) if v != claimed => self.fail(format!(
                "{player}: client and server disagree on the run: client {claimed:?}, server {v:?}"
            )),
            Some(_) => self.maybe_record(),
        }
    }

    /// `player` wants to watch `target` this round, or nobody. Only a player
    /// whose run is over, or who is not in the round, may watch: one still
    /// playing has their own game in the main view, and the sidebar for the
    /// rest (ADR 0038). The targets whose count changed are told how many
    /// watch them now.
    fn watch(&mut self, conn: ConnId, player: PlayerId, round: u64, target: Option<PlayerId>) -> Outbox {
        let Some(rs) = self.live.as_mut().and_then(|l| l.round.as_mut()) else { return Vec::new() };
        if rs.deal.index != round {
            // A round that ended while the request was on its way.
            return Vec::new();
        }
        let refuse = |reason: &str| vec![(conn, ToClient::Refused { reason: reason.into() })];
        if rs.seats.get(&player).is_some_and(|s| s.verdict.is_none()) {
            return refuse("you are still playing this round");
        }
        if let Some(t) = &target {
            if *t == player {
                return refuse("you cannot watch yourself");
            }
            if !rs.seats.contains_key(t) {
                return refuse(&format!("{t} is not playing this round"));
            }
        }
        let before = rs.watching.remove(&player);
        if let Some(t) = target.clone() {
            rs.watching.insert(player, t);
        }
        let mut changed: Vec<PlayerId> = Vec::new();
        for t in before.into_iter().chain(target) {
            if !changed.contains(&t) {
                changed.push(t);
            }
        }
        let mut out = Vec::new();
        for t in changed {
            let (watchers, thumbs) = audience(rs, &t);
            if let Some(seat) = self.seats.iter().find(|s| s.id == t) {
                out.push((seat.conn, ToClient::Watched { round, watchers, thumbs }));
            }
        }
        out
    }

    /// `player` shows `targets` in their sidebar this round, replacing whom
    /// they showed (ADR 0038). Anyone in the match may, playing or not, and
    /// for anyone in the round: seeing ahead is allowed. The targets whose
    /// count changed are told how many show them now.
    fn thumbs(&mut self, conn: ConnId, player: PlayerId, round: u64, mut targets: Vec<PlayerId>) -> Outbox {
        let Some(rs) = self.live.as_mut().and_then(|l| l.round.as_mut()) else { return Vec::new() };
        if rs.deal.index != round {
            return Vec::new();
        }
        let refuse = |reason: &str| vec![(conn, ToClient::Refused { reason: reason.into() })];
        targets.dedup();
        if targets.len() > MAX_THUMBS {
            return refuse(&format!("at most {MAX_THUMBS} thumbnails"));
        }
        for t in &targets {
            if *t == player {
                return refuse("you cannot show yourself");
            }
            if !rs.seats.contains_key(t) {
                return refuse(&format!("{t} is not playing this round"));
            }
        }
        let before = rs.thumbs.remove(&player).unwrap_or_default();
        let changed: Vec<PlayerId> = before
            .iter()
            .filter(|t| !targets.contains(t))
            .chain(targets.iter().filter(|t| !before.contains(t)))
            .cloned()
            .collect();
        if !targets.is_empty() {
            rs.thumbs.insert(player, targets);
        }
        let mut out = Vec::new();
        for t in changed {
            let (watchers, thumbs) = audience(rs, &t);
            if let Some(seat) = self.seats.iter().find(|s| s.id == t) {
                out.push((seat.conn, ToClient::Watched { round, watchers, thumbs }));
            }
        }
        out
    }

    /// One of `player`'s stream messages, to everyone watching them this
    /// round. Never refused: a chunk in flight when the round ends is not an
    /// error, it is just nobody's any more.
    fn relay(&self, player: &PlayerId, round: u64, msg: impl Fn(String) -> ToClient) -> Outbox {
        let Some(rs) = self.live.as_ref().and_then(|l| l.round.as_ref()) else { return Vec::new() };
        if rs.deal.index != round {
            return Vec::new();
        }
        rs.watching
            .iter()
            .filter(|(_, t)| *t == player)
            .filter_map(|(w, _)| self.seats.iter().find(|s| s.id == *w))
            .map(|s| (s.conn, msg(player.0.clone())))
            .collect()
    }

    /// One of `player`'s thumbnail pictures, to everyone showing them this
    /// round (ADR 0038). Never refused, as [`Server::relay`].
    fn relay_thumb(&self, player: &PlayerId, round: u64, msg: impl Fn(String) -> ToClient) -> Outbox {
        let Some(rs) = self.live.as_ref().and_then(|l| l.round.as_ref()) else { return Vec::new() };
        if rs.deal.index != round {
            return Vec::new();
        }
        rs.thumbs
            .iter()
            .filter(|(_, ts)| ts.contains(player))
            .filter_map(|(w, _)| self.seats.iter().find(|s| s.id == *w))
            .map(|s| (s.conn, msg(player.0.clone())))
            .collect()
    }

    /// Records the round once every seated player has both verdicts in — or,
    /// for a slot whose link died, the server's alone.
    fn maybe_record(&mut self) -> Outbox {
        let Some(rs) = self.live.as_ref().and_then(|l| l.round.as_ref()) else { return Vec::new() };
        if rs.seats.values().any(|s| s.verdict.is_none() || (s.claimed.is_none() && s.lost.is_none())) {
            return Vec::new();
        }
        // A bout's claims were held until now (`finished`).
        let mismatch = rs.bout.as_ref().and_then(|_| {
            rs.seats.iter().find_map(|(p, s)| {
                let v = RunOutcome::from(s.verdict.as_ref().unwrap());
                s.claimed.as_ref().filter(|c| **c != v).map(|c| {
                    format!("{p}: client and server disagree on the run: client {c:?}, server {v:?}")
                })
            })
        });
        if let Some(why) = mismatch {
            return self.fail(why);
        }
        let live = self.live.as_mut().unwrap();
        let rs = live.round.take().unwrap();
        let round = rs.deal.index;
        let results: Vec<(PlayerId, RunResult)> =
            rs.seats.into_iter().map(|(p, s)| (p, s.verdict.unwrap())).collect();
        let recorded = live.m.record(results);
        let msg = ToClient::RoundResult {
            round,
            entries: recorded.entries.iter().map(Into::into).collect(),
            eliminated: recorded.eliminated.iter().map(|p| p.0.clone()).collect(),
            standings: standings_of(&live.m),
        };
        let mut out = self.broadcast(msg);
        out.extend(self.deal_next());
        out
    }

    fn end_match(&mut self) -> Outbox {
        let Some(live) = self.live.as_mut() else { return Vec::new() };
        live.m.end();
        live.round = None;
        let standings = standings_of(&live.m);
        self.broadcast(ToClient::MatchOver { standings })
    }

    fn play_again(&mut self, host: ConnId) -> Outbox {
        let over = self.live.as_ref().is_some_and(|l| l.m.is_over());
        if !over {
            return vec![(host, ToClient::Refused { reason: "the match is not over".into() })];
        }
        let unready = self.unready();
        if !unready.is_empty() {
            return vec![(host, ToClient::Refused { reason: unready.join("; ") })];
        }
        match self.new_match() {
            Ok(m) => {
                let table = self.seats.iter().map(|s| s.conn).collect();
                self.live = Some(Live { m, round: None, table });
                self.deal_next()
            }
            Err(e) => vec![(host, ToClient::Refused { reason: e })],
        }
    }

    /// The match cannot go on. Says why, then ends it where it stands.
    fn fail(&mut self, message: String) -> Outbox {
        let mut out = self.broadcast(ToClient::Error { message });
        out.extend(self.end_match());
        out
    }

    fn broadcast(&self, msg: ToClient) -> Outbox {
        self.seats.iter().map(|s| (s.conn, msg.clone())).collect()
    }
}

fn standings_of(m: &Match) -> Vec<protocol::Standing> {
    m.standings().iter().map(Into::into).collect()
}

/// How many watch `player` full-size this round, and how many show them in
/// their sidebar.
fn audience(rs: &RoundState, player: &PlayerId) -> (u32, u32) {
    let watchers = rs.watching.values().filter(|w| *w == player).count() as u32;
    let thumbs = rs.thumbs.values().filter(|ts| ts.contains(player)).count() as u32;
    (watchers, thumbs)
}

fn rs_seated(rs: &RoundState, player: &PlayerId) -> bool {
    rs.seats.contains_key(player)
}

/// The next match's seed from this one: SplitMix64's step, written out like
/// the deal's generator because two machines compare what comes of it.
fn step(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests;
