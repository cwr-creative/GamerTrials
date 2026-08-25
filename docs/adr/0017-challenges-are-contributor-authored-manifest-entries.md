# 0017. Challenges are contributor-authored manifest entries, not core-defined kinds

Date: 2026-08-25
Status: Accepted

## Context

Phase 2 made adding a *game* a data change (invariant 5, ADR 0016). It did not
do the same for adding a *challenge*, and the gap was not noticed because the
word "challenge" was being used for three different things at once.

What the schema actually had:

* `[[stage]].challenges = ["time_trial", "endurance"]` — a list of which of two
  kinds a stage was *eligible* for. Eligibility, not a definition.
* `[[stage]].endurance = { survive_seconds = 45 }` — one victory condition, on
  the stage rather than on a challenge, so a stage could carry exactly one.
* `ChallengeKind { TimeTrial, Endurance }` — a closed Rust enum. A contributor
  wanting "kill 20 enemies without dying" was editing that enum, the loader,
  and eventually `challenge-core`.
* `[[challenge]]` — reserved for a *generated variant's* ROM patch slots
  (ADR 0015), which is a different concept wearing the same name.

Meanwhile `docs/manifest-schema.md` stated, under **Deliberately absent**, that
"Nothing here says what a run was worth", and CLAUDE.md said a manifest reports
facts while `challenge-core` decides what they mean. Both are right about the
*verdict*. Both were read as also forbidding the *criteria*, which is what left
completion conditions with no home: `[[rule]]`s already emitted
`enemy_defeated` and `item` with counts, and there was nowhere to say which
count wins.

The product model this project is built for is that a contributor adds a
favourite game, and the challenges within it, entirely through a manifest. That
requires challenges to be authored data.

## Decision

**A challenge is a first-class manifest entry.** `[[challenge]]` names one
stage, one kind, and the criteria that complete it. Contributors author every
challenge they want to exist; nothing is inferred. A game whose 100 stages
should all be available as time trials carries 100 `[[challenge]]` entries, and
200 if they should also all be available as survival runs.

**Two kinds, and they differ in what death costs, not in what completes them:**

| Kind | On death | Scored on |
|---|---|---|
| `time_trial` | re-enter at the stage's entry point; the clock keeps running | total frames across every attempt |
| `survival` | the run is over; the player spectates | whether the criteria were met at all |

Because death ends a survival run by definition, "without dying" is *not* a
criterion — it is what the kind means. Both kinds therefore share one criteria
vocabulary, and `complete_when` reads the same in either.

**Criteria are a closed set of predicates over facts a manifest already
emits**, combined only by conjunction:

```toml
[challenge.complete_when]
cleared = true                        # a [[rule]] emitting stage_cleared fired
survive_seconds = 45.0                # survival only
enemies_defeated = 20
objectives = ["boss-defeated"]
items_collected = { banana = 50 }
```

Every listed criterion must hold. There is no `any`, no negation, no nesting,
and no arithmetic — the same boundary `[[rule]]` already sits behind, and for
the same reason: the moment this grows an expression parser it is a scripting
language living in a data file.

**One new fact, `emit = "objective"`**, carrying a name. It is what "defeat the
boss" and "reach a certain point" are expressed with, and it is a fact in
exactly the sense the existing emits are — the game announced a named
milestone; whether that milestone wins anything is the challenge's question.

**The verdict boundary moves from the manifest to the adapter.** The rule that
matters is that *`session-libretro` never decides a run was won* — not that a
TOML file may not say what winning is. So:

* adapters produce facts (`SessionEvent`),
* manifests declare criteria (`[[challenge]].complete_when`),
* `challenge-core` compares one against the other and produces the verdict.

Invariants 1 and 3 are untouched: pure code reading declarative data is still
pure, and nothing in `[[challenge]]` names a savestate, a core, or a RAM
address.

**`[[stage]]` keeps only what is needed to *prepare* a run** — an id, a name, an
entry point, and a note. `challenges` and `endurance` are removed from it:
which challenges a stage can carry is now expressed by which challenges exist,
so there is one source of truth rather than two that can disagree.

**`[[challenge]]` is the same table ADR 0015 reserved for patch slots**, which
is the right home for them: a generated custom challenge *is* a challenge, and
it needs a stage, a kind and completion criteria like any other. The
`patch_slot`, `group` and `generator_version` keys remain documented and remain
*refused* by the loader until `content-gen` exists.

**Terminology.** `endurance` becomes `survival` throughout, including
`ChallengeKind::Survival`, because that is the word the product uses. Three
words are now distinct and are used consistently:

* **stage** — a place, and the entry point that reaches it.
* **attempt** — one entry-to-terminal-event span. What the watcher latches.
* **run** — one player's whole go at one challenge. A survival run is one
  attempt; a time-trial run is every attempt summed.

This is a breaking schema change, so `schema_version` becomes 2.

## Alternatives considered

**Keep `challenges = [...]` on the stage and infer challenges from it.** This is
what existed, and it is what the decision replaces. Rejected: it can express
"this stage could be a time trial" and nothing else. Two challenges on one stage
are impossible, a criterion other than "reach the end" is impossible, and the
eligibility list is a second source of truth that can contradict the challenges
actually defined.

**A `kind = "both"` shorthand that expands to two challenges.** Rejected on
explicit instruction, and it is right: the two challenges on one stage almost
never share criteria — the survival version of a level is "reach the end", the
time trial version may be "reach the end" or may be "stomp five enemies". A
shorthand that only works when they coincide buys one line and costs the
property that every challenge that exists is visible in the file.

**Let `complete_when` take a general expression** (`and`, `or`, `not`,
comparisons). Rejected for the reason the schema already rejects arithmetic in
`[[rule]]`: a data file with an expression language in it is a programming
language with none of a programming language's tooling. Conjunction of a closed
predicate set covered every case anyone could name.

**Keep victory conditions out of manifests entirely and put them in
`challenge-core`.** Rejected: it is the reading that produced the gap. It
mistakes "adapters must not judge" for "data must not describe", and it makes
adding a challenge a Rust change, which is invariant 5's failure mode.

**Rename `[[challenge]]`'s patch slots to `[[variant]]` and give
`[[challenge]]` to this.** Considered and unnecessary: ADR 0015's table already
carried `id`, `name` and `stage`. It was a challenge missing a kind and a
completion condition, not a different concept.

**Express "without dying" as an explicit criterion so both kinds are
symmetrical.** Rejected: it would be mandatory on every survival challenge and
meaningless on every time trial, which is a field that carries no information.
The kind already says it.

## Consequences

**Adding a challenge is a manifest change.** So is adding a challenge *type*, as
long as it is expressible in the predicate set. Adding a predicate is still
code, and deliberately so — a predicate has to be evaluable against facts that
actually exist.

**Contributors author more lines.** 100 levels as both kinds is 200 entries.
Accepted knowingly: every challenge a player can be dealt is visible in the
file, and the alternative shorthands all break as soon as the two kinds want
different criteria.

**Criteria are validated against the rules in the same manifest.** A
`complete_when` naming an item no `[[rule]]` emits, an objective nothing
announces, or `cleared` in a manifest with no `stage_cleared` rule is a load
error. This is the `deny_unknown_fields` argument applied one level up: an
unsatisfiable criterion is a challenge nobody can ever complete, and it looks
exactly like a wrong address.

**Neither kind has a failure bound, and one is owed.** A time trial where death
only restarts has no losing state — a player who never finishes never finishes.
A survival challenge with a criterion the player can decline to pursue
(`enemies_defeated`) has the same problem in the other direction; only
`survive_seconds` terminates itself. A per-challenge time limit, a global
per-round cap, or a forfeit is needed before either kind ships. It is
deliberately *not* in this schema yet: nothing enforces it, and ADR 0015's rule
against shipping an unbuilt opt-in path applies. Settle it with
`challenge-core`, where the round clock will live.

**A time trial's total is the session's arithmetic, not a runner's.**
`StageRunner::enter` resets the frame counter to 0 by design (ADR 0005), so the
sum across attempts belongs to whatever drives the runner. Nothing in an adapter
changes.

**The manifest loader is still owned by an adapter.** `[[challenge]]` is
`challenge-core`'s half of the file, and `session-libretro::manifest` is what
parses it, which is backwards under invariant 2. It is not worth a crate split
before `challenge-core` exists, but that split is now owed — the shared *types*
live in `session-api` so the eventual reader does not have to depend on a
libretro adapter to see a challenge.

**`docs/manifest-schema.md`'s "Scoring" exclusion is narrowed, not removed.**
Nothing in a manifest says what a completed run was *worth* — no points, no
ranking, no comparison between players. That is still `challenge-core`'s, and
`[[challenge]]` does not encroach on it.
