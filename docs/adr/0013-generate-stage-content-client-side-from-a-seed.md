# 0013. Generate stage content on each client from a server-issued seed

Date: 2026-08-20
Status: Proposed

## Context

Some challenges are not drawn from vanilla game content. They are generated per
attempt: a 30-second SMB3 survival stage whose layout and enemy population differ
every time; a Doom map built on the fly. This is a stated product goal, not an
experiment, and no existing ADR accounts for it.

Three facts constrain the design.

Every player in a session must face the *same* generated stage, or the scores are
not comparable.

The generated artifact is a derivative of a copyrighted ROM the player supplies
themselves. `asset-store` already verifies each player owns a matching base ROM;
shipping a patched ROM between machines would move copyrighted bytes across the
wire, which player-supplied ROMs specifically avoid.

Generation is per-game-engine work — an SMB3 level writer knows about SMB3 object
lists. It cannot live in `challenge-core`, which is pure.

## Decision

The server issues a **seed** as part of the challenge assignment it already owns
(ADR 0009). Each client generates the artifact **locally** and deterministically
from `(base_rom_hash, generator_id, generator_version, seed)`. Nothing generated
is transmitted.

That tuple is the **derivation key**. `asset-store` caches generated artifacts
under it, so replaying the same challenge is a cache hit.

Each client hashes the artifact it produced and reports that hash with its first
`SessionEvent`. The server compares hashes across players and **aborts the
challenge on mismatch**. Version skew must fail loudly; players unknowingly
racing on different layouts is the failure mode ADR 0003 calls the worst one
available.

Generators live in a `content-gen` crate, behind an interface owned by whatever
prepares a stage before handing it to a `StageRunner`. `challenge-core` and
`session-api` learn nothing about them: a generator's output is an ordinary
artifact plus an `EntryPoint`.

**Not built in Phase 1 or Phase 2.** This ADR records the constraints generation
places on decisions being made now. It does not schedule the work.

## Alternatives considered

**Server generates and distributes the artifact.** Guarantees byte-identical
content with no determinism burden on generator authors, and makes version skew
impossible. Rejected because it distributes a derivative of a copyrighted ROM,
and because it puts a per-game content pipeline inside a server that ADR 0009
deliberately keeps free of game knowledge.

**Ship pre-generated stage packs.** Generate offline, treat each output as
ordinary content. Simple, and compatible with today's asset pipeline. Rejected
because a fixed pool is memorisable, which removes the point — the challenge is
reading an unfamiliar layout under pressure. Keep as the fallback if a generator
proves not to be reproducible.

**Generate a savestate rather than a ROM.** Would reuse the ADR 0002 entry path
unchanged. Rejected because savestate-only generation can vary enemy placement
and initial state but not level geometry, and does nothing for Doom.

## Consequences

**Constrains work happening now.** The challenge-assignment message needs an
optional `seed` field from the start — fields are never removed, so adding it
early costs nothing and avoids a wire break mid-slice. `EntryPoint` must be able
to name a derived artifact, not only a file already in the store. `asset-store`
keys on a derivation, not only on a shipped bundle's hash.

**Scopes ADR 0002.** That ADR rejects ROM patching *as the way to enter a vanilla
stage*. It does not forbid generated content, which patches for a different
reason. Write it narrowly.

**Generators are deterministic and versioned.** Same seed, same bytes, every
machine. A generator change is a version bump, never an edit in place, or old
runs stop reproducing and live sessions abort on hash mismatch.

**Harder.** Generators cannot be integration-tested with `session-fake`; they
need a real base ROM, so their tests are golden-hash tests run manually.

**Qualifies invariant 5.** Adding a *game* stays a data change. Adding a
*generator* is code. What survives is the part that matters — nothing generated
reaches `challenge-core` — but "new content is always data" is no longer true.

**Spectating leaks layout.** ADR 0003 accepted that a finished player watching an
in-progress one is harmless "in time trial", and anticipated a stage type where
it would matter. This is that stage type: on a fresh layout, reading it *is* the
skill. If a generated challenge allows retries, a player who dies early and
spectates gains a real advantage. Resolve when generation is built — delay
buffer, or no spectating on retryable generated stages.
