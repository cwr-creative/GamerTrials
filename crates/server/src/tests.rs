//! The server driven the way a client drives it, on scripted telemetry.
//!
//! The client's part here is [`Client::play`]: the loop `app-client` runs on
//! a real runner, on a `FakeRunner` instead. It keeps the one rule the wire
//! depends on: a `Frame` goes to the server for every frame that carried an
//! event and for the frame the client's own judge settled the run on, and
//! nothing else, so the server replays the judgement from exactly those.

use std::fs;
use std::path::Path;

use challenge_core::{Next, Run};
use challenge_library::Catalog;
use protocol::{
    ChallengeRef, Ending, FileReport, Format, LobbySettings, MissingFile, Mode, RunOutcome,
    ToClient, ToServer,
};
use session_api::{ChallengeKind, ChallengeSpec, StageRunner};
use session_fake::{stage_for, Attempt, FakeRunner};

use super::{ConnId, Outbox, Server};

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
        self.send(who, ToServer::Hello { name: name.into(), library_hash: hash });
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
        ToServer::Hello { name: "x".into(), library_hash: "0000000000000000".into() },
    );
    assert!(matches!(&out[..], [(c, ToClient::Refused { reason })] if *c == conn && reason.contains("library")));
}

#[test]
fn the_lobby_blocks_start_until_files_are_reported_and_names_the_reason() {
    let mut t = Table::new(1);
    let conn = t.server.connect();
    t.clients.push(Client { conn, name: "solo".into(), inbox: Vec::new(), style: ace });
    let hash = t.server.library_hash();
    t.send(0, ToServer::Hello { name: "solo".into(), library_hash: hash });
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
