//! Multiplayer sessions on scripted telemetry.
//!
//! Nothing runs a session yet, so these tests play the part of the driver: the
//! loop from `challenge_core::run`'s docs, once per player, every player one
//! frame per tick. The server half of ADR 0009 — deal, judge, place — is
//! `challenge-core` itself; the client half — enter, advance, re-enter — is
//! [`play_together`] below, and the game is a [`FakeRunner`].
//!
//! One choice here is the tests', not the platform's: a runner that fails is
//! a forfeit. The bound is the match's (ADR 0026): every run takes the limit
//! its [`Deal`] carries, and the single-challenge tests use [`LIMIT`] for it.
//!
//! The match tests at the end drive a [`Match`] the way the server will: deal,
//! play every active player, record, until it is over.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use challenge_core::{
    deal, places, AttemptEnd, Deal, Ending, Entry, Format, Match, Mode, Next, Run, RunResult,
    Settings, Standing,
};
use challenge_library::Library;
use session_api::{
    ChallengeId, ChallengeKind, ChallengeSpec, Completion, ExitReason, GameId, PlayerId, StageId,
    StageRunner, DAMAGED,
};
use session_fake::{stage_for, Attempt, FakeRunner};

/// The frame limit of every run in the single-challenge tests, and the match
/// tests' default limit.
const LIMIT: u64 = 5_000;

// ------------------------------------------------------------------- driver

struct Seat {
    player: String,
    runner: FakeRunner,
    run: Run,
    result: Option<RunResult>,
}

/// What one challenge came to, for every player.
struct Outcome {
    /// By player, in seat order.
    results: Vec<(String, RunResult)>,
    /// Players in the order their runs ended. Whoever is ahead of a player
    /// here was spectating them.
    finished: Vec<String>,
    /// How many times each player entered the stage, by player.
    entries: BTreeMap<String, usize>,
}

impl Outcome {
    fn result(&self, player: &str) -> &RunResult {
        &self.results.iter().find(|(p, _)| p == player).unwrap().1
    }

    /// Places in seat order, by `challenge_core::places`.
    fn places(&self) -> Vec<u32> {
        places(&self.results.iter().map(|(_, r)| r.clone()).collect::<Vec<_>>())
    }
}

/// Plays one challenge for every player at once: each tick advances every
/// player still running by one frame. Lockstep is not what a real session
/// does — every client runs at its own pace — and nothing scored may depend on
/// it, which `interleaving_players_changes_no_result` checks.
fn play_together(challenge: &ChallengeSpec, players: Vec<(&str, Vec<Attempt>)>) -> Outcome {
    play_within(challenge, players, LIMIT)
}

fn play_within(
    challenge: &ChallengeSpec,
    players: Vec<(&str, Vec<Attempt>)>,
    limit: u64,
) -> Outcome {
    let stage = stage_for(challenge);
    let mut seats: Vec<Seat> = players
        .into_iter()
        .map(|(player, attempts)| {
            let mut runner = FakeRunner::new(stage.clone(), attempts);
            let mut run = Run::new(challenge.clone()).unwrap().with_limit(limit);
            runner.enter(&stage).unwrap();
            run.enter();
            Seat { player: player.to_string(), runner, run, result: None }
        })
        .collect();

    let mut finished = Vec::new();
    while seats.iter().any(|s| s.result.is_none()) {
        for seat in seats.iter_mut().filter(|s| s.result.is_none()) {
            let next = match seat.runner.advance() {
                Ok(events) => {
                    let events = events.to_vec();
                    seat.run.observe(seat.runner.frame(), &events)
                }
                Err(_) => Next::Finished(seat.run.forfeit()),
            };
            match next {
                Next::Continue => {}
                Next::Reenter => {
                    seat.runner.enter(&stage).unwrap_or_else(|e| {
                        panic!("{} ran out of script on {}: {e}", seat.player, challenge.id)
                    });
                    seat.run.enter();
                }
                Next::Finished(result) => {
                    seat.result = Some(result);
                    finished.push(seat.player.clone());
                }
            }
        }
    }

    Outcome {
        entries: seats.iter().map(|s| (s.player.clone(), s.runner.entries())).collect(),
        results: seats.into_iter().map(|s| (s.player, s.result.unwrap())).collect(),
        finished,
    }
}

fn play_alone(challenge: &ChallengeSpec, attempts: Vec<Attempt>) -> RunResult {
    play_together(challenge, vec![("solo", attempts)]).results.remove(0).1
}

// ------------------------------------------------------------------ players

/// Meets every criterion on the first try, fast.
fn ace(c: &ChallengeSpec) -> Vec<Attempt> {
    vec![Attempt::meeting(&c.complete_when, at_least(c, 600))]
}

/// Dies once in a time trial and then gets there; takes its time surviving.
fn steady(c: &ChallengeSpec) -> Vec<Attempt> {
    match c.kind {
        ChallengeKind::TimeTrial => vec![
            Attempt::new().died(200),
            Attempt::meeting(&c.complete_when, at_least(c, 900)),
        ],
        ChallengeKind::Survival => vec![Attempt::meeting(&c.complete_when, at_least(c, 1200))],
    }
}

/// Dies early, three times, then stops playing.
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

/// `frame`, or later if the challenge asks to survive longer than that.
fn at_least(c: &ChallengeSpec, frame: u64) -> u64 {
    c.complete_when.survive_frames.map_or(frame, |n| n.max(frame))
}

fn everyone(c: &ChallengeSpec) -> Vec<(&'static str, Vec<Attempt>)> {
    vec![("ace", ace(c)), ("steady", steady(c)), ("reckless", reckless(c))]
}

// ------------------------------------------------------------------ library

/// Every challenge in `manifests/`, read the way the server will read it: the
/// challenge half only, through `challenge-library`, with no emulator.
fn library() -> Vec<ChallengeSpec> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mut specs = Vec::new();
    for entry in fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "toml") {
            let text = fs::read_to_string(&path).unwrap();
            let lib = Library::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            specs.extend(lib.specs());
        }
    }
    assert!(!specs.is_empty(), "no manifests in {}", dir.display());
    specs
}

fn challenge(kind: ChallengeKind, complete_when: Completion) -> ChallengeSpec {
    ChallengeSpec {
        id: ChallengeId("c".into()),
        name: "C".into(),
        game: GameId("g".into()),
        stage: StageId("s".into()),
        kind,
        complete_when,
        ends_attempt_on: Default::default(),
    }
}

fn reach_the_end() -> Completion {
    Completion { cleared: true, ..Default::default() }
}

// -------------------------------------------------------------------- tests

/// Every challenge any manifest declares can be completed by the facts it
/// asks for. A challenge whose own `ends_attempt_on` undoes its criteria would
/// fail here, and so would one `challenge-core` could not judge at all.
#[test]
fn every_challenge_in_the_library_can_be_completed() {
    for c in library() {
        let r = play_alone(&c, ace(&c));
        assert_eq!(r.ending, Ending::Completed, "{}/{}: {r}", c.game, c.id);
        assert_eq!(r.attempts, 1);
    }
}

/// A whole session: dealt from the real library by seed, played by three
/// players, placed challenge by challenge. Totals across challenges are not
/// decided, so this stops at places.
#[test]
fn a_dealt_session_places_every_player_on_every_challenge() {
    let library = library();
    let dealt = deal(&library, Mode::Both, 6, 2026).unwrap();
    let kinds: Vec<_> = dealt.iter().map(|c| c.kind).collect();
    assert!(
        kinds.contains(&ChallengeKind::TimeTrial) && kinds.contains(&ChallengeKind::Survival),
        "seed 2026 should deal both kinds; dealt {kinds:?}"
    );

    for c in dealt {
        let outcome = play_together(c, everyone(c));
        let places = outcome.places();
        match c.kind {
            // Fewest frames first; a timeout places behind every finisher.
            ChallengeKind::TimeTrial => {
                assert_eq!(places, [1, 2, 3], "{}", c.id);
                assert_eq!(outcome.result("reckless").ending, Ending::TimedOut);
                assert_eq!(outcome.result("reckless").frames, LIMIT);
                assert_eq!(outcome.result("steady").attempts, 2);
            }
            // Completed or not; how fast is not what the kind measures.
            ChallengeKind::Survival => {
                assert_eq!(places, [1, 1, 3], "{}", c.id);
                assert_eq!(outcome.result("reckless").ending, Ending::Failed(AttemptEnd::Died));
            }
        }
    }
}

/// Whoever finishes first spectates the others until they finish too. A
/// survival death finishes a run as surely as completing it does.
#[test]
fn players_finish_in_the_order_their_runs_end() {
    let tt = challenge(ChallengeKind::TimeTrial, reach_the_end());
    let o = play_together(&tt, everyone(&tt));
    assert_eq!(o.finished, ["ace", "steady", "reckless"]);
    assert_eq!(o.entries["reckless"], 4);

    let sv = challenge(ChallengeKind::Survival, reach_the_end());
    let o = play_together(&sv, everyone(&sv));
    assert_eq!(o.finished, ["reckless", "ace", "steady"]);
    assert_eq!(o.entries["reckless"], 1, "a survival death is never re-entered");
}

/// Frames are emulated (invariant 4), so a player's result cannot depend on
/// who else is in the session or how their clients are paced.
#[test]
fn interleaving_players_changes_no_result() {
    for c in library() {
        let together = play_together(&c, everyone(&c));
        for (player, script) in everyone(&c) {
            assert_eq!(&play_alone(&c, script), together.result(player), "{player} on {}", c.id);
        }
    }
}

/// DKC reports a death and an exit on one frame. That is one attempt ending,
/// not two, so the next entry is the second and not the third.
#[test]
fn a_death_and_an_exit_on_one_frame_are_one_reentry() {
    let c = challenge(ChallengeKind::TimeTrial, reach_the_end());
    let o = play_together(
        &c,
        vec![(
            "dkc",
            vec![
                Attempt::new().died(300).exited(300, ExitReason::Death),
                Attempt::new().cleared(700),
            ],
        )],
    );
    assert_eq!(o.entries["dkc"], 2);
    assert_eq!(o.result("dkc").frames, 1000);
}

/// Doom reports a death and stays in the level until the player presses use.
/// The attempt ends at the death all the same.
#[test]
fn a_death_with_no_exit_still_reenters() {
    let c = challenge(ChallengeKind::TimeTrial, reach_the_end());
    let r = play_alone(&c, vec![Attempt::new().died(140), Attempt::new().cleared(2100)]);
    assert_eq!(r.ending, Ending::Completed);
    assert_eq!((r.frames, r.attempts), (2240, 2));
}

/// A no-hit time trial: the hit costs what a death costs (ADR 0025), and the
/// player who took none wins although they were slower to the goal.
#[test]
fn a_hit_in_a_no_hit_run_sends_the_player_back() {
    let mut c = challenge(ChallengeKind::TimeTrial, reach_the_end());
    c.ends_attempt_on.insert(DAMAGED.to_string());
    let o = play_together(
        &c,
        vec![
            ("clipped", vec![Attempt::new().damaged(500).cleared(800), Attempt::new().cleared(800)]),
            ("clean", vec![Attempt::new().cleared(1100)]),
        ],
    );
    assert_eq!(o.result("clipped").frames, 1300);
    assert_eq!(o.places(), [2, 1]);
}

/// Progress is per attempt, so dying after three of four stomps and stomping
/// once more is one stomp — the player is still short when the limit arrives.
#[test]
fn stomps_cannot_be_farmed_by_dying() {
    let stomps = Completion {
        counts: BTreeMap::from([(session_api::ENEMY_DEFEATED.to_string(), 4)]),
        ..Default::default()
    };
    let c = challenge(ChallengeKind::TimeTrial, stomps);
    let three_then_die = Attempt::new().defeated(10, 1).defeated(20, 1).defeated(30, 1).died(40);
    let r = play_alone(&c, vec![three_then_die, Attempt::new().defeated(10, 1)]);
    assert_eq!(r.ending, Ending::TimedOut);
}

/// A client whose game crashes is forfeited here (the tests' policy, not a
/// settled rule). Everyone else's run is untouched by it.
#[test]
fn a_crashed_runner_is_forfeited_and_the_session_goes_on() {
    let c = challenge(ChallengeKind::TimeTrial, reach_the_end());
    let o = play_together(
        &c,
        vec![
            ("crashed", vec![Attempt::new().cleared(400).fails_at(250, "core crashed")]),
            ("fine", vec![Attempt::new().cleared(900)]),
        ],
    );
    let crashed = o.result("crashed");
    assert_eq!((crashed.ending.clone(), crashed.frames), (Ending::Forfeited, 249));
    assert_eq!(o.result("fine").ending, Ending::Completed);
    assert_eq!(o.places(), [2, 1]);
    assert_eq!(o.finished, ["crashed", "fine"]);
}

/// A survival criterion completes with nothing announcing it, on the frame it
/// is reached, for everyone who is still alive then.
#[test]
fn survivors_tie_on_the_frame_they_survive_to() {
    let c = challenge(
        ChallengeKind::Survival,
        Completion { survive_frames: Some(2704), ..Default::default() },
    );
    let o = play_together(
        &c,
        vec![
            ("idle", vec![Attempt::new()]),
            ("busy", vec![Attempt::new().damaged(100).defeated(900, 2)]),
            ("late", vec![Attempt::new().died(2704)]),
        ],
    );
    assert_eq!(o.result("idle").frames, 2704);
    assert_eq!(o.result("late").ending, Ending::Failed(AttemptEnd::Died));
    assert_eq!(o.places(), [1, 1, 3]);
}

// ------------------------------------------------------------------ matches

/// Each player's script, by name, for whatever challenge is dealt.
fn script(player: &str, c: &ChallengeSpec) -> Vec<Attempt> {
    match player {
        "ace" => ace(c),
        "steady" => steady(c),
        "reckless" => reckless(c),
        other => panic!("no script for {other}"),
    }
}

fn pool() -> Vec<Entry> {
    library().into_iter().map(|spec| Entry { spec, limit: None }).collect()
}

fn names(players: &[&str]) -> Vec<PlayerId> {
    players.iter().map(|p| PlayerId(p.to_string())).collect()
}

/// Plays a whole match: deal, play every active player on the dealt
/// challenge within its limit, record, until the match says it is over.
/// Returns the match for its rounds and standings.
fn play_match(players: &[&str], mode: Mode, format: Format, seed: u64) -> Match {
    let settings = Settings { mode, format, default_limit: LIMIT };
    let mut m = Match::new(pool(), names(players), settings, seed).unwrap();
    while let Some(deal) = m.deal() {
        let active: Vec<String> = m.active().into_iter().map(|p| p.0.clone()).collect();
        let seats = active.iter().map(|p| (p.as_str(), script(p, &deal.challenge))).collect();
        let outcome = play_within(&deal.challenge, seats, deal.limit);
        let results = outcome.results.into_iter().map(|(p, r)| (PlayerId(p), r)).collect();
        m.record(results);
    }
    m
}

fn standing(m: &Match, player: &str) -> Standing {
    m.standings().into_iter().find(|s| s.player.0 == player).unwrap()
}

fn ranks(m: &Match) -> Vec<(String, u32)> {
    m.standings().into_iter().map(|s| (s.player.0, s.rank)).collect()
}

/// Ten challenges from the real library, both kinds, three players: the
/// default match. Every round reads every active player, and the standings
/// come out in the order the scripts deserve.
#[test]
fn a_ten_challenge_match_on_the_library_stands_everyone() {
    let m = play_match(&["ace", "steady", "reckless"], Mode::Both, Format::Challenges(10), 2026);
    assert_eq!(m.rounds().len(), 10);
    assert!(m.rounds().iter().all(|r| r.entries.len() == 3));
    assert_eq!(ranks(&m), [("ace".into(), 1), ("steady".into(), 2), ("reckless".into(), 3)]);
    let (ace, steady, reckless) =
        (standing(&m, "ace"), standing(&m, "steady"), standing(&m, "reckless"));
    assert!(ace.points > steady.points && steady.points > reckless.points);
    assert_eq!(ace.completions, 10);
    assert_eq!(reckless.completions, 0);
    assert_eq!(reckless.points, 0);
}

/// A time-trial match is total frames, and a run that never completes costs
/// the full limit every time.
#[test]
fn a_time_trial_match_totals_frames_and_a_timeout_costs_the_limit() {
    let m = play_match(&["ace", "steady", "reckless"], Mode::TimeTrial, Format::Challenges(3), 7);
    assert_eq!(m.rounds().len(), 3);
    assert_eq!(standing(&m, "reckless").frames, 3 * LIMIT);
    assert!(standing(&m, "ace").frames < standing(&m, "steady").frames);
    assert_eq!(ranks(&m), [("ace".into(), 1), ("steady".into(), 2), ("reckless".into(), 3)]);
    assert!(m.rounds().iter().all(|r| r.deal.limit == LIMIT));
}

/// A survival match counts completions, so two players who clear everything
/// share first however fast either was.
#[test]
fn a_survival_match_counts_completions_and_ties_the_finishers() {
    let m = play_match(&["ace", "steady", "reckless"], Mode::Survival, Format::Challenges(4), 11);
    assert_eq!(ranks(&m), [("ace".into(), 1), ("steady".into(), 1), ("reckless".into(), 3)]);
    assert_eq!(standing(&m, "steady").completions, 4);
    assert_eq!(standing(&m, "reckless").failures, 4);
    assert!(m.rounds().iter().all(|r| r.eliminated.is_empty()));
}

/// Stock: reckless fails the first challenge, loses their one life, and the
/// other player is the last standing, so the match ends after one round.
#[test]
fn a_stock_match_ends_when_one_player_stands() {
    let m = play_match(&["steady", "reckless"], Mode::Both, Format::Stock(1), 3);
    assert_eq!(m.rounds().len(), 1);
    assert_eq!(m.rounds()[0].eliminated, names(&["reckless"]));
    assert!(m.is_over());
    assert_eq!(ranks(&m), [("steady".into(), 1), ("reckless".into(), 2)]);
    assert_eq!(standing(&m, "reckless").eliminated, Some(0));
}

/// Strikes: reckless is out after two consecutive failures and is dealt
/// nothing after that, while the other two play on. Nothing else ends a
/// strikes match between players who never fail, so the host ends it.
#[test]
fn a_strikes_match_stops_dealing_to_the_eliminated() {
    let settings = Settings { mode: Mode::Survival, format: Format::Strikes(2), default_limit: LIMIT };
    let mut m = Match::new(pool(), names(&["ace", "steady", "reckless"]), settings, 5).unwrap();
    for _ in 0..4 {
        let deal = m.deal().unwrap();
        let active: Vec<String> = m.active().into_iter().map(|p| p.0.clone()).collect();
        let seats = active.iter().map(|p| (p.as_str(), script(p, &deal.challenge))).collect();
        let outcome = play_within(&deal.challenge, seats, deal.limit);
        m.record(outcome.results.into_iter().map(|(p, r)| (PlayerId(p), r)).collect());
    }
    assert!(!m.is_over());
    assert_eq!(m.rounds()[1].eliminated, names(&["reckless"]));
    assert_eq!(m.rounds()[2].entries.len(), 2);
    assert_eq!(m.rounds()[3].entries.len(), 2);
    m.end();
    assert!(m.is_over());
    assert_eq!(ranks(&m), [("ace".into(), 1), ("steady".into(), 1), ("reckless".into(), 3)]);
}

/// Wins: ace is first on every time trial, so a first-to-three match is
/// exactly three rounds long.
#[test]
fn a_wins_match_ends_on_the_round_the_count_is_reached() {
    let m = play_match(&["ace", "steady", "reckless"], Mode::TimeTrial, Format::Wins(3), 9);
    assert_eq!(m.rounds().len(), 3);
    assert_eq!(standing(&m, "ace").wins, 3);
    assert_eq!(standing(&m, "steady").wins, 0);
    assert_eq!(ranks(&m), [("ace".into(), 1), ("steady".into(), 2), ("reckless".into(), 3)]);
}

/// The server and every client deal from the same seed, so the same seed
/// must mean the same challenges, the same content seeds and the same
/// standings everywhere.
#[test]
fn a_seed_means_the_same_match_everywhere() {
    let dealt = |m: &Match| -> Vec<(String, u64, u64)> {
        m.rounds().iter().map(|r| (r.deal.challenge.id.0.clone(), r.deal.seed, r.deal.limit)).collect()
    };
    let a = play_match(&["ace", "steady"], Mode::Both, Format::Challenges(8), 4242);
    let b = play_match(&["ace", "steady"], Mode::Both, Format::Challenges(8), 4242);
    assert_eq!(dealt(&a), dealt(&b));
    assert_eq!(a.standings(), b.standings());
    let c = play_match(&["ace", "steady"], Mode::Both, Format::Challenges(8), 4243);
    assert_ne!(dealt(&a), dealt(&c));
}

/// The library today holds fewer than eight challenges of some kinds, so a
/// longer match reshuffles: no challenge repeats within a deck, and the deck
/// boundary never deals one twice running.
#[test]
fn a_long_match_reshuffles_without_dealing_one_twice_running() {
    let m = play_match(&["ace"], Mode::TimeTrial, Format::Challenges(20), 1);
    let ids: Vec<String> = m.rounds().iter().map(|r| r.deal.challenge.id.0.clone()).collect();
    let deck = pool().iter().filter(|e| e.spec.kind == ChallengeKind::TimeTrial).count();
    assert!(deck < 20, "the library grew; pick a longer match");
    for chunk in ids.chunks(deck) {
        let mut unique = chunk.to_vec();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), chunk.len());
    }
    assert!(ids.windows(2).all(|w| w[0] != w[1]));
}

/// A deal names the round's limit and a content seed, which is what a client
/// needs to prepare the stage before anyone counts down.
#[test]
fn every_deal_carries_a_limit_and_a_seed() {
    let settings = Settings { mode: Mode::Both, format: Format::Challenges(1), default_limit: 60 };
    let mut m = Match::new(pool(), names(&["ace"]), settings, 0).unwrap();
    let deal: Deal = m.deal().unwrap();
    assert_eq!(deal.index, 0);
    assert_eq!(deal.limit, 60);
    assert_eq!(m.pending(), Some(&deal));
}
