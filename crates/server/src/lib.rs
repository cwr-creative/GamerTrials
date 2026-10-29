//! The server half of a match (ADR 0009): a lobby, then a match dealt, judged
//! and ranked by `challenge-core`, driven entirely by [`protocol`] messages.
//!
//! This is a state machine, not a service. [`Server::handle`] takes one
//! message from one connection and returns the messages to send; whoever
//! owns the sockets delivers them. Solo play drives it over an in-process
//! channel and a networked lobby will drive it over WebSockets (Phase C), and
//! neither changes a rule here, because there is nothing else here.
//!
//! What it never does: run a game, read a ROM, hold a path. Its `Cargo.toml`
//! is the fence (ADR 0024).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use challenge_core::{Deal, Entry, Match, Next, Run, RunResult, Settings};
use challenge_library::{Catalog, Challenge, Library};
use protocol::{
    ChallengeRef, FileReport, LobbyPlayer, LobbySettings, LobbyState, RunOutcome, ToClient,
    ToServer,
};
use session_api::PlayerId;

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

struct SeatRun {
    run: Run,
    artifact: Option<String>,
    /// The server's verdict.
    verdict: Option<RunResult>,
    /// The client's provisional one, to be checked against the verdict.
    claimed: Option<RunOutcome>,
}

struct RoundState {
    deal: Deal,
    seats: BTreeMap<PlayerId, SeatRun>,
    /// `Go` has been sent: runs are live.
    started: bool,
}

struct Live {
    m: Match,
    round: Option<RoundState>,
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
        }
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
    /// run with nobody to report it cannot be judged.
    pub fn disconnect(&mut self, conn: ConnId) -> Outbox {
        self.unnamed.remove(&conn);
        let Some(i) = self.seats.iter().position(|s| s.conn == conn) else {
            return Vec::new();
        };
        let seat = self.seats.remove(i);
        let mut out = Vec::new();
        if self.live.is_some() {
            out.extend(self.fail(format!("{} left the match", seat.name)));
        }
        out.extend(self.broadcast(ToClient::Lobby(self.lobby_state())));
        out
    }

    pub fn handle(&mut self, conn: ConnId, msg: ToServer) -> Outbox {
        if let ToServer::Hello { name, library_hash } = msg {
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
        if self.live.is_some() {
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
        let mut out = vec![(conn, ToClient::Welcome { player: id, lobby: lobby.clone() })];
        for s in &self.seats {
            if s.conn != conn {
                out.push((s.conn, ToClient::Lobby(lobby.clone())));
            }
        }
        out
    }

    fn admits(&self, c: &Challenge) -> bool {
        let mode: challenge_core::Mode = self.settings.mode.into();
        let kind_ok = match mode {
            challenge_core::Mode::TimeTrial => c.kind == session_api::ChallengeKind::TimeTrial,
            challenge_core::Mode::Survival => c.kind == session_api::ChallengeKind::Survival,
            challenge_core::Mode::Both => true,
        };
        kind_ok && (self.settings.allow_custom || !c.is_custom())
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

    pub fn lobby_state(&self) -> LobbyState {
        let dealable: Vec<ChallengeRef> = self
            .pool()
            .into_iter()
            .map(|(lib, c)| ChallengeRef { game: lib.game.id.0.clone(), challenge: c.id.0.clone() })
            .collect();
        let mut blocking = Vec::new();
        if self.seats.is_empty() {
            blocking.push("nobody is in the lobby".to_string());
        }
        for s in &self.seats {
            if s.files.is_none() {
                blocking.push(format!("{} has not checked their files yet", s.name));
            }
        }
        if dealable.is_empty() {
            blocking.push(
                "no enabled challenge fits the settings and everyone's files".to_string(),
            );
        }
        if self.live.is_some() {
            blocking.push("a match is in progress".to_string());
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
                self.live = Some(Live { m, round: None });
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
                for p in live.m.active() {
                    let run = match Run::new(deal.challenge.clone()) {
                        Ok(r) => r.with_limit(deal.limit),
                        Err(e) => return self.fail(e.to_string()),
                    };
                    seats.insert(
                        p.clone(),
                        SeatRun { run, artifact: None, verdict: None, claimed: None },
                    );
                }
                let players: Vec<String> = seats.keys().map(|p| p.0.clone()).collect();
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
                };
                live.round = Some(RoundState { deal, seats, started: false });
                self.broadcast(msg)
            }
        }
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
        let rs = self.live.as_mut().unwrap().round.as_mut().unwrap();
        rs.started = true;
        for s in rs.seats.values_mut() {
            s.run.enter();
        }
        self.broadcast(ToClient::Go { round })
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
        let Some(seat) = self.seat_run(round, &player) else { return Vec::new() };
        if seat.verdict.is_some() {
            return Vec::new();
        }
        let events: Vec<_> = events.iter().map(|e| e.to_session()).collect();
        match seat.run.observe(frame, &events) {
            Next::Continue => Vec::new(),
            Next::Reenter => {
                seat.run.enter();
                Vec::new()
            }
            Next::Finished(result) => {
                seat.verdict = Some(result.clone());
                let mut out = self.broadcast(ToClient::PlayerFinished {
                    round,
                    player: player.0.clone(),
                    outcome: (&result).into(),
                });
                out.extend(self.maybe_record());
                out
            }
        }
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
        let Some(seat) = self.seat_run(round, &player) else { return Vec::new() };
        if seat.verdict.is_some() {
            return Vec::new();
        }
        // The frames up to the give-up count; the limit may have beaten it.
        let result = match seat.run.observe(frame, &[]) {
            Next::Finished(r) => r,
            Next::Reenter => unreachable!("no event, no re-entry"),
            Next::Continue => seat.run.forfeit(),
        };
        seat.verdict = Some(result.clone());
        let mut out = self.broadcast(ToClient::PlayerFinished {
            round,
            player: player.0.clone(),
            outcome: (&result).into(),
        });
        out.extend(self.maybe_record());
        out
    }

    fn finished(&mut self, conn: ConnId, player: PlayerId, round: u64, claimed: RunOutcome) -> Outbox {
        let _ = conn;
        let Some(seat) = self.seat_run(round, &player) else { return Vec::new() };
        let verdict = seat.verdict.as_ref().map(RunOutcome::from);
        seat.claimed = Some(claimed.clone());
        match verdict {
            None => self.fail(format!(
                "{player} reports a finished run the server has not seen finish: {claimed:?}"
            )),
            Some(v) if v != claimed => self.fail(format!(
                "{player}: client and server disagree on the run: client {claimed:?}, server {v:?}"
            )),
            Some(_) => self.maybe_record(),
        }
    }

    /// Records the round once every active player has both verdicts in.
    fn maybe_record(&mut self) -> Outbox {
        let Some(live) = self.live.as_mut() else { return Vec::new() };
        let Some(rs) = live.round.as_ref() else { return Vec::new() };
        if rs.seats.values().any(|s| s.verdict.is_none() || s.claimed.is_none()) {
            return Vec::new();
        }
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
        match self.new_match() {
            Ok(m) => {
                self.live = Some(Live { m, round: None });
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
