//! The server driven the way a client drives it, on scripted telemetry.
//!
//! The client's part here is [`Client::play`]: the loop `app-client` runs on
//! a real runner, on a `FakeRunner` instead. It keeps the one rule the wire
//! depends on: a `Frame` goes to the server for every frame that carried an
//! event and for the frame the client's own judge settled the run on, and
//! nothing else, so the server replays the judgement from exactly those.

use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};

use challenge_core::{Next, Run};
use challenge_library::Catalog;
use protocol::{
    ChallengeRef, Ending, FileReport, Format, LobbySettings, MissingFile, Mode, RunOutcome,
    ToClient, ToServer,
};
use session_api::{ChallengeKind, ChallengeSpec, StageRunner};
use session_fake::{stage_for, Attempt, FakeRunner};

use super::{ConnId, Outbox, Server, BACKING_SECONDS, LOST_GRACE};

fn catalog() -> Catalog {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mut texts = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "toml") {
            texts.push(fs::read_to_string(&path).unwrap());
        }
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    Catalog::parse_all(&refs).unwrap()
}

/// A player's seat at the table: what they received, and how they play.
struct Client {
    conn: ConnId,
    name: String,
    inbox: Vec<ToClient>,
    /// How this player plays any challenge.
    style: fn(&ChallengeSpec) -> Vec<Attempt>,
}

fn ace(c: &ChallengeSpec) -> Vec<Attempt> {
    vec![Attempt::meeting(&c.complete_when, at_least(c, 600))]
}

fn steady(c: &ChallengeSpec) -> Vec<Attempt> {
    match c.kind {
        ChallengeKind::TimeTrial => vec![
            Attempt::new().died(200),
            Attempt::meeting(&c.complete_when, at_least(c, 900)),
        ],
        ChallengeKind::Survival => vec![Attempt::meeting(&c.complete_when, at_least(c, 1200))],
        ChallengeKind::HeadToHead => unreachable!("a bout is not played on a runner of one"),
    }
}

fn reckless(c: &ChallengeSpec) -> Vec<Attempt> {
    match c.kind {
        ChallengeKind::TimeTrial => vec![
            Attempt::new().died(50),
            Attempt::new().died(50),
            Attempt::new().died(50),
            Attempt::new(),
        ],
        ChallengeKind::Survival => vec![Attempt::new().died(50)],
        ChallengeKind::HeadToHead => unreachable!("a bout is not played on a runner of one"),
    }
}

fn at_least(c: &ChallengeSpec, frame: u64) -> u64 {
    c.complete_when.survive_frames.map_or(frame, |n| n.max(frame))
}

struct Table {
    server: Server,
    clients: Vec<Client>,
}

impl Table {
    fn new(seed: u64) -> Table {
        Table { server: Server::new(catalog(), "test", seed), clients: Vec::new() }
    }

    fn deliver(&mut self, out: Outbox) {
        for (conn, msg) in out {
            if let Some(c) = self.clients.iter_mut().find(|c| c.conn == conn) {
                c.inbox.push(msg);
            }
        }
    }

    fn send(&mut self, who: usize, msg: ToServer) {
        let conn = self.clients[who].conn;
        let out = self.server.handle(conn, msg);
        self.deliver(out);
    }

    fn join(&mut self, name: &str, style: fn(&ChallengeSpec) -> Vec<Attempt>) -> usize {
        let conn = self.server.connect();
        self.clients.push(Client { conn, name: name.into(), inbox: Vec::new(), style });
        let who = self.clients.len() - 1;
        let hash = self.server.library_hash();
        self.send(who, ToServer::Hello { name: name.into(), library_hash: hash, lobby: None });
        self.send(who, ToServer::Files(FileReport::default()));
        who
    }

    fn take(&mut self, who: usize) -> Vec<ToClient> {
        std::mem::take(&mut self.clients[who].inbox)
    }

    fn last_dealt(&self, who: usize) -> Option<(u64, ChallengeRef, u64)> {
        self.clients[who].inbox.iter().rev().find_map(|m| match m {
            ToClient::Dealt { round, challenge, limit_frames, .. } => {
                Some((*round, challenge.clone(), *limit_frames))
            }
            _ => None,
        })
    }

    fn spec(&self, r: &ChallengeRef) -> ChallengeSpec {
        let lib = self.server.catalog.game(&r.game).unwrap();
        lib.challenge(&r.challenge).unwrap().spec(&lib.game.id)
    }

    /// Every seated client prepares the pending round on one artifact, which
    /// brings the `Go`; then each plays it through, in turn. Returns the
    /// outcome each client claimed, in seat order.
    fn play_round(&mut self) -> Vec<RunOutcome> {
        let (round, cref, limit) = self.last_dealt(0).expect("a round was dealt");
        let spec = self.spec(&cref);
        for who in 0..self.clients.len() {
            self.send(who, ToServer::Prepared { round, artifact: "abc".into() });
        }
        assert!(
            self.clients[0].inbox.iter().any(|m| matches!(m, ToClient::Go { round: r } if *r == round)),
            "no Go after everyone prepared: {:?}",
            self.clients[0].inbox
        );
        let mut claimed = Vec::new();
        for who in 0..self.clients.len() {
            claimed.push(self.play(who, round, &spec, limit));
        }
        claimed
    }

    /// The client's loop, on a fake runner.
    fn play(&mut self, who: usize, round: u64, spec: &ChallengeSpec, limit: u64) -> RunOutcome {
        let stage = stage_for(spec);
        let mut runner = FakeRunner::new(stage.clone(), (self.clients[who].style)(spec));
        let mut run = Run::new(spec.clone()).unwrap().with_limit(limit);
        runner.enter(&stage).unwrap();
        run.enter();
        loop {
            let events = runner.advance().unwrap().to_vec();
            let frame = runner.frame();
            let next = run.observe(frame, &events);
            if !events.is_empty() || next != Next::Continue {
                let wire = events.iter().map(Into::into).collect();
                self.send(who, ToServer::Frame { round, frame, events: wire });
            }
            match next {
                Next::Continue => {}
                Next::Reenter => {
                    runner.enter(&stage).unwrap();
                    run.enter();
                }
                Next::Finished(result) => {
                    let outcome: RunOutcome = (&result).into();
                    self.send(who, ToServer::Finished { round, outcome: outcome.clone() });
                    return outcome;
                }
            }
        }
    }

    fn settings(&mut self, s: LobbySettings) {
        self.send(0, ToServer::Settings(s));
    }

    fn start(&mut self) {
        self.send(0, ToServer::Start);
    }

    fn lobby(&self, who: usize) -> protocol::LobbyState {
        self.clients[who]
            .inbox
            .iter()
            .rev()
            .find_map(|m| match m {
                ToClient::Lobby(l) | ToClient::Welcome { lobby: l, .. } => Some(l.clone()),
                _ => None,
            })
            .expect("a lobby state")
    }

    fn errors(&self, who: usize) -> Vec<String> {
        self.clients[who]
            .inbox
            .iter()
            .filter_map(|m| match m {
                ToClient::Error { message } => Some(message.clone()),
                _ => None,
            })
            .collect()
    }

    fn match_over(&self, who: usize) -> Option<Vec<protocol::Standing>> {
        self.clients[who].inbox.iter().rev().find_map(|m| match m {
            ToClient::MatchOver { standings } => Some(standings.clone()),
            _ => None,
        })
    }
}

fn r(game: &str, challenge: &str) -> ChallengeRef {
    ChallengeRef { game: game.into(), challenge: challenge.into() }
}

// ------------------------------------------------------------------- lobby

#[test]
fn a_client_holding_another_library_is_refused() {
    let mut t = Table::new(1);
    let conn = t.server.connect();
    let out = t.server.handle(
        conn,
        ToServer::Hello { name: "x".into(), library_hash: "0000000000000000".into(), lobby: None },
    );
    assert!(matches!(&out[..], [(c, ToClient::Refused { reason })] if *c == conn && reason.contains("library")));
}

#[test]
fn the_lobby_blocks_start_until_files_are_reported_and_names_the_reason() {
    let mut t = Table::new(1);
    let conn = t.server.connect();
    t.clients.push(Client { conn, name: "solo".into(), inbox: Vec::new(), style: ace });
    let hash = t.server.library_hash();
    t.send(0, ToServer::Hello { name: "solo".into(), library_hash: hash, lobby: None });
    let lobby = t.lobby(0);
    assert!(!lobby.can_start);
    assert!(lobby.blocking.iter().any(|b| b.contains("files")), "{:?}", lobby.blocking);
    t.send(0, ToServer::Files(FileReport::default()));
    assert!(t.lobby(0).can_start);
}

#[test]
fn a_missing_dump_takes_its_game_out_of_the_pool() {
    let mut t = Table::new(1);
    t.join("solo", ace);
    let before = t.lobby(0).dealable;
    assert!(before.iter().any(|c| c.game == "doom"));
    t.send(
        0,
        ToServer::Files(FileReport {
            missing: vec![MissingFile {
                game: "doom".into(),
                rom: "base".into(),
                name: "doom-shareware-e1m1.zip".into(),
                sha1: None,
                problem: "not found".into(),
            }],
        }),
    );
    let after = t.lobby(0).dealable;
    assert!(after.iter().all(|c| c.game != "doom"), "{after:?}");
    assert!(after.len() < before.len());
}

#[test]
fn the_mode_and_the_custom_toggle_filter_the_pool() {
    let mut t = Table::new(1);
    t.join("solo", ace);
    t.settings(LobbySettings { mode: Mode::TimeTrial, allow_custom: false, ..Default::default() });
    let dealable = t.lobby(0).dealable;
    assert!(!dealable.is_empty());
    for c in &dealable {
        let (lib, ch) = t.server.catalog.challenge(&session_api::ChallengeId(c.challenge.clone())).unwrap();
        assert_eq!(lib.game.id.0, c.game);
        assert_eq!(ch.kind, ChallengeKind::TimeTrial);
        assert!(!ch.is_custom(), "{} is custom", c.challenge);
    }
}

#[test]
fn only_the_host_changes_the_lobby() {
    let mut t = Table::new(1);
    t.join("host", ace);
    t.join("guest", ace);
    t.send(1, ToServer::Settings(LobbySettings { mode: Mode::Combo, ..Default::default() }));
    let msgs = t.take(1);
    assert!(matches!(msgs.last(), Some(ToClient::Refused { .. })), "{msgs:?}");
    assert_eq!(t.server.settings.mode, Mode::Survival);
}

// ------------------------------------------------------------------- match

#[test]
fn a_solo_survival_match_runs_to_its_count() {
    let mut t = Table::new(42);
    t.join("solo", ace);
    t.settings(LobbySettings { format: Format::Challenges(3), ..Default::default() });
    t.start();
    for round in 0..3 {
        let (dealt_round, _, _) = t.last_dealt(0).unwrap();
        assert_eq!(dealt_round, round);
        let claimed = t.play_round();
        assert!(claimed[0].completed());
        assert!(t.errors(0).is_empty(), "{:?}", t.errors(0));
    }
    let standings = t.match_over(0).expect("the match is over after three rounds");
    assert_eq!(standings.len(), 1);
    assert_eq!(standings[0].completions, 3);
    assert_eq!(standings[0].rank, 1);
    assert!(t.last_dealt(0).map(|d| d.0) == Some(2), "no fourth deal");
}

#[test]
fn a_round_result_reaches_everyone_with_places() {
    let mut t = Table::new(7);
    t.join("ace", ace);
    t.join("steady", steady);
    t.join("reckless", reckless);
    t.settings(LobbySettings {
        mode: Mode::TimeTrial,
        format: Format::Challenges(1),
        ..Default::default()
    });
    t.start();
    t.play_round();
    for who in 0..3 {
        assert!(t.errors(who).is_empty(), "{:?}", t.errors(who));
        let result = t.clients[who].inbox.iter().find_map(|m| match m {
            ToClient::RoundResult { entries, .. } => Some(entries.clone()),
            _ => None,
        });
        let entries = result.expect("a round result");
        let place = |name: &str| entries.iter().find(|e| e.player == name).unwrap().place;
        assert_eq!(place("ace"), 1);
        assert_eq!(place("steady"), 2);
        assert_eq!(place("reckless"), 3);
        assert!(!entries.iter().find(|e| e.player == "reckless").unwrap().completed);
    }
    let standings = t.match_over(0).unwrap();
    assert_eq!(standings[0].player, "ace");
}

#[test]
fn a_time_trial_reentry_is_replayed_by_the_server() {
    let mut t = Table::new(3);
    t.join("steady", steady);
    t.settings(LobbySettings {
        mode: Mode::TimeTrial,
        format: Format::Challenges(1),
        ..Default::default()
    });
    t.start();
    let claimed = t.play_round();
    assert_eq!(claimed[0].attempts, 2);
    assert!(claimed[0].completed());
    assert!(t.errors(0).is_empty(), "{:?}", t.errors(0));
    let verdict = t.clients[0].inbox.iter().find_map(|m| match m {
        ToClient::PlayerFinished { outcome, .. } => Some(outcome.clone()),
        _ => None,
    });
    assert_eq!(verdict.as_ref(), Some(&claimed[0]));
}

#[test]
fn a_verdict_the_server_did_not_reach_is_an_error() {
    let mut t = Table::new(3);
    t.join("liar", ace);
    t.settings(LobbySettings { format: Format::Challenges(1), ..Default::default() });
    t.start();
    let (round, cref, _) = t.last_dealt(0).unwrap();
    let spec = t.spec(&cref);
    t.send(0, ToServer::Prepared { round, artifact: "abc".into() });
    // Claims a completion without having sent a single frame.
    t.send(
        0,
        ToServer::Finished {
            round,
            outcome: RunOutcome {
                challenge: spec.id.0.clone(),
                kind: spec.kind.into(),
                ending: Ending::Completed,
                frames: 10,
                attempts: 1,
            },
        },
    );
    let errors = t.errors(0);
    assert_eq!(errors.len(), 1, "{errors:?}");
    assert!(errors[0].contains("has not seen finish"), "{errors:?}");
    assert!(t.match_over(0).is_some(), "an error ends the match");
}

#[test]
fn differing_artifacts_abort_the_round() {
    let mut t = Table::new(5);
    t.join("a", ace);
    t.join("b", ace);
    t.settings(LobbySettings { format: Format::Challenges(1), ..Default::default() });
    t.start();
    let (round, _, _) = t.last_dealt(0).unwrap();
    t.send(0, ToServer::Prepared { round, artifact: "aaa".into() });
    assert!(t.errors(0).is_empty(), "one hash is not a mismatch");
    t.send(1, ToServer::Prepared { round, artifact: "bbb".into() });
    let errors = t.errors(1);
    assert!(errors.iter().any(|e| e.contains("artifact")), "{errors:?}");
    assert!(!t.clients[0].inbox.iter().any(|m| matches!(m, ToClient::Go { .. })));
}

#[test]
fn a_forfeit_is_judged_at_the_frame_it_was_given_up_on() {
    let mut t = Table::new(9);
    t.join("quitter", ace);
    t.settings(LobbySettings {
        mode: Mode::TimeTrial,
        format: Format::Challenges(1),
        ..Default::default()
    });
    t.start();
    let (round, _, _) = t.last_dealt(0).unwrap();
    t.send(0, ToServer::Prepared { round, artifact: "abc".into() });
    t.send(0, ToServer::Forfeit { round, frame: 333 });
    let verdict = t.clients[0].inbox.iter().find_map(|m| match m {
        ToClient::PlayerFinished { outcome, .. } => Some(outcome.clone()),
        _ => None,
    });
    let verdict = verdict.expect("a verdict");
    assert_eq!(verdict.ending, Ending::Forfeited);
    assert_eq!(verdict.frames, 333);
    // The client's own judge says the same, and the round closes on it.
    t.send(0, ToServer::Finished { round, outcome: verdict });
    assert!(t.errors(0).is_empty(), "{:?}", t.errors(0));
    assert!(t.match_over(0).is_some());
}

#[test]
fn the_host_can_end_a_match_and_play_again_from_a_new_seed() {
    let mut t = Table::new(11);
    t.join("solo", ace);
    t.settings(LobbySettings { format: Format::Challenges(10), ..Default::default() });
    t.start();
    let first = t.last_dealt(0).unwrap();
    t.play_round();
    t.send(0, ToServer::EndMatch);
    let standings = t.match_over(0).expect("ended");
    assert_eq!(standings[0].rounds, 1);

    t.take(0);
    t.send(0, ToServer::PlayAgain);
    let again = t.last_dealt(0).expect("a new match deals");
    assert_eq!(again.0, 0, "a new match starts at round 0");
    // A different seed deals a different first challenge, with overwhelming
    // probability over this pool; equal would be the same seed reused.
    assert_ne!(first.1, again.1);

    t.send(0, ToServer::EndMatch);
    t.send(0, ToServer::BackToLobby);
    assert!(t.lobby(0).can_start);
}

#[test]
fn eliminated_players_stop_being_dealt_runs() {
    let mut t = Table::new(13);
    t.join("ace", ace);
    t.join("reckless", reckless);
    t.settings(LobbySettings { mode: Mode::Survival, format: Format::Stock(1), ..Default::default() });
    t.start();
    t.play_round();
    assert!(t.errors(0).is_empty(), "{:?}", t.errors(0));
    // Reckless fails the first survival challenge and is out; with one of
    // two players left, the match is over.
    let standings = t.match_over(0).expect("over once one player stands");
    let reckless = standings.iter().find(|s| s.player == "reckless").unwrap();
    assert_eq!(reckless.eliminated, Some(0));
    assert_eq!(standings[0].player, "ace");
}

#[test]
fn only_a_match_in_progress_takes_frames() {
    let mut t = Table::new(1);
    t.join("solo", ace);
    t.send(0, ToServer::Frame { round: 0, frame: 1, events: vec![] });
    let msgs = t.take(0);
    assert!(matches!(msgs.last(), Some(ToClient::Refused { .. })), "{msgs:?}");
    let names: Vec<&str> = t.clients.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["solo"]);
}

// -------------------------------------------------------------------- void

fn voided(t: &Table, who: usize) -> Vec<(u64, String, String)> {
    t.clients[who]
        .inbox
        .iter()
        .filter_map(|m| match m {
            ToClient::Voided { round, player, reason } => Some((*round, player.clone(), reason.clone())),
            _ => None,
        })
        .collect()
}

#[test]
fn an_unprepared_round_is_voided_and_redealt() {
    let mut t = Table::new(42);
    t.join("host", ace);
    t.join("guest", ace);
    t.settings(LobbySettings { format: Format::Challenges(1), ..Default::default() });
    t.start();
    let (round, first, _) = t.last_dealt(0).unwrap();
    t.send(0, ToServer::Prepared { round, artifact: "abc".into() });
    t.send(1, ToServer::Unprepared { round, reason: "no core".into() });
    for who in 0..2 {
        assert_eq!(voided(&t, who), [(round, "guest".into(), "no core".into())]);
        assert!(t.errors(who).is_empty(), "a void is not an error: {:?}", t.errors(who));
    }
    let (next, second, _) = t.last_dealt(0).unwrap();
    assert_eq!(next, round + 1, "the voided index is not reused");
    assert_ne!(second, first);
    assert!(t.match_over(0).is_none(), "the void does not count toward the one challenge");
    t.play_round();
    let standings = t.match_over(0).expect("over after the one round played");
    assert!(standings.iter().all(|s| s.rounds == 1));
}

#[test]
fn a_voided_challenge_is_not_dealt_again() {
    let mut t = Table::new(5);
    t.join("solo", ace);
    t.settings(LobbySettings { format: Format::Challenges(20), ..Default::default() });
    t.start();
    let (round, first, _) = t.last_dealt(0).unwrap();
    t.send(0, ToServer::Unprepared { round, reason: "missing dump".into() });
    for _ in 0..20 {
        let (_, dealt, _) = t.last_dealt(0).unwrap();
        assert_ne!(dealt, first);
        t.play_round();
    }
    assert!(t.match_over(0).is_some());
}

#[test]
fn voiding_every_challenge_ends_the_match() {
    let mut t = Table::new(9);
    t.join("solo", ace);
    let only = t.lobby(0).enabled[..2].to_vec();
    t.send(0, ToServer::Enable { challenges: only });
    t.settings(LobbySettings { mode: Mode::Combo, format: Format::Wins(5), ..Default::default() });
    t.start();
    for _ in 0..2 {
        let (round, _, _) = t.last_dealt(0).unwrap();
        t.send(0, ToServer::Unprepared { round, reason: "no".into() });
    }
    assert_eq!(voided(&t, 0).len(), 2);
    assert!(t.match_over(0).is_some(), "nothing left to deal");
}

#[test]
fn unprepared_after_go_is_refused() {
    let mut t = Table::new(3);
    t.join("solo", ace);
    t.start();
    let (round, _, _) = t.last_dealt(0).unwrap();
    t.send(0, ToServer::Prepared { round, artifact: "abc".into() });
    t.take(0);
    t.send(0, ToServer::Unprepared { round, reason: "late".into() });
    let msgs = t.take(0);
    assert!(matches!(msgs.as_slice(), [ToClient::Refused { .. }]), "{msgs:?}");
}

#[test]
fn a_player_leaving_mid_match_ends_it() {
    let mut t = Table::new(4);
    t.join("host", ace);
    t.join("guest", ace);
    t.start();
    let conn = t.clients[1].conn;
    let out = t.server.disconnect(conn);
    t.deliver(out);
    assert_eq!(t.errors(0), ["guest left the match"]);
    assert!(t.match_over(0).is_some());
    let players: Vec<String> = t.lobby(0).players.iter().map(|p| p.id.clone()).collect();
    assert_eq!(players, ["host"]);
}

// ------------------------------------------------------------- spectating

fn video(round: u64, frame: u64) -> ToServer {
    ToServer::Video { round, frame, key: true, width: 256, height: 240, aspect: 1.0, data: vec![0, 0, 1].into() }
}

fn videos_from(t: &Table, who: usize) -> Vec<(String, u64)> {
    t.clients[who]
        .inbox
        .iter()
        .filter_map(|m| match m {
            ToClient::Video { player, frame, .. } => Some((player.clone(), *frame)),
            _ => None,
        })
        .collect()
}

fn watched(t: &Table, who: usize) -> Vec<u32> {
    t.clients[who]
        .inbox
        .iter()
        .filter_map(|m| match m {
            ToClient::Watched { watchers, .. } => Some(*watchers),
            _ => None,
        })
        .collect()
}

fn refusals(t: &Table, who: usize) -> Vec<String> {
    t.clients[who]
        .inbox
        .iter()
        .filter_map(|m| match m {
            ToClient::Refused { reason } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

/// A two-seat table with the host's run over and the guest's not begun.
fn host_done() -> (Table, u64, ChallengeSpec, u64) {
    let mut t = Table::new(42);
    t.join("host", ace);
    t.join("guest", ace);
    t.settings(LobbySettings { format: Format::Challenges(1), ..Default::default() });
    t.start();
    let (round, cref, limit) = t.last_dealt(0).unwrap();
    let spec = t.spec(&cref);
    for who in 0..2 {
        t.send(who, ToServer::Prepared { round, artifact: "abc".into() });
    }
    t.play(0, round, &spec, limit);
    for who in 0..2 {
        t.take(who);
    }
    (t, round, spec, limit)
}

#[test]
fn a_finished_player_watches_one_still_playing() {
    let (mut t, round, spec, limit) = host_done();
    t.send(0, ToServer::Watch { round, player: Some("guest".into()) });
    assert_eq!(watched(&t, 1), [1], "the guest is told they have a watcher");
    assert!(watched(&t, 0).is_empty());
    t.send(1, video(round, 10));
    t.send(1, ToServer::Audio { round, frame: 10, rate: 32040, channels: 2, data: vec![0].into() });
    assert_eq!(videos_from(&t, 0), [("guest".to_string(), 10)]);
    assert!(videos_from(&t, 1).is_empty(), "a stream never comes back to its sender");
    assert!(
        t.clients[0].inbox.iter().any(|m| matches!(m, ToClient::Audio { player, frame: 10, rate: 32040, channels: 2, .. } if player == "guest")),
        "{:?}",
        t.clients[0].inbox
    );
    t.take(0);
    t.send(0, ToServer::Watch { round, player: None });
    assert_eq!(watched(&t, 1), [1, 0], "and told when the watcher leaves");
    t.send(1, video(round, 11));
    assert!(videos_from(&t, 0).is_empty(), "nobody watching, nothing relayed");
    // Watch again, then the round ends under it.
    t.send(0, ToServer::Watch { round, player: Some("guest".into()) });
    t.play(1, round, &spec, limit);
    // Every frame the guest reported reached the watcher as where the run
    // stood, the last one with the server's own clock (ADR 0034).
    let progress: Vec<(u64, u32, u64)> = t.clients[0]
        .inbox
        .iter()
        .filter_map(|m| match m {
            ToClient::Progress { player, frame, attempt, clock, .. } if player == "guest" => Some((*frame, *attempt, *clock)),
            _ => None,
        })
        .collect();
    let finished = t.clients[0].inbox.iter().find_map(|m| match m {
        ToClient::PlayerFinished { player, outcome, .. } if player == "guest" => Some(outcome.clone()),
        _ => None,
    });
    let (_, attempt, clock) = *progress.last().expect("progress reached the watcher");
    let outcome = finished.expect("the guest finished");
    assert_eq!((attempt, clock), (outcome.attempts, outcome.frames));
    t.take(0);
    t.send(1, video(round, 12));
    assert!(videos_from(&t, 0).is_empty(), "a chunk from an ended round goes nowhere");
    assert!(refusals(&t, 1).is_empty(), "and is not an error: {:?}", refusals(&t, 1));
    assert!(t.errors(0).is_empty());
}

#[test]
fn only_a_player_whose_run_is_over_may_watch() {
    let (mut t, round, _, _) = host_done();
    t.send(1, ToServer::Watch { round, player: Some("host".into()) });
    assert_eq!(refusals(&t, 1), ["you are still playing this round"]);
    t.send(0, ToServer::Watch { round, player: Some("host".into()) });
    assert_eq!(refusals(&t, 0), ["you cannot watch yourself"]);
    t.take(0);
    t.send(0, ToServer::Watch { round, player: Some("nobody".into()) });
    assert_eq!(refusals(&t, 0), ["nobody is not playing this round"]);
    t.take(0);
    t.send(0, ToServer::Watch { round: round + 7, player: Some("guest".into()) });
    assert!(refusals(&t, 0).is_empty(), "a round that is not live is ignored, not refused");
    assert!(watched(&t, 1).is_empty());
}

#[test]
fn an_eliminated_player_watches_from_the_deal() {
    let mut t = Table::new(7);
    t.join("host", ace);
    t.join("guest", reckless);
    // A third seat: with two, one elimination would end the match.
    t.join("other", ace);
    t.settings(LobbySettings { mode: Mode::Survival, format: Format::Strikes(1), ..Default::default() });
    t.start();
    t.play_round();
    let (round, cref, limit) = t.last_dealt(0).expect("a second round for the two survivors");
    let spec = t.spec(&cref);
    for who in 0..3 {
        t.take(who);
    }
    // The guest is out and prepares nothing; they may watch before `Go`.
    t.send(1, ToServer::Watch { round, player: Some("host".into()) });
    assert_eq!(watched(&t, 0), [1]);
    assert!(refusals(&t, 1).is_empty(), "{:?}", refusals(&t, 1));
    for who in [0, 2] {
        t.send(who, ToServer::Prepared { round, artifact: "abc".into() });
    }
    t.send(0, video(round, 1));
    assert_eq!(videos_from(&t, 1), [("host".to_string(), 1)]);
    assert!(videos_from(&t, 2).is_empty(), "only the watcher gets it");
    t.play(0, round, &spec, limit);
    t.play(2, round, &spec, limit);
    assert!(t.errors(1).is_empty());
}

// ------------------------------------------------------------ head-to-head

/// A fighting game's challenge half, for bouts (ADR 0036). No manifest in
/// `manifests/` deals one yet, and the server reads only this half anyway.
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

fn fighting_catalog() -> Catalog {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mut texts = vec![FIGHT.to_string()];
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "toml") {
            texts.push(fs::read_to_string(&path).unwrap());
        }
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    Catalog::parse_all(&refs).unwrap()
}

fn won(frame: u64) -> protocol::Event {
    (&session_api::SessionEvent::Counted { frame, counter: "rounds_won".into(), count: 1 }).into()
}

fn outcome(ending: Ending, frames: u64) -> RunOutcome {
    RunOutcome { challenge: "fight-best-of-three".into(), kind: protocol::Kind::HeadToHead, ending, frames, attempts: 1 }
}

fn fetch<T>(t: &Table, who: usize, f: impl Fn(&ToClient) -> Option<T>) -> Vec<T> {
    t.clients[who].inbox.iter().filter_map(f).collect()
}

fn got_go(t: &Table, who: usize, round: u64) -> bool {
    t.clients[who].inbox.iter().any(|m| matches!(m, ToClient::Go { round: r } if *r == round))
}

impl Table {
    /// A table on a server that can broker a bout, with the fight enabled
    /// and nothing else.
    fn fighting(seed: u64, names: &[&str]) -> Table {
        let server = Server::new(fighting_catalog(), "test", seed).with_rendezvous(":7878");
        let mut t = Table { server, clients: Vec::new() };
        for n in names {
            t.join(n, ace);
        }
        t.settings(LobbySettings {
            mode: Mode::Combo,
            format: Format::Challenges(1),
            allow_head_to_head: true,
            ..Default::default()
        });
        t.send(0, ToServer::Enable { challenges: vec![r("fight", "fight-best-of-three")] });
        t
    }

    fn who(&self, player: &str) -> usize {
        self.clients.iter().position(|c| c.name == player).unwrap_or_else(|| panic!("{player}"))
    }

    /// The seated players of the last deal, in slot order, and everyone else.
    fn slots(&self) -> (Vec<String>, Vec<String>) {
        let players = self.clients[0]
            .inbox
            .iter()
            .rev()
            .find_map(|m| match m {
                ToClient::Dealt { players, .. } => Some(players.clone()),
                _ => None,
            })
            .expect("a deal");
        let rest = self.clients.iter().map(|c| c.name.clone()).filter(|n| !players.contains(n)).collect();
        (players, rest)
    }

    /// Every seated player says where it is and the server sees its probe,
    /// then prepares. Returns the round.
    fn rendezvous(&mut self) -> u64 {
        let (round, _, _) = self.last_dealt(0).expect("a round was dealt");
        let (slots, _) = self.slots();
        for (i, p) in slots.iter().enumerate() {
            let who = self.who(p);
            let token = 1000 + i as u64;
            if i % 2 == 0 {
                // A probe that beats its `Endpoint` to the server.
                let out = self.server.observed(token, format!("203.0.113.{i}:7000"));
                self.deliver(out);
            }
            self.send(who, ToServer::Endpoint { round, token, local: format!("10.0.0.{i}:7000") });
            if i % 2 == 1 {
                let out = self.server.observed(token, format!("203.0.113.{i}:7000"));
                self.deliver(out);
            }
        }
        for p in &slots {
            let who = self.who(p);
            let peers = fetch(self, who, |m| match m {
                ToClient::Peers { round: r, peers } if *r == round => Some(peers.clone()),
                _ => None,
            });
            assert_eq!(peers.len(), 1, "{p} hears where the slots are once: {peers:?}");
            assert_eq!(peers[0].len(), slots.len());
            for (i, peer) in peers[0].iter().enumerate() {
                assert_eq!((peer.slot, &peer.player), (i as u8, &slots[i]));
                assert_eq!(peer.candidates, [format!("10.0.0.{i}:7000"), format!("203.0.113.{i}:7000")]);
            }
            self.send(who, ToServer::Prepared { round, artifact: "abc".into() });
        }
        round
    }

    /// Slot 0 wins rounds at 100 and 300, slot 1 at 200, each frame reported
    /// by its own client in its own order; both claim, and the winner's
    /// frames come last so the loser's claim is in first.
    fn fight(&mut self, round: u64) {
        let (slots, _) = self.slots();
        let (a, b) = (self.who(&slots[0]), self.who(&slots[1]));
        self.send(a, ToServer::Frame { round, frame: 100, events: vec![won(100)] });
        self.send(b, ToServer::Frame { round, frame: 200, events: vec![won(200)] });
        self.send(b, ToServer::Frame { round, frame: 300, events: vec![] });
        self.send(b, ToServer::Finished { round, outcome: outcome(Ending::Beaten, 300) });
        assert!(self.errors(a).is_empty(), "a claim ahead of the verdict is held, not an error");
        self.send(a, ToServer::Frame { round, frame: 300, events: vec![won(300)] });
        self.send(a, ToServer::Finished { round, outcome: outcome(Ending::Completed, 300) });
    }
}

fn entries(t: &Table, who: usize) -> Vec<protocol::RoundEntry> {
    fetch(t, who, |m| match m {
        ToClient::RoundResult { entries, .. } => Some(entries.clone()),
        _ => None,
    })
    .pop()
    .expect("a round result")
}

#[test]
fn a_bout_seats_two_and_the_backer_takes_the_losers_entry() {
    let mut t = Table::fighting(5, &["ann", "bob", "cy"]);
    assert!(t.lobby(0).can_start, "{:?}", t.lobby(0).blocking);
    t.start();
    let (slots, rest) = t.slots();
    assert_eq!((slots.len(), rest.len()), (2, 1));
    let backer = t.who(&rest[0]);
    let round = t.rendezvous();
    assert!(!got_go(&t, 0, round), "no Go while a backer has not picked");
    t.send(backer, ToServer::Back { round, player: slots[1].clone() });
    let sides = fetch(&t, 0, |m| match m {
        ToClient::Sides { backing, .. } => Some(backing.clone()),
        _ => None,
    });
    assert_eq!(sides.last().unwrap(), &vec![(rest[0].clone(), slots[1].clone())]);
    assert!(got_go(&t, 0, round));
    t.fight(round);
    let e = entries(&t, backer);
    assert_eq!(e.len(), 3);
    let of = |p: &str| e.iter().find(|x| x.player == p).unwrap().clone();
    assert_eq!((of(&slots[0]).place, of(&slots[0]).points, of(&slots[0]).completed), (1, 2, true));
    assert_eq!(of(&slots[1]).outcome.ending, Ending::Beaten);
    let b = of(&rest[0]);
    assert_eq!(b.backed.as_deref(), Some(slots[1].as_str()));
    assert_eq!((b.place, b.failure, b.completed), (2, true, false));
    assert_eq!(b.outcome, of(&slots[1]).outcome);
    for who in 0..3 {
        assert!(t.errors(who).is_empty(), "{:?}", t.errors(who));
    }
    assert!(t.match_over(0).is_some());
}

#[test]
fn a_backer_who_does_not_pick_is_assigned_when_the_window_closes() {
    let mut t = Table::fighting(5, &["ann", "bob", "cy"]);
    t.start();
    let round = t.rendezvous();
    let now = Instant::now();
    let out = t.server.tick(now);
    t.deliver(out);
    assert!(!got_go(&t, 0, round));
    let out = t.server.tick(now + Duration::from_secs(u64::from(BACKING_SECONDS)));
    t.deliver(out);
    let sides = fetch(&t, 0, |m| match m {
        ToClient::Sides { backing, .. } => Some(backing.clone()),
        _ => None,
    });
    let (slots, rest) = t.slots();
    assert_eq!(sides.last().unwrap().len(), 1);
    assert_eq!(sides.last().unwrap()[0].0, rest[0]);
    assert!(slots.contains(&sides.last().unwrap()[0].1));
    assert!(got_go(&t, 0, round));
}

#[test]
fn a_two_player_bout_goes_without_backing() {
    let mut t = Table::fighting(5, &["ann", "bob"]);
    t.start();
    let round = t.rendezvous();
    assert!(got_go(&t, 0, round));
    assert!(fetch(&t, 0, |m| matches!(m, ToClient::Sides { .. }).then_some(())).is_empty());
    t.fight(round);
    assert_eq!(entries(&t, 0).len(), 2);
    assert!(t.errors(0).is_empty());
}

#[test]
fn a_slot_that_cannot_reach_the_other_voids_the_round() {
    let mut t = Table::fighting(5, &["ann", "bob", "cy"]);
    t.start();
    let (round, _, _) = t.last_dealt(0).unwrap();
    let (slots, _) = t.slots();
    let who = t.who(&slots[0]);
    t.send(who, ToServer::Unprepared { round, reason: format!("could not reach {}", slots[1]) });
    let voided = fetch(&t, 0, |m| match m {
        ToClient::Voided { round: r, reason, .. } => Some((*r, reason.clone())),
        _ => None,
    });
    assert_eq!(voided.len(), 1);
    assert_eq!(voided[0].0, round);
    assert!(t.match_over(0).is_some(), "the only challenge is withdrawn");
}

#[test]
fn a_backer_may_watch_from_the_deal() {
    let mut t = Table::fighting(5, &["ann", "bob", "cy"]);
    t.start();
    let (round, _, _) = t.last_dealt(0).unwrap();
    let (slots, rest) = t.slots();
    let backer = t.who(&rest[0]);
    for who in 0..3 {
        t.take(who);
    }
    t.send(backer, ToServer::Watch { round, player: Some(slots[0].clone()) });
    assert!(refusals(&t, backer).is_empty(), "{:?}", refusals(&t, backer));
    assert_eq!(watched(&t, t.who(&slots[0])), [1]);
}

#[test]
fn a_lost_link_is_lost_by_both_after_the_grace() {
    let mut t = Table::fighting(5, &["ann", "bob"]);
    t.start();
    let round = t.rendezvous();
    let (slots, _) = t.slots();
    let (a, b) = (t.who(&slots[0]), t.who(&slots[1]));
    t.send(a, ToServer::Frame { round, frame: 100, events: vec![won(100)] });
    t.send(a, ToServer::PeerLost { round, frame: 100 });
    t.send(b, ToServer::PeerLost { round, frame: 110 });
    assert!(entries_or_none(&t, 0).is_none(), "nothing is called before the grace");
    let now = Instant::now();
    let out = t.server.tick(now);
    t.deliver(out);
    let out = t.server.tick(now + LOST_GRACE);
    t.deliver(out);
    let e = entries(&t, 0);
    assert!(e.iter().all(|x| x.outcome.ending == Ending::Forfeited && x.failure), "{e:?}");
    assert_eq!(e.iter().map(|x| x.outcome.frames).collect::<Vec<_>>().len(), 2);
    assert!(t.errors(0).is_empty());
}

#[test]
fn a_give_up_after_the_other_lost_the_link_hands_the_win() {
    let mut t = Table::fighting(5, &["ann", "bob"]);
    t.start();
    let round = t.rendezvous();
    let (slots, _) = t.slots();
    let (a, b) = (t.who(&slots[0]), t.who(&slots[1]));
    t.send(a, ToServer::PeerLost { round, frame: 100 });
    t.send(b, ToServer::Forfeit { round, frame: 130 });
    t.send(b, ToServer::Finished { round, outcome: outcome(Ending::Forfeited, 130) });
    let e = entries(&t, 0);
    let of = |p: &str| e.iter().find(|x| x.player == p).unwrap().clone();
    assert_eq!(of(&slots[0]).outcome, outcome(Ending::Completed, 130));
    assert_eq!(of(&slots[1]).outcome, outcome(Ending::Forfeited, 130));
    assert!(t.errors(0).is_empty());
}

fn entries_or_none(t: &Table, who: usize) -> Option<Vec<protocol::RoundEntry>> {
    fetch(t, who, |m| match m {
        ToClient::RoundResult { entries, .. } => Some(entries.clone()),
        _ => None,
    })
    .pop()
}

#[test]
fn a_bout_needs_the_toggle_a_rendezvous_and_enough_players() {
    let mut t = Table::fighting(5, &["ann"]);
    assert!(!t.lobby(0).can_start, "one player cannot fill two slots");
    t.join("bob", ace);
    assert!(t.lobby(0).can_start);
    t.settings(LobbySettings { allow_head_to_head: false, mode: Mode::Combo, ..Default::default() });
    assert!(!t.lobby(0).can_start, "the toggle is off");

    let mut plain = Table { server: Server::new(fighting_catalog(), "test", 1), clients: Vec::new() };
    plain.join("ann", ace);
    plain.join("bob", ace);
    plain.settings(LobbySettings { allow_head_to_head: true, mode: Mode::Combo, ..Default::default() });
    plain.send(0, ToServer::Enable { challenges: vec![r("fight", "fight-best-of-three")] });
    assert!(!plain.lobby(0).can_start, "no rendezvous, no bout");
}
