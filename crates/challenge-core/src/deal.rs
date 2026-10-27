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

use std::fmt;

use session_api::{ChallengeKind, ChallengeSpec};

/// The three ways a session can run.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    TimeTrial,
    Survival,
    /// Either kind, drawn from both as one pool.
    Both,
}

impl Mode {
    fn admits(self, kind: ChallengeKind) -> bool {
        match self {
            Mode::TimeTrial => kind == ChallengeKind::TimeTrial,
            Mode::Survival => kind == ChallengeKind::Survival,
            Mode::Both => true,
        }
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
    let mut pool: Vec<&ChallengeSpec> = library.iter().filter(|c| mode.admits(c.kind)).collect();
    if pool.len() < count {
        return Err(DealError::NotEnough { mode, wanted: count, available: pool.len() });
    }
    pool.sort_by(|a, b| (&a.game, &a.id).cmp(&(&b.game, &b.id)));

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

/// SplitMix64 (Steele, Lea & Flood, 2014). Small, fully specified, and good
/// enough to shuffle a library; not for anything adversarial.
struct SplitMix64(u64);

impl SplitMix64 {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in `0..n`, by rejection rather than a bare modulo, which would
    /// favour the low values whenever `n` does not divide 2^64.
    fn below(&mut self, n: u64) -> u64 {
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
}
