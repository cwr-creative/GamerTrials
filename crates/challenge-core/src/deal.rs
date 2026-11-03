//! Which challenges a session plays.
//!
//! A session runs in one of three modes and deals a randomised series from the
//! library. Dealing is a pure function of the library, the mode, the count and
//! a seed, so whoever holds the seed can say what was dealt — the server
//! (ADR 0009), or the client itself for solo play.
//!
//! What is deliberately *not* here: avoiding two challenges on one stage,
//! balancing games or kinds against each other, or weighting anything. Each of
//! those is a product decision nobody has made, and a uniform draw is the one
//! that does not pretend otherwise.
//!
//! The generator is written out by hand for the reason `content-gen`'s is:
//! a deal that changed with a crate upgrade would change which challenges a
//! seed means. It is not yet a *wire* contract — nothing deals on two machines
//! and compares — but it becomes one the moment anything does, and the same
//! rule applies then: changing it changes every seed.
//!
//! [`deal`] deals a fixed count. A match whose length is not known up front —
//! first to N wins, last player standing — needs a [`Dealer`]: a stream that
//! plays the pool through as one shuffled deck, then reshuffles under the next
//! seed derived from the match seed and goes on (ADR 0026). Its first deck is
//! exactly what [`deal`] deals for the same seed, so a fixed-length match and
//! an open-ended one that happens to run the same number of rounds see the
//! same challenges.

use std::collections::BTreeSet;
use std::fmt;

use session_api::{ChallengeId, ChallengeKind, ChallengeSpec, GameId};

/// The three ways a session can run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    TimeTrial,
    Survival,
    /// Either kind, drawn from both as one pool.
    Both,
}

impl Mode {
    /// A head-to-head challenge is admitted by every mode (ADR 0036): a bout
    /// is a round like any other once it is in the pool, and whether it is in
    /// the pool is the lobby's toggle, applied before the pool reaches here.
    pub fn admits(self, kind: ChallengeKind) -> bool {
        match self {
            Mode::TimeTrial => kind == ChallengeKind::TimeTrial,
            Mode::Survival => kind == ChallengeKind::Survival,
            Mode::Both => true,
        }
        .then_some(())
        .or_else(|| (kind == ChallengeKind::HeadToHead).then_some(()))
        .is_some()
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum DealError {
    /// The library holds fewer challenges of this mode than were asked for. A
    /// challenge is never dealt twice in one session.
    NotEnough { mode: Mode, wanted: usize, available: usize },
}

impl fmt::Display for DealError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DealError::NotEnough { mode, wanted, available } => write!(
                f,
                "cannot deal {wanted} challenges in {mode:?} mode: the library holds {available}"
            ),
        }
    }
}

impl std::error::Error for DealError {}

/// Deals `count` distinct challenges of `mode` from `library`, in play order.
///
/// The library is sorted by game and challenge id first, so a deal depends on
/// which challenges exist and not on the order manifests happened to load in.
pub fn deal(
    library: &[ChallengeSpec],
    mode: Mode,
    count: usize,
    seed: u64,
) -> Result<Vec<&ChallengeSpec>, DealError> {
    Ok(order(library, mode, count, seed)?.into_iter().map(|i| &library[i]).collect())
}

/// [`deal`], as indices into `library`.
fn order(
    library: &[ChallengeSpec],
    mode: Mode,
    count: usize,
    seed: u64,
) -> Result<Vec<usize>, DealError> {
    let mut pool: Vec<usize> =
        (0..library.len()).filter(|&i| mode.admits(library[i].kind)).collect();
    if pool.len() < count {
        return Err(DealError::NotEnough { mode, wanted: count, available: pool.len() });
    }
    pool.sort_by(|&a, &b| {
        (&library[a].game, &library[a].id).cmp(&(&library[b].game, &library[b].id))
    });

    // A partial Fisher-Yates: the first `count` places are a uniform draw
    // without replacement, in order.
    let mut rng = SplitMix64(seed);
    for i in 0..count {
        let j = i + rng.below((pool.len() - i) as u64) as usize;
        pool.swap(i, j);
    }
    pool.truncate(count);
    Ok(pool)
}

/// What a [`Dealer`] hands out: the `index`-th challenge of the match, counted
/// from 0, and the seed a generated challenge is rolled from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Dealt {
    pub index: u64,
    pub challenge: ChallengeSpec,
    /// Derived from the match seed and `index`, so every player who derives
    /// this challenge's content derives the same artifact (ADR 0013). Issued
    /// for every challenge; a vanilla one ignores it.
    pub seed: u64,
}

/// An open-ended deal: one shuffled deck of the pool after another.
///
/// Within a deck no challenge repeats. Across the deck boundary the first
/// challenge of the new deck is moved one place down if it would repeat the
/// last one dealt, which is the only departure from a plain reshuffle.
///
/// A challenge can be [withdrawn](Dealer::withdraw) — a round nobody could
/// prepare (ADR 0029). The deck is still shuffled over the whole pool and the
/// withdrawn ones are passed over, so what a seed deals stays reproducible
/// from the seed and the withdrawals alone.
#[derive(Clone, Debug)]
pub struct Dealer {
    pool: Vec<ChallengeSpec>,
    mode: Mode,
    seed: u64,
    /// Indices into `pool` still to deal from the current deck, last first.
    deck: Vec<usize>,
    decks: u64,
    dealt: u64,
    last: Option<usize>,
    /// Indices into `pool` that are never dealt again.
    withdrawn: BTreeSet<usize>,
}

/// Domains for [`derive`]: which purpose a derived seed serves.
const DECK: u64 = 1;
const CONTENT: u64 = 2;
/// Which players sit in a bout's slots (ADR 0036).
pub(crate) const SLOTS: u64 = 3;

/// A seed for one purpose, from the match seed. Written out so that it can
/// be reproduced anywhere: one SplitMix64 output over a state that folds in
/// the domain and the index.
pub(crate) fn derive(seed: u64, domain: u64, index: u64) -> u64 {
    let state = seed ^ domain.rotate_left(32) ^ index.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    SplitMix64(state).next()
}

impl Dealer {
    /// Refuses a pool with nothing of `mode` in it; otherwise never runs out.
    pub fn new(pool: Vec<ChallengeSpec>, mode: Mode, seed: u64) -> Result<Dealer, DealError> {
        let available = pool.iter().filter(|c| mode.admits(c.kind)).count();
        if available == 0 {
            return Err(DealError::NotEnough { mode, wanted: 1, available });
        }
        Ok(Dealer {
            pool,
            mode,
            seed,
            deck: Vec::new(),
            decks: 0,
            dealt: 0,
            last: None,
            withdrawn: BTreeSet::new(),
        })
    }

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// The match seed this dealer deals from.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Every challenge the mode admits that has not been withdrawn.
    pub fn remaining(&self) -> impl Iterator<Item = &ChallengeSpec> {
        self.pool
            .iter()
            .enumerate()
            .filter(|(i, c)| self.mode.admits(c.kind) && !self.withdrawn.contains(i))
            .map(|(_, c)| c)
    }

    /// How many challenges have been dealt so far.
    pub fn dealt(&self) -> u64 {
        self.dealt
    }

    /// The seed a generated challenge at `index` is rolled from.
    pub fn content_seed(&self, index: u64) -> u64 {
        derive(self.seed, CONTENT, index)
    }

    /// Never deals this challenge again.
    pub fn withdraw(&mut self, game: &GameId, id: &ChallengeId) {
        for (i, c) in self.pool.iter().enumerate() {
            if &c.game == game && &c.id == id {
                self.withdrawn.insert(i);
            }
        }
    }

    /// Every challenge the mode admits has been withdrawn: nothing is left to deal.
    pub fn exhausted(&self) -> bool {
        !self
            .pool
            .iter()
            .enumerate()
            .any(|(i, c)| self.mode.admits(c.kind) && !self.withdrawn.contains(&i))
    }

    /// # Panics
    ///
    /// If the dealer is [exhausted](Dealer::exhausted).
    pub fn deal(&mut self) -> Dealt {
        self.deal_where(|_| true).expect("every challenge has been withdrawn")
    }

    /// [`deal`](Dealer::deal), passing over any challenge `playable` refuses
    /// — a bout with more slots than there are players to sit in them (ADR
    /// 0036). Passed over is not withdrawn: the deck comes round again.
    /// `None` when nothing left in the pool is playable.
    pub fn deal_where(&mut self, playable: impl Fn(&ChallengeSpec) -> bool) -> Option<Dealt> {
        let any = self
            .pool
            .iter()
            .enumerate()
            .any(|(i, c)| self.mode.admits(c.kind) && !self.withdrawn.contains(&i) && playable(c));
        if !any {
            return None;
        }
        let i = loop {
            if self.deck.is_empty() {
                self.shuffle();
            }
            let i = self.deck.pop().expect("a deck is never empty");
            // Withdrawn since this deck was shuffled, or not playable today.
            if !self.withdrawn.contains(&i) && playable(&self.pool[i]) {
                break i;
            }
        };
        self.last = Some(i);
        let index = self.dealt;
        self.dealt += 1;
        Some(Dealt { index, challenge: self.pool[i].clone(), seed: self.content_seed(index) })
    }

    fn shuffle(&mut self) {
        let seed = if self.decks == 0 { self.seed } else { derive(self.seed, DECK, self.decks) };
        let count = self.pool.iter().filter(|c| self.mode.admits(c.kind)).count();
        let mut order = order(&self.pool, self.mode, count, seed).expect("checked in new");
        order.retain(|i| !self.withdrawn.contains(i));
        if order.len() > 1 && self.last == Some(order[0]) {
            order.swap(0, 1);
        }
        order.reverse();
        self.deck = order;
        self.decks += 1;
    }
}

/// SplitMix64 (Steele, Lea & Flood, 2014). Small, fully specified, and good
/// enough to shuffle a library; not for anything adversarial.
pub(crate) struct SplitMix64(pub(crate) u64);

impl SplitMix64 {
    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`, by rejection rather than a bare modulo, which would
    /// favour the low values whenever `n` does not divide 2^64.
    pub(crate) fn below(&mut self, n: u64) -> u64 {
        assert!(n > 0, "nothing to choose from");
        let threshold = n.wrapping_neg() % n;
        loop {
            let x = self.next();
            if x >= threshold {
                return x % n;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use session_api::{ChallengeId, Completion, GameId, StageId};

    fn challenge(game: &str, id: &str, kind: ChallengeKind) -> ChallengeSpec {
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

    fn library() -> Vec<ChallengeSpec> {
        let mut l = Vec::new();
        for game in ["smb3", "dkc", "doom"] {
            for n in 0..4 {
                l.push(challenge(game, &format!("tt-{n}"), ChallengeKind::TimeTrial));
                l.push(challenge(game, &format!("sv-{n}"), ChallengeKind::Survival));
            }
        }
        l
    }

    fn ids(dealt: &[&ChallengeSpec]) -> Vec<String> {
        dealt.iter().map(|c| format!("{}/{}", c.game, c.id)).collect()
    }

    /// The published SplitMix64 reference output for seed 1234567, so the
    /// generator cannot drift without a test saying so.
    #[test]
    fn the_generator_matches_its_reference_output() {
        let mut rng = SplitMix64(1234567);
        let got: Vec<u64> = (0..5).map(|_| rng.next()).collect();
        assert_eq!(
            got,
            [
                6457827717110365317,
                3203168211198807973,
                9817491932198370423,
                4593380528125082431,
                16408922859458223821,
            ]
        );
    }

    #[test]
    fn a_seed_always_deals_the_same_session() {
        let l = library();
        let a = ids(&deal(&l, Mode::Both, 6, 42).unwrap());
        assert_eq!(a, ids(&deal(&l, Mode::Both, 6, 42).unwrap()));
        assert_ne!(a, ids(&deal(&l, Mode::Both, 6, 43).unwrap()));
    }

    #[test]
    fn the_load_order_of_the_library_does_not_matter() {
        let l = library();
        let mut reversed = l.clone();
        reversed.reverse();
        assert_eq!(
            ids(&deal(&l, Mode::Both, 6, 7).unwrap()),
            ids(&deal(&reversed, Mode::Both, 6, 7).unwrap())
        );
    }

    #[test]
    fn a_mode_deals_only_its_own_kind_and_never_repeats() {
        let l = library();
        for seed in 0..100 {
            let dealt = deal(&l, Mode::Survival, 12, seed).unwrap();
            assert!(dealt.iter().all(|c| c.kind == ChallengeKind::Survival));
            let mut unique = ids(&dealt);
            unique.sort();
            unique.dedup();
            assert_eq!(unique.len(), 12, "seed {seed} repeated a challenge");
        }
    }

    #[test]
    fn asking_for_more_than_the_library_holds_is_refused() {
        let err = deal(&library(), Mode::TimeTrial, 13, 0).unwrap_err();
        assert_eq!(err, DealError::NotEnough { mode: Mode::TimeTrial, wanted: 13, available: 12 });
    }

    /// A bout is admitted by every mode; whether it is in the pool at all is
    /// the lobby's toggle (ADR 0036).
    #[test]
    fn every_mode_admits_a_head_to_head_challenge() {
        let mut l = library();
        l.push(challenge("sf2", "fight", ChallengeKind::HeadToHead));
        for mode in [Mode::TimeTrial, Mode::Survival, Mode::Both] {
            assert!(mode.admits(ChallengeKind::HeadToHead), "{mode:?}");
            let dealt = deal(&l, mode, l.len() - if mode == Mode::Both { 0 } else { 12 }, 1).unwrap();
            assert!(dealt.iter().any(|c| c.kind == ChallengeKind::HeadToHead), "{mode:?}");
        }
    }

    /// Passed over is not withdrawn: a challenge not playable today is dealt
    /// when it is, and `None` says nothing playable is left.
    #[test]
    fn deal_where_passes_over_and_comes_back_round() {
        let l = library();
        let mut dealer = Dealer::new(l.clone(), Mode::Survival, 4).unwrap();
        let skip = dealer.deal().challenge.id.clone();
        let mut dealer = Dealer::new(l, Mode::Survival, 4).unwrap();
        let first = dealer.deal_where(|c| c.id != skip).unwrap();
        assert_ne!(first.challenge.id, skip);
        assert_eq!(first.index, 0, "a pass-over costs no index");
        let later: Vec<_> = (0..11).map(|_| dealer.deal().challenge.id.clone()).collect();
        assert!(later.contains(&skip));
        assert!(dealer.deal_where(|_| false).is_none());
    }

    /// Every challenge can come first, and roughly as often as any other.
    #[test]
    fn the_first_challenge_is_drawn_uniformly() {
        let l = library();
        let mut firsts = std::collections::BTreeMap::new();
        for seed in 0..24_000 {
            let first = deal(&l, Mode::Both, 1, seed).unwrap()[0];
            *firsts.entry(format!("{}/{}", first.game, first.id)).or_insert(0u32) += 1;
        }
        assert_eq!(firsts.len(), 24);
        // 1000 expected each; a generous band still catches a skewed draw.
        assert!(firsts.values().all(|n| (850..1150).contains(n)), "{firsts:?}");
    }

    fn dealt_ids(dealer: &mut Dealer, n: usize) -> Vec<String> {
        (0..n)
            .map(|_| dealer.deal())
            .map(|d| format!("{}/{}", d.challenge.game, d.challenge.id))
            .collect()
    }

    #[test]
    fn a_dealers_first_deck_is_what_deal_deals() {
        let l = library();
        let mut dealer = Dealer::new(l.clone(), Mode::Both, 42).unwrap();
        assert_eq!(dealt_ids(&mut dealer, 24), ids(&deal(&l, Mode::Both, 24, 42).unwrap()));
    }

    #[test]
    fn a_dealer_never_runs_out_and_never_repeats_within_a_deck() {
        let l = library();
        for seed in 0..50 {
            let mut dealer = Dealer::new(l.clone(), Mode::Survival, seed).unwrap();
            let all = dealt_ids(&mut dealer, 36);
            for deck in all.chunks(12) {
                let mut unique = deck.to_vec();
                unique.sort();
                unique.dedup();
                assert_eq!(unique.len(), 12, "seed {seed} repeated within a deck");
            }
            assert!(all.windows(2).all(|w| w[0] != w[1]), "seed {seed} dealt one twice running");
            assert_eq!(dealer.dealt(), 36);
        }
    }

    #[test]
    fn a_dealer_is_deterministic_and_seeds_differ_per_challenge() {
        let l = library();
        let a: Vec<Dealt> = {
            let mut d = Dealer::new(l.clone(), Mode::Both, 9).unwrap();
            (0..30).map(|_| d.deal()).collect()
        };
        let b: Vec<Dealt> = {
            let mut d = Dealer::new(l.clone(), Mode::Both, 9).unwrap();
            (0..30).map(|_| d.deal()).collect()
        };
        assert_eq!(a, b);
        assert_eq!(a.iter().map(|d| d.index).collect::<Vec<_>>(), (0..30).collect::<Vec<_>>());
        let mut seeds: Vec<u64> = a.iter().map(|d| d.seed).collect();
        seeds.sort();
        seeds.dedup();
        assert_eq!(seeds.len(), 30);
        let mut other = Dealer::new(l, Mode::Both, 10).unwrap();
        assert_ne!(other.deal().seed, a[0].seed);
    }

    #[test]
    fn a_pool_with_nothing_of_the_mode_is_refused() {
        let l: Vec<ChallengeSpec> =
            library().into_iter().filter(|c| c.kind == ChallengeKind::Survival).collect();
        let err = Dealer::new(l, Mode::TimeTrial, 0).unwrap_err();
        assert_eq!(err, DealError::NotEnough { mode: Mode::TimeTrial, wanted: 1, available: 0 });
    }

    #[test]
    fn a_pool_of_one_deals_it_forever() {
        let l = vec![challenge("smb3", "only", ChallengeKind::Survival)];
        let mut dealer = Dealer::new(l, Mode::Both, 3).unwrap();
        assert!(dealt_ids(&mut dealer, 5).iter().all(|id| id == "smb3/only"));
    }

    #[test]
    fn a_withdrawn_challenge_is_passed_over_mid_deck_and_in_every_later_deck() {
        let l = library();
        let mut dealer = Dealer::new(l.clone(), Mode::Survival, 5).unwrap();
        let first = dealer.deal();
        let plain = {
            let mut d = Dealer::new(l, Mode::Survival, 5).unwrap();
            dealt_ids(&mut d, 36)
        };
        // Withdraw something still to come in this deck.
        let later = &plain[3];
        let (game, id) = later.split_once('/').unwrap();
        dealer.withdraw(&GameId(game.into()), &ChallengeId(id.into()));
        let rest = dealt_ids(&mut dealer, 33);
        assert!(!rest.contains(later));
        let expected: Vec<String> = plain[1..].iter().filter(|c| *c != later).cloned().collect();
        assert_eq!(rest[..10], expected[..10], "the rest of the deck keeps its order");
        assert_eq!(format!("{}/{}", first.challenge.game, first.challenge.id), plain[0]);
        assert!(!dealer.exhausted());
    }

    #[test]
    fn withdrawing_everything_the_mode_admits_exhausts_the_dealer() {
        let l = library();
        let mut dealer = Dealer::new(l.clone(), Mode::TimeTrial, 1).unwrap();
        for c in l.iter().filter(|c| c.kind == ChallengeKind::TimeTrial) {
            assert!(!dealer.exhausted());
            dealer.withdraw(&c.game, &c.id);
        }
        assert!(dealer.exhausted());
    }
}
