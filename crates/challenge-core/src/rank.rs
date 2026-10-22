//! How the players who ran one challenge place against each other.
//!
//! The two kinds are scored on different things, and this is the whole of it:
//!
//! * **Time trial** — completed runs first, fewest frames first among them.
//!   A run that timed out or was forfeited places behind every completed one,
//!   tied with the others that did not finish.
//! * **Survival** — completed or not. Every completed run ties, and so does
//!   every run that was not; how long a failed run lasted is not what the kind
//!   measures.
//!
//! Points, and how a session totals places across challenges, are not here:
//! nobody has decided them, and ranking within one challenge is all the
//! documented rules say.

use std::cmp::Ordering;

use session_api::ChallengeKind;

use crate::run::RunResult;

/// The place of each result, by index, in standard competition ranking: two
/// tied for first are both 1, and the next is 3.
///
/// # Panics
///
/// If the results are not all from one challenge. Places across challenges
/// mean nothing, and silently producing them would be worse than stopping.
pub fn places(results: &[RunResult]) -> Vec<u32> {
    if let Some(first) = results.first() {
        assert!(
            results.iter().all(|r| r.challenge == first.challenge && r.kind == first.kind),
            "places() compares runs of one challenge, and these are from several"
        );
    }
    results
        .iter()
        .map(|r| 1 + results.iter().filter(|o| compare(o, r) == Ordering::Less).count() as u32)
        .collect()
}

/// `Less` means `a` places ahead of `b`.
fn compare(a: &RunResult, b: &RunResult) -> Ordering {
    match (a.completed(), b.completed()) {
        (true, false) => Ordering::Less,
        (false, true) => Ordering::Greater,
        (false, false) => Ordering::Equal,
        (true, true) => match a.kind {
            ChallengeKind::TimeTrial => a.frames.cmp(&b.frames),
            ChallengeKind::Survival => Ordering::Equal,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{AttemptEnd, Ending};
    use session_api::ChallengeId;

    fn result(kind: ChallengeKind, ending: Ending, frames: u64) -> RunResult {
        RunResult { challenge: ChallengeId("c".into()), kind, ending, frames, attempts: 1 }
    }

    #[test]
    fn a_time_trial_ranks_finishers_by_frames_and_ties_the_rest() {
        let tt = ChallengeKind::TimeTrial;
        let results = [
            result(tt, Ending::Completed, 1650),
            result(tt, Ending::TimedOut, 3600),
            result(tt, Ending::Completed, 900),
            result(tt, Ending::Forfeited, 120),
            result(tt, Ending::Completed, 1650),
        ];
        assert_eq!(places(&results), [2, 4, 1, 4, 2]);
    }

    #[test]
    fn a_survival_run_is_completed_or_not() {
        let sv = ChallengeKind::Survival;
        let results = [
            result(sv, Ending::Failed(AttemptEnd::Died), 3000),
            result(sv, Ending::Completed, 1800),
            result(sv, Ending::Completed, 2400),
            result(sv, Ending::Failed(AttemptEnd::Died), 10),
        ];
        assert_eq!(places(&results), [3, 1, 1, 3]);
    }

    #[test]
    fn nobody_finishing_is_a_tie_for_first() {
        let tt = ChallengeKind::TimeTrial;
        let results = [result(tt, Ending::TimedOut, 10), result(tt, Ending::Forfeited, 4)];
        assert_eq!(places(&results), [1, 1]);
    }

    #[test]
    #[should_panic(expected = "one challenge")]
    fn runs_of_different_challenges_are_not_compared() {
        let mut other = result(ChallengeKind::TimeTrial, Ending::Completed, 1);
        other.challenge = ChallengeId("d".into());
        places(&[result(ChallengeKind::TimeTrial, Ending::Completed, 1), other]);
    }
}
