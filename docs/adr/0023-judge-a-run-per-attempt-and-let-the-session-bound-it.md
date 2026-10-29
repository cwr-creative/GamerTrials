# 0023. Judge a run per attempt, end an attempt at a death, and let the session bound it

Date: 2026-10-09
Status: Proposed. Policy settled by 0026.

## Context

`challenge-core` is the one party that judges (ADR 0017): it compares the
`SessionEvent`s an adapter reports against the `complete_when` a manifest
declares. Building it forced four questions the existing ADRs leave open, and
each has an answer that looks obvious and is wrong somewhere in the vertical
slice:

* **What ends an attempt?** `SessionEvent::is_terminal` says a clear or an exit.
  But DKC reports a death and an exit on the same frame, SMB3 reports a death as
  an exit of unknown cause, and Doom reports a death and *stays in the level* —
  it waits for the player to press use.
* **Does progress carry across attempts?** A time trial's frames do (ADR 0017).
  Whether its stomp count does is not written anywhere.
* **When does a run complete?** At the end of the stage, or when the criteria
  first hold? The SMB3 manifest's own note says "the clock stops on the fifth
  stomp; reaching the end of the level is not required".
* **What bounds a run?** ADR 0017 owes a failure bound and lists three shapes —
  a per-challenge limit, a per-round cap, a forfeit — without choosing.

## Decision

**A death ends the attempt, whether or not the stage is left.** The two kinds
are defined by what a death costs, so the death is the event that matters. An
attempt also ends at a clear or at any exit. The first event on a frame that
ends the attempt wins, in the order the runner reported them, and the rest of
that frame is ignored.

**Criteria are met within one attempt.** Re-entering restores the stage,
enemies and items included, so a count carried across attempts could be farmed
by dying. A time trial banks its frames across attempts; it does not bank its
progress.

**A run completes on the first frame its criteria all hold**, not at the end of
the stage. `cleared` is how a challenge asks for the end of the stage. A death
never completes a run, even on a frame where the criteria would otherwise hold:
surviving a frame means being alive at the end of it.

**The bound is the session's, not the manifest's.** A `Run` takes an optional
frame limit, counted across every attempt, and a forfeit. Both are arguments,
so a per-challenge limit, a per-round cap and a give-up button are all
expressible without the schema changing. Which of them a session uses is
decided with the session, not here.

## Alternatives considered

**End an attempt only at `is_terminal`.** Rejected: Doom never leaves the level
on death, so a Doom time trial would wait forever. `is_terminal` is still right
for what the watcher latches; it is the wrong question for what a death costs.

**Accumulate progress across a time trial's attempts.** Rejected: "stomp four
enemies" becomes "stomp the first enemy, die, repeat", which is faster than
playing the stage and is not the challenge anyone wrote.

**Complete at the terminal event.** Rejected: it makes every non-`cleared`
criterion also require reaching the end, so the criterion a contributor wrote
is not the one that is judged.

**Put a limit in `[[challenge]]` now.** Rejected for the reason ADR 0017 gives:
nothing would enforce it until a session exists, and an unbuilt opt-in path is
the thing ADR 0015 says not to ship. A caller-supplied limit can be fed from a
manifest field later without this decision changing.

## Consequences

**A card-less level cleared normally looks like a death to a time trial.** SMB3
reports both as an exit of unknown cause, so a fortress time trial re-enters
forever. That is ADR 0014's gap reaching the judge, not a new one, and the
manifests already avoid it: SMB3's 1-4 offers survival only.

**On a frame where a criterion and a death land together, the manifest's rule
order decides.** The runner reports in rule order, and that is the only order
there is. It is deterministic, which is what fairness needs.

**`StageEntered` is not used.** The libretro runner clears it before any caller
can read it, so the driver tells the run when an attempt begins (`Run::enter`).

**ADR 0017's failure bound is half settled.** The mechanism exists and is
tested; the policy — how long, and whether per challenge or per round — waits
for the session that will apply it.
