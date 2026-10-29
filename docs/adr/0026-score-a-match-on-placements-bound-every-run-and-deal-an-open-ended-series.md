# 0026. Score a match on placements, bound every run, and deal an open-ended series

Date: 2026-10-28
Status: Accepted

## Context

`challenge-core` judges one run (ADR 0023) and places the players of one
challenge (`places`): completed runs ahead of the rest, a time trial's
finishers by frames, a survival's finishers tied. `deal` draws a fixed count of
challenges from a seed. That is where the documented rules stop, and three
things are missing between them and a game anyone can play:

* **Nothing scores across challenges.** ADR 0017 keeps scoring out of
  manifests and says it is `challenge-core`'s; `rank.rs` says nobody has
  decided it.
* **Nothing bounds a run in practice.** `Run` takes a frame limit and a
  forfeit, but ADR 0017 and 0023 both owe the policy — how long, and whether
  per challenge or per round — and neither kind was to ship without one.
* **Nothing deals a series whose length is not known in advance.** `deal`
  returns `count` challenges and never repeats one.

`docs/client-sketch.md` supplies what was missing: a lobby with three match
modes and four duration formats, and a reading of each. This ADR records what
the sketch settles and what fitting it to `places` decided.

## Decision

**A series of challenges is a *match*.** It is what ADR 0009 called a session.
The `session-*` crates keep their names, and so does `SessionEvent`, because
there "session" is one running stage.

**Every run is bounded.** The limit is the manifest's optional
`limit_seconds`, converted to frames at load as `survive_seconds` is; a
challenge that declares none gets the lobby's per-challenge default, 180
seconds. Every run also has a give-up, which is `Run::forfeit`. In any total a
run that did not complete counts the full limit, forfeit included — a forfeit's
own frame count would make giving up early cheaper than playing on.

**Per challenge each player has three values**: `completed`, `place` (from
`places`) and `frames`, which is the limit when the run did not complete. Every
format reads those three and nothing else:

* *Success* — time trial: place 1, a shared first included; survival:
  completed.
* *Failure* — survival: not completed; time trial: not completed, or in the
  bottom placement group when that group is not everyone. The exception keeps
  a solo player, or a round where every finisher ties, from being punished for
  completing a challenge. Everyone timing out is everyone failing.
* *Points* — the number of players placed behind you. For a time trial that is
  everyone completed faster than, plus everyone who did not complete; for
  survival it is the number who failed.

**Three modes, three standings.** Time Trial ranks on total frames ascending;
Survival on completions descending; Combo on points descending. **Ties stand**
as shared ranks everywhere: a Survival match where everyone clears everything,
a Wins match where two players reach N on the same challenge, an elimination
where the last players go out together. Breaking them by frames would make
survival a race, which ADR 0017 rules out. The score screen shows frames and
wins beside the rank, so there is something to argue about.

**Four formats.** *Challenges N* plays N and then stands. *Strikes N*: N
consecutive failures eliminates. *Wins N*: the first to N successes. *Stock N*:
N failures, consecutive or not, eliminates. An eliminated player spectates. An
elimination format ends when at most one player remains; when the last players
go out on the same challenge the match ends tied between them. A solo
elimination match ends when its one player is eliminated, scored on challenges
survived. Wins, Strikes and Stock are open-ended, so a host may end a match
from the pause menu.

**The dealer is a stream.** A full shuffle of the enabled pool — the existing
`deal` — reshuffled under the next seed derived from the match seed when it is
exhausted. The seed a generated challenge is rolled from is derived from the
match seed and the challenge's index, so every player derives the same
artifact (ADR 0013).

**Spectating does not leak a generated layout.** ADR 0013 left this open. Only
finished or eliminated players spectate, and neither plays that challenge
again, so there is nothing for a spectator to carry back in.

## Alternatives considered

**Placement points in every mode.** One rule for all three. Rejected: a pure
time trial is better read as total time and a pure survival as a tally — the
sketch's per-mode readings are what players expect. Points are what Combo
needs because its two kinds share no other figure.

**Survival ranked by frames.** Rejected: ADR 0017. Survival is pass/fail, and
how long a failed run lasted is not what the kind measures.

**Break ties by frames.** Rejected for the same reason, and ties are rare in a
time trial anyway.

**A per-round cap instead of a per-challenge limit.** Rejected: a
per-challenge limit is what a manifest can hint at, and a cap across rounds
punishes a slow early challenge twice — once in frames, once in what is left
for the rest.

**A cap on challenge count for the open-ended formats.** Rejected: the bound
per challenge makes each round finite, and the host can end the match. A cap
would be Challenges N hiding inside each of the other three.

**Wins only as the Combo score.** Rejected: a consistent second in a field of
six scores zero.

## Consequences

**`challenge-core` gains a `Match` and a `Dealer`.** Built alongside this ADR. `places`
and `Run` are unchanged; success, failure and points are each one small
function over the three per-challenge values, with the solo and all-tied cases
as named tests.

**The limit is mandatory, so the lobby always has a default.** `Run::with_limit`
stays optional as a mechanism; a match never runs without one.

**`limit_seconds` and `goal` join `[[challenge]]` additively.** `schema_version`
stays 3. `goal` is the plain-English text the sketch's intro shows; it is
static, so it must not promise a rolled quantity.

**ADR 0023 is effectively accepted.** Its Status line gains "Policy settled by
0026" — appended, not rewritten, because ADRs are append-only.

**ADR 0013 stays Proposed** until a server issues seeds and compares hashes.
The dealer derives every per-challenge seed from the match seed; who issues
the match seed is the half that is not built.
