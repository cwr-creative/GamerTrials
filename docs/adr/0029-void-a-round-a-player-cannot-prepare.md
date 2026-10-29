# 0029. Void a round a player cannot prepare

Date: 2026-10-29
Status: Accepted

## Context

A client prepares every round it is dealt: it reads the player's dump,
applies any hack, rolls generated content and loads the core (ADR 0009). The
lobby checks dumps and cores before Start, and the server keeps a game whose
files a player is missing out of the pool. The check can still be beaten by
the time a round is dealt. A dump is moved mid-match, a core fails to load, or
a client holds a different build of a game's runner half. When that happens
the round cannot be played on that machine.

The protocol had no word for this. The solo client sent `EndMatch`, which
records no reason and is refused from anyone but the host. In a lobby of
several, a guest who could not prepare would hang the round: the server waits
for every `Prepared` before `Go`.

## Decision

**The client says so, with `Unprepared { round, reason }` in place of
`Prepared`.** This includes a dealt game or challenge it does not hold at all.

**The server voids the round for everyone.** Nobody plays it, nothing is
recorded, and it counts toward no format. A `Challenges N` match still plays N.
The server broadcasts `Voided { round, player, reason }` and deals the next
round. `Unprepared` is accepted only before `Go`. A run under way is given up
with `Forfeit`, which is judged.

**The voided challenge is withdrawn for the rest of the match**
(`Match::void`, `Dealer::withdraw`). Otherwise a challenge that always fails on
one machine would be dealt again at every reshuffle. A match with nothing left
to deal is over, and stands where it is.

**Round indices can have gaps.** A voided index is not reused, so a round
number still names one deal and one content seed.

## Alternatives considered

**The player who cannot prepare forfeits; everyone else plays.** It needs no
new rule in `challenge-core`: a forfeit already costs the full limit (ADR 0026).
Rejected because the failure is usually not the player's choice, and a round
in which one player was never able to play is not a comparison of anyone.

**End the match, with a reason.** It is today's behaviour, given a name.
Rejected because one missing file would end everyone's match, and the next
match would deal from the same pool.

**Withdraw the whole game, not only the challenge.** It would be faster when a
dump is missing, since every challenge of that game would fail too. Rejected
for now: the reason is free text, and a challenge-level failure (one stage's
state, one hack's patch) would take out challenges that work. Revisit if voids
in a row from one game turn out to be common.

## Consequences

**One player's machine can veto a challenge for everyone.** This was accepted
knowingly. The lobby's check is what keeps it rare, and it now covers cores and
reruns every few seconds while the lobby is open, so a void is the exception
it is meant to be.

**A void is not an error.** It does not end the match. The overlay says which
player skipped the round and why, and moves on.

**`challenge-core` learns withdrawal, not preparation.** It knows a round was
dropped, not why. Dumps, cores and generators stay the client's.

**The deal stays reproducible.** A withdrawn challenge is passed over in a deck
shuffled over the whole pool, so the seed and the withdrawals together still
say what was dealt.
