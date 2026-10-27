# 0025. Widen the challenge vocabulary additively, and let a challenge name facts that cost a life

Date: 2026-10-24
Status: Accepted

## Context

ADR 0017 made challenges contributor-authored data: a closed set of predicates,
combined by conjunction, over facts the manifest's own rules emit. It claimed
that this "covered every case anyone could name", and said that adding a
predicate is code, deliberately.

Asking what a contributor could author for games unlike the slice found four
cases it does not cover:

* **A game with no items and no enemies.** Mario Paint's "place twenty notes" is
  a count, and the only open-ended count was `item`. It works, under a name that
  pretends notes are collectibles.
* **A threshold a value can skip.** `enters 20` misses a count that goes from 18
  to 23 in one frame, and a guard could only test `eq`, `ne` and bits — so "at
  least 20" could not be said robustly either way.
* **Counts of facts that are not items.** `damaged` was emitted by all three
  slice games and readable by no criterion; `enemy_defeated` had a criterion
  of its own. Two spellings for one idea — "a tally reached N" — and a third
  fact with none.
* **Facts that must not happen.** "No-hit" and "pacifist" are among the most
  common challenges players invent, and conjunction of goals cannot say them.

The fourth is the one that touches 0017's "no negation", so the question is
how much of it to answer.

## Decision

**Rules gain a threshold edge and two comparison tests.** `edge = "reaches"`
fires on `prev < v && cur >= v`. Guards and captures gain `test = "ge"` and
`test = "le"`. All three are evaluated by the runner; none touches a
criterion.

**One tally namespace, and a counter that is not an item.** `emit = "count"`
with `counter = "<name>"` produces `SessionEvent::Counted`, counted exactly as an
item is. `Completion` carries one map, `counts`, from tally name to "at least
this many", and `SessionEvent::tally` is the one place an event's tally name is
decided: `damaged` and `enemy_defeated` for the built-ins, the manifest's own
names for items and counters. `complete_when` gains `counts = { … }`;
`enemies_defeated` and `items_collected` stay as spellings of it, so no shipped
manifest changes and `schema_version` does not move. A name used by two kinds
of fact is a load error.

**A challenge may name facts that cost what a death costs.**
`ends_attempt_on = ["damaged"]` on a `[[challenge]]` makes a hit end the attempt
exactly as a death does: a time trial re-enters with the clock running, a
survival run is over, and — the ADR 0023 rule for deaths — it never completes a
run on the frame it arrives. Names are checked like criteria. A name that is
also in `complete_when` is refused, as is one that already ends an attempt.

**`at_most` is deferred.** No upper bound on a count, no general negation.

## Alternatives considered

**`at_most` as a criterion.** Expresses "at most three hits", which
`ends_attempt_on` cannot. Deferred rather than rejected, for three reasons that
are each a decision of its own. It holds at frame 0, so it cannot be a goal and
a run with only it would complete instantly — criteria would split into goals
and constraints. Breaking it is a new way for an attempt to end, needing the
same answer `ends_attempt_on` gives but for a threshold rather than a fact. And
it inverts the failure mode, as below, more broadly than `ends_attempt_on`
does. When a challenge needs it and the detectors it would read have been
playtested for misses, it gets its own ADR.

**Numbered "special event" slots** a contributor fills in. Rejected: a fixed
number is a ceiling nothing justifies, and a slot number is a name with the
meaning taken out. Named tallies and named objectives are unbounded,
self-describing and checked at load, which slots would not be.

**General `not` in `complete_when`.** Rejected for 0017's reason, unchanged: an
operator over arbitrary predicates is the start of an expression language.
`ends_attempt_on` is one more fixed predicate, which is what 0017 said adding
one would look like.

**A `falls_to` edge** mirroring `reaches`. Not added: no case needs it yet, and
`changes` guarded by `le` already says "is now at or below" for an objective,
which is idempotent. If a counter needs it, it is one line in the edge table.

**Rename `items_collected` to `counts` and drop the old spelling.** Rejected: it
would make the change breaking for nothing. Both spellings read into one field.

## Consequences

**Contributors can author games the slice does not resemble.** A composer, a
racing game counting laps, a score attack: a counter, a criterion, no Rust.

**`ends_attempt_on` fails quietly where criteria fail loudly.** A criterion
over a broken detector is a challenge nobody completes, and someone reports it.
A fact in `ends_attempt_on` over a broken detector is a challenge where the
attempt never ends — every no-hit run passes and nothing looks wrong. The load
check proves a rule exists, not that it catches every hit. That is accepted, on
the condition `docs/manifest-schema.md` states: a fact used here is playtested
for missed detections first. None of the slice's `damaged` rules has been yet.

**`AttemptEnd` gained `EndedOn(name)`, so it and `Ending` are no longer `Copy`.**
A result can now say which fact ended a survival run.

**The wire will carry `Counted`.** It is a new `SessionEvent` variant, and the
versioned wire messages ADR 0024 anticipates will need to say so.

**0017's "no negation" stands for `complete_when`.** Every criterion is still a
goal. What changed is that a challenge can say what a life costs, which 0017's
two kinds already decided for deaths and nothing else.
