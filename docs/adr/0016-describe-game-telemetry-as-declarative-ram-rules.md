# 0016. Describe game telemetry as declarative RAM rules in the manifest

Date: 2026-08-25
Status: Accepted

## Context

ADR 0012 said to build the second harness before extracting an interface. Both
now exist — `smb3-harness` (Phase 1) and `dkc-harness` (Phase 2's first half) —
and the extraction is Phase 2's second half. Put side by side, most of the
duplication is uninteresting: the libretro FFI layers differ by four joypad
button ids and three environment calls, the audio layer is byte-for-byte
identical, and the input layer differs only in how a modern controller's face
diamond maps onto the console's pad. Those merge without a decision.

One thing does need a decision. Each harness has a hand-written `tick` that
reads a fixed struct out of RAM and runs a list of `if prev.x != cur.x` arms.
SMB3's is 120 lines, DKC's is 190, and comparing them shows they differ only in
three things per arm: **which field**, **which kind of transition**, and **what
it means**. Nothing else — not the structure, not the ordering discipline, not
the once-per-attempt latching.

That matters because invariant 5 says adding a game is a data change. A per-game
`tick` in Rust makes that false immediately: every new libretro game would be a
new module, a new struct, and a new list of `if`s.

Two facts constrain the answer. Detecting stage completion and death by watching
emulator RAM is the presumed mechanism for the whole slice (unwritten ADR 0014),
so whatever expresses it will be exercised hard and by contributors, not only by
the author. And the addresses themselves are the least trustworthy part of the
system — most of DKC's have two independent sources and none have a full
playtest — so being able to retarget one without a rebuild is not a
convenience, it is the working method.

## Decision

Game telemetry is **data in the manifest**, and `session-libretro` interprets it.

A manifest declares named `[watch.<name>]` addresses, and `[[rule]]`s over them.
A rule names one field, one **edge** (`increases`, `decreases`, `changes`,
`enters`/`leaves` a value, `bit_set`/`bit_cleared`, `bits_set`), and one **emit**
— which is a `SessionEvent` kind. Optional modifiers cover the three things the
two games needed and nothing more: a `guard` (a test on one field's current
value), `unless` (suppression by another rule that fired on the same frame), and
`timed_to` a `[[marker]]`.

A **marker** is a transition that arms, expires after a window, and optionally
**captures** whether a test held at the moment it armed. It exists because DKC's
screen starts fading before the overworld finishes loading, and ADR 0005 says
the hand-off is not the player's time; the pause-flag capture is what separates a
menu quit from a real exit, and it has to be read at arming time because by the
time the map is up the menu is gone.

The full schema is `docs/manifest-schema.md`. The two games are
`manifests/smb3.toml` and `manifests/dkc.toml`. There is no per-game Rust left.

Three properties are deliberate and are the boundaries of this decision:

**There is no arithmetic and no expression language.** A rule reads one field's
transition, optionally guarded by one other field's current value. SMB3 stores
*world − 1*; the manifest does not add one to it, it labels `0x00` as
`"world 1"`. If a game needs more than an edge to express what happened, that is
evidence a better address exists and has not been found yet — which is exactly
what happened to DKC's completion rule, twice.

**An emit is a fact, not a verdict.** `stage_cleared`, `died`, `damaged` — never
`passed` or `failed`. Whether a death ends a run depends on the challenge, and
that is `challenge-core`'s question. Both old harnesses conflated these
(`Event::StageFailed`), and separating them is a correction, not just a rename.

**Strict parsing.** Every table denies unknown keys. A permissive loader turns a
typo into a detector that silently never fires, which is indistinguishable from
a wrong address and much harder to notice than a parse error.

The interface ADR 0012 promised is extracted alongside: `StageRunner`,
`Presentable` and `SessionEvent` live in `session-api`, and `LibretroRunner`
implements them.

## Alternatives considered

**Keep a per-game Rust module behind a trait.** A `GameWatcher` trait with an
`smb3` and a `dkc` impl. This is the smallest change and it type-checks the rules
— an address typo becomes a compile error rather than a silent no-op. Rejected
because it fails invariant 5 outright: every new game is code, review, and a
release. It also does not solve the problem that actually costs time, which is
that an address needs retargeting *while the emulator is running* and a rebuild
breaks that loop.

**An embedded scripting language (Lua, Rhai, WASM).** Expressive enough for any
game, and there is prior art — emulator scripting is how most RAM-watching tools
work. Rejected on scope: it makes every manifest arbitrary code, which is a
security boundary this project does not want on contributor-supplied data, and
it makes determinism a per-manifest property rather than a structural one.
It would also be *more* than the evidence supports: two independently written
harnesses needed eight edge kinds and three modifiers between them, and building
a language to cover an unmeasured future is the same instinct ADR 0007 rejected.

**rcheevos-style expression strings** (`0xH0527=0_d0xH0527!=0`). The obvious
neighbour, and the format the RetroAchievements code notes this project already
mines are paired with. Genuinely tempting: it is proven over thousands of games,
and reusing it would make achievement-authoring knowledge transfer directly.
Rejected for now because its compactness is the point of it and readability is
the point of ours — a manifest is a document a contributor reads to understand a
game, and the provenance note next to each address is as important as the
address. Worth revisiting if manifests ever need to be *generated* from RA data
rather than written; that would be a new ADR, not an extension of this one.

**Put the rules in a separate file from the manifest.** Rules are the volatile
part; identity and ROM hashes are not. Rejected as premature: one file per game
is one thing to find, and nothing yet needs to version them separately.

## Consequences

**Adding a libretro game is a TOML file.** The claim in invariant 5 is now
testable, and `manifests/` is where a contributor works. Whether that is
*pleasant* for a game whose RAM map does not exist yet is unproven — DKC and
SMB3 both had documented maps to start from.

**The rules lost their type checking.** An address that reads the wrong byte was
never caught by the compiler, but a field name typo used to be. Strict parsing
replaces most of it: an unknown key, an unknown field name, an edge missing its
`value`, a `timed_to` naming no marker, and a `reason` on an emit that takes none
are all load errors. What is not caught is a rule that parses and means the wrong
thing, which is what the tests exist for.

**The old harnesses' tests survived and now run against the shipped manifests.**
Every behavioural test both crates had was ported into
`session-libretro/src/manifest_tests.rs`, driving `manifests/smb3.toml` and
`manifests/dkc.toml` rather than a fixture. If a manifest stops expressing "a
bonus room is not a clear", they fail. Two tests did not survive: DKC's
level-name "family" tests, which supported a completion rule that no longer
exists.

**SMB3 gained two detectors for free.** Its hit timer and stomp counter had
addresses but only ever printed; expressed as rules they emit `Damaged` and
`EnemyDefeated` like DKC's. Nobody found a new address — the harness had simply
never been asked to treat them as events.

**Some log output got less pretty.** DKC's `KONG` bitfield printed as `K-N-`;
labels cannot express a bitfield, so the running line shows the raw byte. The
per-letter pickup event still names the letter, which is the part that matters.

**`--addr` now works for every game**, not just DKC, because the override path
is a property of the loader rather than of a hand-written macro.

**A manifest is now a trust boundary.** It decides what bytes get read and what
they are taken to mean. That is the same boundary ADR 0015 put patch slots on,
and it is worth stating once rather than discovering per feature.

**Unwritten ADR 0014 gets easier and no less necessary.** The RAM-watching half
of it is now a schema, which is most of what that ADR would have had to
describe. What it still has to answer is unchanged: what an external-process
adapter does instead, since Doom has no addressable core RAM and reports through
a stats file. `[runner.external]` is where that answer will go.
