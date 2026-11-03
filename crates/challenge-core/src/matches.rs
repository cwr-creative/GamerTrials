//! A match: a series of challenges, and what each player's runs added up to.
//!
//! A [`Match`] deals one challenge at a time, is told every active player's
//! [`RunResult`] for it, and answers with the round's reading and whether the
//! match is over. It never drives a runner and never judges a run — a
//! [`Run`](crate::run::Run) does that — so, like the rest of this crate, it is
//! tested with literal results.
//!
//! The rules are ADR 0026's, and each is a choice:
//!
//! * **Every run is bounded.** A round's limit is the entry's own
//!   (`limit_seconds` in the manifest) or the settings' default, and in any
//!   total a run that did not complete costs the full limit, forfeit included.
//!   Otherwise giving up early would be cheaper than playing on.
//! * **Three readings of a place.** Per round each player has a place (from
//!   [`places`]); a *success* is first place in a time trial or completing a
//!   survival challenge; a *failure* is not completing, or — time trial only —
//!   being in the bottom placement group when that group is not everyone, so
//!   that a solo player, or a field that ties, is not punished for finishing;
//!   *points* are the players placed behind you.
//! * **Each mode stands on its own figure.** Time trial: total frames,
//!   fewest first. Survival: completions, most first. Both: points, most
//!   first. Ties stand as shared ranks; nothing breaks them.
//! * **Four formats.** Play N challenges; the first to N successes; N
//!   consecutive failures eliminates; N failures eliminates. Eliminated
//!   players are dealt nothing further. An elimination match ends when fewer
//!   than two remain — or, with one player, when they are out.
//! * **A round nobody can prepare is void** (ADR 0029). It is recorded for no
//!   one, does not count toward a format, and its challenge is withdrawn for
//!   the rest of the match. A match with nothing left to deal is over.
//! * **A bout seats some and the rest back a slot** (ADR 0036). A
//!   head-to-head challenge is dealt only while the active players number at
//!   least its slots, and is passed over, not withdrawn, otherwise. Who sits
//!   is drawn from the match seed and the round index. Everyone else backs a
//!   seated player — by choice, or assigned when the caller says the time for
//!   choosing is up — and is recorded with a copy of that player's entry, so
//!   every format and standing reads a backer as it reads a competitor.
//!   Points are still players placed behind you, backers counted.

use std::collections::BTreeMap;
use std::fmt;

use session_api::{ChallengeId, ChallengeKind, ChallengeSpec, GameId, PlayerId};

use crate::deal::{derive, DealError, Dealer, Mode, SplitMix64, SLOTS};
use crate::rank::places;
use crate::run::RunResult;

/// How a match ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Format {
    /// Play this many challenges, then stand.
    Challenges(u32),
    /// This many consecutive failures puts a player out; last standing wins.
    Strikes(u32),
    /// The first to this many successes wins.
    Wins(u32),
    /// This many failures, consecutive or not, puts a player out.
    Stock(u32),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub mode: Mode,
    pub format: Format,
    /// The limit, in frames, for a challenge whose entry declares none.
    pub default_limit: u64,
}

/// One challenge the lobby enabled, with the limit its manifest hints at.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub spec: ChallengeSpec,
    /// In frames; `None` takes [`Settings::default_limit`].
    pub limit: Option<u64>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MatchError {
    Deal(DealError),
    NoPlayers,
    DuplicatePlayer(PlayerId),
    /// A count or a limit that was zero, named.
    Zero(&'static str),
    /// A head-to-head challenge seating fewer than two.
    TooFewSlots(ChallengeId),
    /// [`Match::back`] refused: no bout is pending, or the backer or the
    /// target is not who the call needs them to be.
    Backing(String),
}

impl fmt::Display for MatchError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MatchError::Deal(e) => write!(f, "{e}"),
            MatchError::NoPlayers => f.write_str("a match needs at least one player"),
            MatchError::DuplicatePlayer(p) => write!(f, "player `{p}` is listed twice"),
            MatchError::Zero(what) => write!(f, "{what} cannot be zero"),
            MatchError::TooFewSlots(c) => {
                write!(f, "head-to-head challenge `{c}` seats fewer than two slots")
            }
            MatchError::Backing(why) => f.write_str(why),
        }
    }
}

impl std::error::Error for MatchError {}

impl From<DealError> for MatchError {
    fn from(e: DealError) -> Self {
        MatchError::Deal(e)
    }
}

/// What the match hands out for one round.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Deal {
    /// Counted from 0 across the match.
    pub index: u64,
    pub challenge: ChallengeSpec,
    /// The seed a generated challenge is rolled from; the same for every player.
    pub seed: u64,
    /// The frame limit every run of this round takes (`Run::with_limit`).
    pub limit: u64,
    /// Who plays the round, in slot order: every active player for a time
    /// trial or a survival challenge; for a bout, the players drawn to sit,
    /// one per slot (ADR 0036). The rest of the active players back one of
    /// these.
    pub slots: Vec<PlayerId>,
}

impl Deal {
    pub fn is_bout(&self) -> bool {
        self.challenge.kind == ChallengeKind::HeadToHead
    }

    pub fn seats(&self, player: &PlayerId) -> bool {
        self.slots.contains(player)
    }
}

/// One player's reading of one round.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RoundEntry {
    pub player: PlayerId,
    /// A backer's is a copy of the backed player's.
    pub result: RunResult,
    /// The seated player this one backed, if this one did not play.
    pub backed: Option<PlayerId>,
    pub completed: bool,
    /// Standard competition ranking among the round's players.
    pub place: u32,
    /// What the run cost the total: its frames if completed, else the limit.
    pub frames: u64,
    pub success: bool,
    pub failure: bool,
    /// How many players placed behind this one.
    pub points: u32,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Round {
    pub deal: Deal,
    /// One per player who was active, in the match's player order.
    pub entries: Vec<RoundEntry>,
    /// Players this round put out of the match.
    pub eliminated: Vec<PlayerId>,
}

impl Round {
    pub fn entry(&self, player: &PlayerId) -> Option<&RoundEntry> {
        self.entries.iter().find(|e| &e.player == player)
    }
}

/// Where a player stands, with every figure the formats and modes read.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Standing {
    pub player: PlayerId,
    /// Standard competition ranking; ties share a rank.
    pub rank: u32,
    pub rounds: u32,
    pub frames: u64,
    pub completions: u32,
    pub points: u32,
    pub wins: u32,
    pub failures: u32,
    /// Consecutive failures, as of the last round played.
    pub streak: u32,
    /// The round that put this player out, if one did.
    pub eliminated: Option<u64>,
}

#[derive(Clone, Default, Debug)]
struct Tally {
    rounds: u32,
    frames: u64,
    completions: u32,
    points: u32,
    wins: u32,
    failures: u32,
    streak: u32,
    eliminated: Option<u64>,
}

#[derive(Clone, Debug)]
pub struct Match {
    settings: Settings,
    players: Vec<PlayerId>,
    dealer: Dealer,
    limits: BTreeMap<(GameId, ChallengeId), u64>,
    pending: Option<Deal>,
    /// Backer to seated player, for the pending bout.
    backing: BTreeMap<PlayerId, PlayerId>,
    rounds: Vec<Round>,
    tallies: Vec<Tally>,
    ended: bool,
}

impl Match {
    pub fn new(
        pool: Vec<Entry>,
        players: Vec<PlayerId>,
        settings: Settings,
        seed: u64,
    ) -> Result<Match, MatchError> {
        match settings.format {
            Format::Challenges(0) => return Err(MatchError::Zero("the number of challenges")),
            Format::Strikes(0) => return Err(MatchError::Zero("the number of strikes")),
            Format::Wins(0) => return Err(MatchError::Zero("the number of wins")),
            Format::Stock(0) => return Err(MatchError::Zero("the stock")),
            _ => {}
        }
        if settings.default_limit == 0 {
            return Err(MatchError::Zero("the default limit"));
        }
        if players.is_empty() {
            return Err(MatchError::NoPlayers);
        }
        for (i, p) in players.iter().enumerate() {
            if players[..i].contains(p) {
                return Err(MatchError::DuplicatePlayer(p.clone()));
            }
        }
        let mut limits = BTreeMap::new();
        let mut specs = Vec::with_capacity(pool.len());
        for entry in pool {
            if let Some(limit) = entry.limit {
                if limit == 0 {
                    return Err(MatchError::Zero("a challenge's limit"));
                }
                limits.insert((entry.spec.game.clone(), entry.spec.id.clone()), limit);
            }
            if entry.spec.kind == ChallengeKind::HeadToHead && entry.spec.slots < 2 {
                return Err(MatchError::TooFewSlots(entry.spec.id));
            }
            specs.push(entry.spec);
        }
        let dealer = Dealer::new(specs, settings.mode, seed)?;
        let tallies = vec![Tally::default(); players.len()];
        Ok(Match {
            settings,
            players,
            dealer,
            limits,
            pending: None,
            backing: BTreeMap::new(),
            rounds: Vec::new(),
            tallies,
            ended: false,
        })
    }

    pub fn settings(&self) -> Settings {
        self.settings
    }

    pub fn players(&self) -> &[PlayerId] {
        &self.players
    }

    /// The players still in the match, in player order.
    pub fn active(&self) -> Vec<&PlayerId> {
        self.players
            .iter()
            .zip(&self.tallies)
            .filter(|(_, t)| t.eliminated.is_none())
            .map(|(p, _)| p)
            .collect()
    }

    pub fn rounds(&self) -> &[Round] {
        &self.rounds
    }

    /// The round dealt and not yet recorded, if any.
    pub fn pending(&self) -> Option<&Deal> {
        self.pending.as_ref()
    }

    pub fn is_over(&self) -> bool {
        if self.ended || self.dealer.exhausted() || !self.anything_playable() {
            return true;
        }
        match self.settings.format {
            Format::Challenges(n) => self.rounds.len() >= n as usize,
            Format::Wins(n) => self.tallies.iter().any(|t| t.wins >= n),
            Format::Strikes(_) | Format::Stock(_) => {
                let needed = self.players.len().min(2);
                self.active().len() < needed
            }
        }
    }

    /// Ends the match where it stands — the host's "End match". Idempotent.
    pub fn end(&mut self) {
        self.ended = true;
        self.pending = None;
        self.backing.clear();
    }

    /// Whether `spec` can be dealt to the players active now: a bout needs
    /// at least as many of them as it has slots (ADR 0036).
    fn playable(spec: &ChallengeSpec, active: usize) -> bool {
        spec.kind != ChallengeKind::HeadToHead || usize::from(spec.slots) <= active
    }

    fn anything_playable(&self) -> bool {
        let active = self.active().len();
        self.dealer.remaining().any(|c| Match::playable(c, active))
    }

    /// Drops the pending round unplayed, because a player could not prepare
    /// it, and withdraws its challenge for the rest of the match. Nothing is
    /// recorded: it costs nobody anything and counts toward no format. Its
    /// index is not reused, so a match's round indices can have gaps.
    ///
    /// # Panics
    ///
    /// If no round is pending.
    pub fn void(&mut self) -> Deal {
        let deal = self.pending.take().expect("no round is pending");
        self.backing.clear();
        self.dealer.withdraw(&deal.challenge.game, &deal.challenge.id);
        deal
    }

    /// Deals the next round, or `None` once the match is over.
    ///
    /// # Panics
    ///
    /// If the previous round has not been recorded.
    pub fn deal(&mut self) -> Option<Deal> {
        if self.is_over() {
            return None;
        }
        assert!(self.pending.is_none(), "the previous round has not been recorded");
        let active: Vec<PlayerId> = self.active().into_iter().cloned().collect();
        let dealt = self.dealer.deal_where(|c| Match::playable(c, active.len()))?;
        let limit = self
            .limits
            .get(&(dealt.challenge.game.clone(), dealt.challenge.id.clone()))
            .copied()
            .unwrap_or(self.settings.default_limit);
        let slots = if dealt.challenge.kind == ChallengeKind::HeadToHead {
            draw(&active, usize::from(dealt.challenge.slots), self.dealer.seed(), dealt.index)
        } else {
            active
        };
        let deal =
            Deal { index: dealt.index, challenge: dealt.challenge, seed: dealt.seed, limit, slots };
        self.backing.clear();
        self.pending = Some(deal.clone());
        Some(deal)
    }

    /// `backer`, active and not seated in the pending bout, throws in their
    /// lot with `target`, who is. Picking again replaces the pick.
    pub fn back(&mut self, backer: PlayerId, target: PlayerId) -> Result<(), MatchError> {
        let refuse = |why: String| Err(MatchError::Backing(why));
        let Some(deal) = self.pending.as_ref().filter(|d| d.is_bout()) else {
            return refuse("no bout is pending".into());
        };
        if !self.active().contains(&&backer) {
            return refuse(format!("{backer} is not in the match"));
        }
        if deal.seats(&backer) {
            return refuse(format!("{backer} is playing this bout"));
        }
        if !deal.seats(&target) {
            return refuse(format!("{target} is not playing this bout"));
        }
        self.backing.insert(backer, target);
        Ok(())
    }

    /// Who each backer backs, for the pending bout.
    pub fn backing(&self) -> &BTreeMap<PlayerId, PlayerId> {
        &self.backing
    }

    /// Active players neither seated in the pending bout nor backing anyone
    /// yet, in player order. Empty when no bout is pending.
    pub fn unbacked(&self) -> Vec<PlayerId> {
        let Some(deal) = self.pending.as_ref().filter(|d| d.is_bout()) else { return Vec::new() };
        self.active()
            .into_iter()
            .filter(|p| !deal.seats(p) && !self.backing.contains_key(*p))
            .cloned()
            .collect()
    }

    /// Time is up: everyone [unbacked](Match::unbacked) is given a seated
    /// player, drawn from `seed`. The caller supplies the seed because this
    /// draw depends on which picks arrived, so it is nobody's to reproduce.
    pub fn assign_backers(&mut self, seed: u64) {
        let Some(slots) = self.pending.as_ref().filter(|d| d.is_bout()).map(|d| d.slots.clone())
        else {
            return;
        };
        let mut rng = SplitMix64(seed);
        for backer in self.unbacked() {
            let target = slots[rng.below(slots.len() as u64) as usize].clone();
            self.backing.insert(backer, target);
        }
    }

    /// Records every seated player's result for the pending round and reads
    /// it. For a bout, every other active player must be backing someone.
    ///
    /// # Panics
    ///
    /// If no round is pending, if the results are not exactly one per seated
    /// player, if a result is for another challenge, or if a bout has an
    /// active player neither seated nor backing. Each is the caller's bug,
    /// and a silently misread round would be worse than stopping.
    pub fn record(&mut self, results: Vec<(PlayerId, RunResult)>) -> &Round {
        let deal = self.pending.take().expect("no round is pending");
        let backing = std::mem::take(&mut self.backing);
        let active: Vec<PlayerId> = self.active().into_iter().cloned().collect();
        let seated = &deal.slots;
        assert_eq!(
            results.len(),
            seated.len(),
            "round {} needs one result per seated player",
            deal.index
        );
        let mut ordered: Vec<RunResult> = Vec::with_capacity(seated.len());
        for player in seated {
            let result = results
                .iter()
                .filter(|(p, _)| p == player)
                .map(|(_, r)| r.clone())
                .next()
                .unwrap_or_else(|| panic!("no result for `{player}` in round {}", deal.index));
            assert!(
                result.challenge == deal.challenge.id && result.kind == deal.challenge.kind,
                "`{player}` reported {} for a round of {}",
                result.challenge,
                deal.challenge.id
            );
            ordered.push(result);
        }

        // Every active player's side: themselves, or whom they backed.
        let side: Vec<usize> = active
            .iter()
            .map(|p| {
                let seat = seated.iter().position(|s| s == p).or_else(|| {
                    backing.get(p).and_then(|t| seated.iter().position(|s| s == t))
                });
                seat.unwrap_or_else(|| {
                    panic!("`{p}` is neither seated in round {} nor backing anyone", deal.index)
                })
            })
            .collect();

        let seat_places = places(&ordered);
        let places: Vec<u32> = side.iter().map(|&s| seat_places[s]).collect();
        let bottom = places.iter().copied().max().unwrap_or(1);
        let everyone_ties = places.iter().all(|&p| p == 1);
        let mut entries = Vec::with_capacity(active.len());
        for (i, player) in active.iter().enumerate() {
            let result = &ordered[side[i]];
            let completed = result.completed();
            let place = places[i];
            let (success, failure) = match deal.challenge.kind {
                ChallengeKind::TimeTrial => (
                    completed && place == 1,
                    !completed || (place == bottom && !everyone_ties),
                ),
                ChallengeKind::Survival | ChallengeKind::HeadToHead => (completed, !completed),
            };
            entries.push(RoundEntry {
                player: player.clone(),
                result: result.clone(),
                backed: backing.get(player).cloned(),
                completed,
                place,
                frames: if completed { result.frames } else { deal.limit },
                success,
                failure,
                points: places.iter().filter(|&&p| p > place).count() as u32,
            });
        }

        let mut eliminated = Vec::new();
        for entry in &entries {
            let i = self.players.iter().position(|p| p == &entry.player).expect("active");
            let t = &mut self.tallies[i];
            t.rounds += 1;
            t.frames += entry.frames;
            t.completions += entry.completed as u32;
            t.points += entry.points;
            t.wins += entry.success as u32;
            if entry.failure {
                t.failures += 1;
                t.streak += 1;
            } else {
                t.streak = 0;
            }
            let out = match self.settings.format {
                Format::Strikes(n) => t.streak >= n,
                Format::Stock(n) => t.failures >= n,
                Format::Challenges(_) | Format::Wins(_) => false,
            };
            if out {
                t.eliminated = Some(deal.index);
                eliminated.push(entry.player.clone());
            }
        }

        self.rounds.push(Round { deal, entries, eliminated });
        self.rounds.last().expect("just pushed")
    }

    /// Every player, best first, with ties sharing a rank.
    pub fn standings(&self) -> Vec<Standing> {
        let keys: Vec<(u8, u64, i128, i128)> = self.tallies.iter().map(|t| self.key(t)).collect();
        let mut standings: Vec<Standing> = self
            .players
            .iter()
            .zip(&self.tallies)
            .enumerate()
            .map(|(i, (player, t))| Standing {
                player: player.clone(),
                rank: 1 + keys.iter().filter(|k| **k < keys[i]).count() as u32,
                rounds: t.rounds,
                frames: t.frames,
                completions: t.completions,
                points: t.points,
                wins: t.wins,
                failures: t.failures,
                streak: t.streak,
                eliminated: t.eliminated,
            })
            .collect();
        // Stable, so tied players keep the match's player order.
        standings.sort_by_key(|s| s.rank);
        standings
    }

    /// The players sharing first place.
    pub fn leaders(&self) -> Vec<PlayerId> {
        self.standings().into_iter().filter(|s| s.rank == 1).map(|s| s.player).collect()
    }

    /// Lower is better. Elimination formats put the eliminated below the
    /// active, later eliminations first; Wins puts the most successes first;
    /// then the mode's own figure, and nothing after it.
    fn key(&self, t: &Tally) -> (u8, u64, i128, i128) {
        let (tier, when) = match (self.settings.format, t.eliminated) {
            (Format::Strikes(_) | Format::Stock(_), Some(round)) => (1, u64::MAX - round),
            _ => (0, 0),
        };
        let wins = match self.settings.format {
            Format::Wins(_) => -(t.wins as i128),
            _ => 0,
        };
        let figure = match self.settings.mode {
            Mode::TimeTrial => t.frames as i128,
            Mode::Survival => -(t.completions as i128),
            Mode::Both => -(t.points as i128),
        };
        (tier, when, wins, figure)
    }
}

/// Which `n` of `active` sit in a bout's slots, in slot order: a partial
/// Fisher-Yates under a seed derived from the match seed and the round
/// index, so whoever holds the seed can say who sat (ADR 0036).
fn draw(active: &[PlayerId], n: usize, seed: u64, index: u64) -> Vec<PlayerId> {
    let mut pool: Vec<PlayerId> = active.to_vec();
    let n = n.min(pool.len());
    let mut rng = SplitMix64(derive(seed, SLOTS, index));
    for i in 0..n {
        let j = i + rng.below((pool.len() - i) as u64) as usize;
        pool.swap(i, j);
    }
    pool.truncate(n);
    pool
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{AttemptEnd, Ending};
    use session_api::{Completion, StageId};

    const LIMIT: u64 = 1000;

    fn spec(game: &str, id: &str, kind: ChallengeKind) -> ChallengeSpec {
        ChallengeSpec {
            id: ChallengeId(id.into()),
            name: id.into(),
            game: GameId(game.into()),
            stage: StageId(id.into()),
            kind,
            complete_when: Completion { cleared: true, ..Default::default() },
            ends_attempt_on: Default::default(),
            slots: if kind == ChallengeKind::HeadToHead { 2 } else { 1 },
        }
    }

    fn pool(kind: ChallengeKind, n: usize) -> Vec<Entry> {
        (0..n).map(|i| Entry { spec: spec("g", &format!("c{i}"), kind), limit: None }).collect()
    }

    fn players(names: &[&str]) -> Vec<PlayerId> {
        names.iter().map(|n| PlayerId(n.to_string())).collect()
    }

    fn settings(mode: Mode, format: Format) -> Settings {
        Settings { mode, format, default_limit: LIMIT }
    }

    fn result(deal: &Deal, ending: Ending, frames: u64) -> RunResult {
        RunResult {
            challenge: deal.challenge.id.clone(),
            kind: deal.challenge.kind,
            ending,
            frames,
            attempts: 1,
        }
    }

    fn done(deal: &Deal, frames: u64) -> RunResult {
        result(deal, Ending::Completed, frames)
    }

    fn died(deal: &Deal) -> RunResult {
        result(deal, Ending::Failed(AttemptEnd::Died), 300)
    }

    /// Plays one round: `outcomes` is one result per active player, in order.
    fn play(m: &mut Match, outcomes: impl Fn(&Deal, usize) -> RunResult) -> Round {
        let deal = m.deal().expect("the match is over");
        let active: Vec<PlayerId> = m.active().into_iter().cloned().collect();
        let results =
            active.iter().enumerate().map(|(i, p)| (p.clone(), outcomes(&deal, i))).collect();
        m.record(results).clone()
    }

    fn ranks(m: &Match) -> Vec<(String, u32)> {
        m.standings().into_iter().map(|s| (s.player.0, s.rank)).collect()
    }

    #[test]
    fn a_challenges_match_plays_n_rounds_and_stands_on_total_frames() {
        let mut m = Match::new(
            pool(ChallengeKind::TimeTrial, 4),
            players(&["ann", "bob", "cy"]),
            settings(Mode::TimeTrial, Format::Challenges(2)),
            1,
        )
        .unwrap();
        play(&mut m, |d, i| done(d, [500, 400, 600][i]));
        assert!(!m.is_over());
        play(&mut m, |d, i| done(d, [100, 300, 200][i]));
        assert!(m.is_over());
        assert_eq!(m.deal(), None);
        assert_eq!(ranks(&m), [("ann".into(), 1), ("bob".into(), 2), ("cy".into(), 3)]);
        assert_eq!(m.standings()[0].frames, 600);
        assert_eq!(m.rounds().len(), 2);
    }

    #[test]
    fn a_run_that_did_not_complete_costs_the_full_limit_forfeit_included() {
        let mut m = Match::new(
            pool(ChallengeKind::TimeTrial, 2),
            players(&["quits", "stalls", "finishes"]),
            settings(Mode::TimeTrial, Format::Challenges(1)),
            1,
        )
        .unwrap();
        let round = play(&mut m, |d, i| match i {
            0 => result(d, Ending::Forfeited, 50),
            1 => result(d, Ending::TimedOut, LIMIT),
            _ => done(d, 900),
        });
        assert_eq!(round.entries[0].frames, LIMIT);
        assert_eq!(round.entries[1].frames, LIMIT);
        assert_eq!(round.entries[2].frames, 900);
        assert_eq!(ranks(&m), [("finishes".into(), 1), ("quits".into(), 2), ("stalls".into(), 2)]);
    }

    #[test]
    fn a_limit_comes_from_the_entry_else_the_default() {
        let mut entries = pool(ChallengeKind::Survival, 2);
        entries[0].limit = Some(7);
        let mut m =
            Match::new(entries, players(&["a"]), settings(Mode::Survival, Format::Challenges(2)), 0)
                .unwrap();
        let mut limits = Vec::new();
        for _ in 0..2 {
            let deal = m.deal().unwrap();
            limits.push((deal.challenge.id.0.clone(), deal.limit));
            let r = died(&deal);
            m.record(vec![(PlayerId("a".into()), r)]);
        }
        limits.sort();
        assert_eq!(limits, [("c0".to_string(), 7), ("c1".to_string(), LIMIT)]);
    }

    #[test]
    fn points_are_the_players_placed_behind_you() {
        let mut m = Match::new(
            [pool(ChallengeKind::TimeTrial, 1), pool(ChallengeKind::Survival, 1)].concat(),
            players(&["a", "b", "c", "d"]),
            settings(Mode::Both, Format::Challenges(2)),
            3,
        )
        .unwrap();
        let first = play(&mut m, |d, i| match (d.challenge.kind, i) {
            (ChallengeKind::TimeTrial, 0) => done(d, 100),
            (ChallengeKind::TimeTrial, 1) => done(d, 200),
            (ChallengeKind::TimeTrial, _) => result(d, Ending::TimedOut, LIMIT),
            (ChallengeKind::Survival, 0 | 1) => done(d, 500),
            (ChallengeKind::Survival, _) => died(d),
            (ChallengeKind::HeadToHead, _) => unreachable!("not in this pool"),
        });
        let points: Vec<u32> = first.entries.iter().map(|e| e.points).collect();
        match first.deal.challenge.kind {
            ChallengeKind::TimeTrial => assert_eq!(points, [3, 2, 0, 0]),
            ChallengeKind::Survival => assert_eq!(points, [2, 2, 0, 0]),
            ChallengeKind::HeadToHead => unreachable!("not in this pool"),
        }
    }

    #[test]
    fn survival_finishers_all_succeed_and_a_wins_match_ends_tied() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 3),
            players(&["a", "b", "c"]),
            settings(Mode::Survival, Format::Wins(1)),
            0,
        )
        .unwrap();
        let round = play(&mut m, |d, i| if i < 2 { done(d, 400) } else { died(d) });
        assert_eq!(round.entries.iter().map(|e| e.success).collect::<Vec<_>>(), [true, true, false]);
        assert!(m.is_over());
        assert_eq!(ranks(&m), [("a".into(), 1), ("b".into(), 1), ("c".into(), 3)]);
        assert_eq!(m.leaders(), players(&["a", "b"]));
    }

    #[test]
    fn a_time_trial_success_is_first_place_among_finishers() {
        let mut m = Match::new(
            pool(ChallengeKind::TimeTrial, 3),
            players(&["a", "b"]),
            settings(Mode::TimeTrial, Format::Wins(2)),
            0,
        )
        .unwrap();
        // Nobody finishing is nobody's success, even though places tie at 1.
        let round = play(&mut m, |d, _| result(d, Ending::TimedOut, LIMIT));
        assert!(round.entries.iter().all(|e| !e.success && e.failure));
        play(&mut m, |d, i| done(d, [300, 200][i]));
        assert!(!m.is_over());
        play(&mut m, |d, i| done(d, [300, 200][i]));
        assert!(m.is_over());
        assert_eq!(ranks(&m), [("b".into(), 1), ("a".into(), 2)]);
    }

    #[test]
    fn everyone_timing_out_strikes_everyone_and_the_match_ends_tied() {
        let mut m = Match::new(
            pool(ChallengeKind::TimeTrial, 3),
            players(&["a", "b", "c"]),
            settings(Mode::TimeTrial, Format::Strikes(1)),
            0,
        )
        .unwrap();
        let round = play(&mut m, |d, _| result(d, Ending::TimedOut, LIMIT));
        assert_eq!(round.eliminated, players(&["a", "b", "c"]));
        assert!(m.is_over());
        assert!(m.standings().iter().all(|s| s.rank == 1 && s.eliminated == Some(0)));
    }

    #[test]
    fn a_solo_time_trial_never_strikes_for_a_completed_run() {
        let mut m = Match::new(
            pool(ChallengeKind::TimeTrial, 3),
            players(&["solo"]),
            settings(Mode::TimeTrial, Format::Strikes(1)),
            0,
        )
        .unwrap();
        let round = play(&mut m, |d, _| done(d, 500));
        assert!(!round.entries[0].failure && round.entries[0].success);
        assert!(!m.is_over(), "one player left is the normal state of a solo match");
        let round = play(&mut m, |d, _| result(d, Ending::Forfeited, 10));
        assert!(round.entries[0].failure);
        assert!(m.is_over());
        assert_eq!(m.standings()[0].eliminated, Some(1));
        assert_eq!(m.standings()[0].rounds, 2);
    }

    #[test]
    fn the_slowest_finisher_strikes_only_when_not_everyone_ties() {
        let mut m = Match::new(
            pool(ChallengeKind::TimeTrial, 3),
            players(&["a", "b"]),
            settings(Mode::TimeTrial, Format::Strikes(3)),
            0,
        )
        .unwrap();
        let round = play(&mut m, |d, i| done(d, [200, 300][i]));
        assert_eq!(round.entries.iter().map(|e| e.failure).collect::<Vec<_>>(), [false, true]);
        let round = play(&mut m, |d, _| done(d, 250));
        assert!(round.entries.iter().all(|e| !e.failure && e.success));
    }

    #[test]
    fn stock_counts_every_failure_and_strikes_only_a_streak() {
        let script = |d: &Deal, i: usize| -> RunResult {
            // Player 0 goes fail, succeed, fail; player 1 always succeeds.
            if i == 1 {
                return done(d, 100);
            }
            match d.index {
                1 => done(d, 50),
                _ => died(d),
            }
        };
        let mut stock = Match::new(
            pool(ChallengeKind::Survival, 4),
            players(&["flaky", "steady"]),
            settings(Mode::Survival, Format::Stock(2)),
            0,
        )
        .unwrap();
        let mut strikes = Match::new(
            pool(ChallengeKind::Survival, 4),
            players(&["flaky", "steady"]),
            settings(Mode::Survival, Format::Strikes(2)),
            0,
        )
        .unwrap();
        for _ in 0..3 {
            play(&mut stock, script);
            play(&mut strikes, script);
        }
        assert!(stock.is_over());
        assert_eq!(ranks(&stock), [("steady".into(), 1), ("flaky".into(), 2)]);
        assert_eq!(stock.standings()[1].eliminated, Some(2));
        assert!(!strikes.is_over());
        assert_eq!(strikes.standings()[1].streak, 1);
        assert_eq!(strikes.standings()[1].failures, 2);
    }

    #[test]
    fn eliminated_players_are_dealt_nothing_and_rank_below_the_active() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 4),
            players(&["a", "b", "c"]),
            settings(Mode::Survival, Format::Stock(1)),
            0,
        )
        .unwrap();
        let round = play(&mut m, |d, i| if i == 1 { died(d) } else { done(d, 100) });
        assert_eq!(round.eliminated, players(&["b"]));
        assert_eq!(m.active(), [&PlayerId("a".into()), &PlayerId("c".into())]);
        assert!(!m.is_over());
        let round = play(&mut m, |d, i| if i == 0 { died(d) } else { done(d, 100) });
        assert_eq!(round.entries.len(), 2);
        assert_eq!(round.eliminated, players(&["a"]));
        assert!(m.is_over());
        // Later out ranks above earlier out; the survivor above both.
        assert_eq!(ranks(&m), [("c".into(), 1), ("a".into(), 2), ("b".into(), 3)]);
    }

    #[test]
    fn the_host_can_end_a_match_where_it_stands() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 4),
            players(&["a", "b"]),
            settings(Mode::Survival, Format::Wins(5)),
            0,
        )
        .unwrap();
        play(&mut m, |d, i| if i == 0 { done(d, 100) } else { died(d) });
        m.deal().unwrap();
        m.end();
        assert!(m.is_over());
        assert_eq!(m.deal(), None);
        assert_eq!(m.pending(), None);
        assert_eq!(ranks(&m), [("a".into(), 1), ("b".into(), 2)]);
    }

    #[test]
    fn a_seed_deals_the_same_match_and_the_same_content_seeds() {
        let deals = |seed| {
            let mut m = Match::new(
                pool(ChallengeKind::Survival, 5),
                players(&["a"]),
                settings(Mode::Survival, Format::Challenges(5)),
                seed,
            )
            .unwrap();
            let mut out = Vec::new();
            while let Some(deal) = m.deal() {
                out.push((deal.challenge.id.0.clone(), deal.seed));
                let r = died(&deal);
                m.record(vec![(PlayerId("a".into()), r)]);
            }
            out
        };
        assert_eq!(deals(77), deals(77));
        assert_ne!(deals(77), deals(78));
    }

    #[test]
    fn bad_settings_are_refused() {
        let err = |s: Settings| {
            Match::new(pool(ChallengeKind::Survival, 1), players(&["a"]), s, 0).unwrap_err()
        };
        assert_eq!(err(settings(Mode::Survival, Format::Stock(0))), MatchError::Zero("the stock"));
        assert_eq!(
            err(Settings { default_limit: 0, ..settings(Mode::Survival, Format::Wins(1)) }),
            MatchError::Zero("the default limit")
        );
        assert!(matches!(
            err(settings(Mode::TimeTrial, Format::Wins(1))),
            MatchError::Deal(DealError::NotEnough { .. })
        ));
        let dup = Match::new(
            pool(ChallengeKind::Survival, 1),
            players(&["a", "a"]),
            settings(Mode::Survival, Format::Wins(1)),
            0,
        )
        .unwrap_err();
        assert_eq!(dup, MatchError::DuplicatePlayer(PlayerId("a".into())));
        let none =
            Match::new(pool(ChallengeKind::Survival, 1), vec![], settings(Mode::Survival, Format::Wins(1)), 0)
                .unwrap_err();
        assert_eq!(none, MatchError::NoPlayers);
    }

    #[test]
    #[should_panic(expected = "no round is pending")]
    fn recording_without_a_deal_is_a_bug() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 1),
            players(&["a"]),
            settings(Mode::Survival, Format::Wins(1)),
            0,
        )
        .unwrap();
        m.record(vec![]);
    }

    #[test]
    #[should_panic(expected = "has not been recorded")]
    fn dealing_twice_is_a_bug() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 1),
            players(&["a"]),
            settings(Mode::Survival, Format::Wins(1)),
            0,
        )
        .unwrap();
        m.deal();
        m.deal();
    }

    #[test]
    fn a_voided_round_is_not_counted() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 4),
            players(&["a", "b"]),
            settings(Mode::Survival, Format::Challenges(2)),
            3,
        )
        .unwrap();
        let voided = m.deal().unwrap();
        assert_eq!(m.void(), voided);
        assert_eq!(m.pending(), None);
        assert!(m.rounds().is_empty());
        assert!(m.standings().iter().all(|s| s.rounds == 0 && s.failures == 0));
        let r1 = play(&mut m, |d, _| done(d, 10));
        assert_eq!(r1.deal.index, voided.index + 1, "the voided index is skipped, not reused");
        assert!(!m.is_over(), "the void did not count toward the two");
        play(&mut m, |d, _| done(d, 10));
        assert!(m.is_over());
    }

    #[test]
    fn a_voided_challenge_is_not_dealt_again() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 3),
            players(&["a"]),
            settings(Mode::Survival, Format::Challenges(12)),
            8,
        )
        .unwrap();
        let voided = m.deal().unwrap().challenge.id;
        m.void();
        for _ in 0..12 {
            let r = play(&mut m, |d, _| done(d, 10));
            assert_ne!(r.deal.challenge.id, voided);
        }
    }

    #[test]
    fn voiding_every_challenge_ends_the_match() {
        let mut m = Match::new(
            pool(ChallengeKind::Survival, 2),
            players(&["a"]),
            settings(Mode::Survival, Format::Wins(3)),
            0,
        )
        .unwrap();
        play(&mut m, |d, _| done(d, 10));
        m.deal().unwrap();
        m.void();
        assert!(!m.is_over());
        m.deal().unwrap();
        m.void();
        assert!(m.is_over());
        assert_eq!(m.deal(), None);
        assert_eq!(m.standings()[0].wins, 1, "what was played still stands");
    }

    fn beaten(deal: &Deal, frames: u64) -> RunResult {
        result(deal, Ending::Failed(AttemptEnd::Beaten), frames)
    }

    fn bout_pool(n: usize) -> Vec<Entry> {
        (0..n)
            .map(|i| Entry {
                spec: spec("sf2", &format!("fight{i}"), ChallengeKind::HeadToHead),
                limit: None,
            })
            .collect()
    }

    /// Three players, two slots: the drawn two fight, the third backs one.
    #[test]
    fn a_bout_seats_two_from_the_seed_and_the_backer_takes_their_sides_entry() {
        let mut m = Match::new(
            bout_pool(3),
            players(&["ann", "bob", "cy"]),
            settings(Mode::Both, Format::Challenges(1)),
            5,
        )
        .unwrap();
        let deal = m.deal().unwrap();
        assert!(deal.is_bout());
        assert_eq!(deal.slots.len(), 2);
        let backer = m.unbacked();
        assert_eq!(backer.len(), 1);
        let backer = backer[0].clone();
        assert!(!deal.seats(&backer));

        // Reproducible from the seed: the same match deals the same slots.
        let again = Match::new(
            bout_pool(3),
            players(&["ann", "bob", "cy"]),
            settings(Mode::Both, Format::Challenges(1)),
            5,
        )
        .unwrap()
        .deal()
        .unwrap();
        assert_eq!(again.slots, deal.slots);

        let (winner, loser) = (deal.slots[0].clone(), deal.slots[1].clone());
        m.back(backer.clone(), loser.clone()).unwrap();
        assert!(m.unbacked().is_empty());
        let round = m
            .record(vec![(winner.clone(), done(&deal, 2000)), (loser.clone(), beaten(&deal, 2000))])
            .clone();
        assert_eq!(round.entries.len(), 3);
        let of = |p: &PlayerId| round.entry(p).unwrap().clone();
        assert_eq!(of(&winner).place, 1);
        assert_eq!(of(&winner).points, 2, "two players placed behind the winner");
        assert_eq!(of(&loser).place, 2);
        assert!(of(&loser).failure);
        let b = of(&backer);
        assert_eq!(b.backed, Some(loser.clone()));
        assert_eq!((b.place, b.completed, b.success, b.failure, b.points), (2, false, false, true, 0));
        assert_eq!(b.frames, LIMIT, "a beaten side costs the limit, backer included");
        assert_eq!(b.result, of(&loser).result);
        assert_eq!(m.standings().iter().find(|s| s.player == winner).unwrap().points, 2);
    }

    #[test]
    fn backing_the_winner_in_a_wins_match_is_a_win_and_the_loser_in_stock_a_strike() {
        let mut m = Match::new(
            bout_pool(2),
            players(&["ann", "bob", "cy"]),
            settings(Mode::Survival, Format::Stock(1)),
            9,
        )
        .unwrap();
        let deal = m.deal().unwrap();
        let backer = m.unbacked()[0].clone();
        let (winner, loser) = (deal.slots[0].clone(), deal.slots[1].clone());
        m.back(backer.clone(), winner.clone()).unwrap();
        let round = m
            .record(vec![(winner.clone(), done(&deal, 100)), (loser.clone(), beaten(&deal, 100))])
            .clone();
        assert_eq!(round.eliminated, vec![loser.clone()]);
        assert!(round.entry(&backer).unwrap().success);
        let wins: Vec<(String, u32)> =
            m.standings().into_iter().map(|s| (s.player.0, s.wins)).collect();
        assert!(wins.iter().filter(|(_, w)| *w == 1).count() == 2, "{wins:?}");
    }

    #[test]
    fn unbacked_players_are_assigned_when_time_is_up() {
        let mut m = Match::new(
            bout_pool(2),
            players(&["a", "b", "c", "d", "e"]),
            settings(Mode::Both, Format::Challenges(1)),
            3,
        )
        .unwrap();
        let deal = m.deal().unwrap();
        assert_eq!(m.unbacked().len(), 3);
        let first = m.unbacked()[0].clone();
        m.back(first.clone(), deal.slots[1].clone()).unwrap();
        m.assign_backers(77);
        assert!(m.unbacked().is_empty());
        assert_eq!(m.backing()[&first], deal.slots[1], "a pick made stands");
        assert!(m.backing().values().all(|t| deal.seats(t)));
    }

    #[test]
    fn backing_is_refused_for_the_wrong_people() {
        let mut m = Match::new(
            bout_pool(2),
            players(&["a", "b", "c"]),
            settings(Mode::Both, Format::Challenges(1)),
            3,
        )
        .unwrap();
        let p = |s: &str| PlayerId(s.into());
        assert!(matches!(m.back(p("a"), p("b")), Err(MatchError::Backing(_))), "no bout yet");
        let deal = m.deal().unwrap();
        let backer = m.unbacked()[0].clone();
        let seated = deal.slots[0].clone();
        assert!(m.back(seated.clone(), deal.slots[1].clone()).is_err(), "seated players play");
        assert!(m.back(backer.clone(), backer.clone()).is_err(), "a backer is not seated");
        assert!(m.back(p("zed"), seated.clone()).is_err(), "not in the match");
        assert!(m.back(backer, seated).is_ok());
    }

    #[test]
    #[should_panic(expected = "neither seated")]
    fn recording_a_bout_with_an_unbacked_player_is_a_caller_bug() {
        let mut m = Match::new(
            bout_pool(2),
            players(&["a", "b", "c"]),
            settings(Mode::Both, Format::Challenges(1)),
            3,
        )
        .unwrap();
        let deal = m.deal().unwrap();
        m.record(vec![
            (deal.slots[0].clone(), done(&deal, 1)),
            (deal.slots[1].clone(), beaten(&deal, 1)),
        ]);
    }

    /// Two players, two slots: everyone sits, nobody backs, and a draw at
    /// the limit is everyone failing (ADR 0026).
    #[test]
    fn a_two_player_bout_has_no_backers_and_a_draw_fails_both() {
        let mut m = Match::new(
            bout_pool(2),
            players(&["a", "b"]),
            settings(Mode::Both, Format::Challenges(1)),
            3,
        )
        .unwrap();
        let deal = m.deal().unwrap();
        assert!(m.unbacked().is_empty());
        let round = play_this(&mut m, &deal, |d, _| result(d, Ending::TimedOut, LIMIT));
        assert!(round.entries.iter().all(|e| e.failure && e.backed.is_none() && e.place == 1));
    }

    fn play_this(m: &mut Match, deal: &Deal, outcomes: impl Fn(&Deal, usize) -> RunResult) -> Round {
        let results =
            deal.slots.iter().enumerate().map(|(i, p)| (p.clone(), outcomes(deal, i))).collect();
        m.record(results).clone()
    }

    /// A bout that seats more than are active is passed over, not withdrawn;
    /// with nothing else in the pool the match is over.
    #[test]
    fn a_bout_with_too_many_slots_is_passed_over() {
        let mut four = spec("smk", "battle", ChallengeKind::HeadToHead);
        four.slots = 4;
        let mut pool = pool(ChallengeKind::Survival, 1);
        pool.push(Entry { spec: four.clone(), limit: None });
        let mut m = Match::new(
            pool,
            players(&["a", "b", "c"]),
            settings(Mode::Both, Format::Challenges(3)),
            1,
        )
        .unwrap();
        for _ in 0..3 {
            let deal = m.deal().unwrap();
            assert_ne!(deal.challenge.id, four.id, "three cannot fill four slots");
            play_this(&mut m, &deal, |d, _| done(d, 10));
        }
        let mut only = Match::new(
            vec![Entry { spec: four, limit: None }],
            players(&["a", "b", "c"]),
            settings(Mode::Both, Format::Challenges(3)),
            1,
        )
        .unwrap();
        assert!(only.is_over());
        assert_eq!(only.deal(), None);
    }

    #[test]
    fn a_bout_seating_one_is_refused() {
        let mut one = spec("g", "solo", ChallengeKind::HeadToHead);
        one.slots = 1;
        let err = Match::new(
            vec![Entry { spec: one, limit: None }],
            players(&["a", "b"]),
            settings(Mode::Both, Format::Challenges(1)),
            1,
        )
        .err()
        .unwrap();
        assert_eq!(err, MatchError::TooFewSlots(ChallengeId("solo".into())));
    }

    #[test]
    fn the_deal_after_a_void_is_reproducible() {
        let deals = || {
            let mut m = Match::new(
                pool(ChallengeKind::Survival, 5),
                players(&["a"]),
                settings(Mode::Survival, Format::Challenges(6)),
                21,
            )
            .unwrap();
            m.deal().unwrap();
            m.void();
            let mut out = Vec::new();
            while let Some(deal) = m.deal() {
                out.push((deal.index, deal.challenge.id.0.clone(), deal.seed));
                let r = died(&deal);
                m.record(vec![(PlayerId("a".into()), r)]);
            }
            out
        };
        assert_eq!(deals(), deals());
    }
}
