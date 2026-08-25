# 0015. Scope custom-challenge ROM generation to declarative value-substitution patches

Date: 2026-08-25
Status: Accepted

## Context

ADR 0013 established the mechanism for per-attempt generated content: a
server-issued seed, client-local deterministic generation, a derivation key
(`base_rom_hash, generator_id, generator_version, seed`) that `asset-store`
caches artifacts under, and a hash compare across players that aborts the
challenge on mismatch. It did not settle *how* a generator turns a seed into
an artifact, and its own reasoning assumed that was inherently per-game code
("an SMB3 level writer knows about SMB3 object lists"). For the specific
feature this ADR is about, that assumption is wrong, and it matters why.

The product goal is that individual contributors add support for a favorite
game entirely through a manifest (invariant 5). That bar should also hold for
*custom challenges within a game* — a contributor building "SMB3, but the
enemy course rerolls enemy type and count" should be able to express that
declaratively and lean on shared engine code, not write a bespoke patcher in
Rust.

A useful class of custom challenge is narrower than "generate a ROM": it is
"reroll the value already sitting at a byte offset in an otherwise-untouched
ROM" — the enemy type at an existing spawn slot, an item at an existing
pickup, which of several existing rooms a door leads to. This class needs no
relocation, no free-space management, and no understanding of a game's
compressed formats. It is a table of `(offset, width, legal-value-source)`
triples plus a seed, and that is genuinely generic — a contributor doesn't
need to write code, they need to find offsets (via disassembly) and declare
what values are legal there.

A harder class — inserting new objects, generating new room geometry, growing
a table beyond the space it was allocated — is not generic. It requires
per-game structural knowledge (compression, pointer tables, free space) to
avoid producing a corrupt or uncompletable stage. Supporting it would mean
exactly the kind of per-game code invariant 5 exists to prevent.

## Decision

Custom challenges are limited to **declarative value-substitution**: a
manifest lists **patch slots**, each naming an offset, a width/endianness,
and a value source — either a numeric range or an explicit pool of legal
values. Several slots may share a named group whose pool is consumed by
permutation rather than independent sampling (e.g., shuffling which enemy
occupies which of an existing set of spawn slots, rather than rolling each
slot independently) — still pure value substitution, no new slots created.

One generic, seeded engine reads a slot list and a seed and produces the
patched ROM: pick a value per slot deterministically from the seed, write it
into an otherwise-untouched copy of the base ROM. No per-game code is
involved in that step. The engine also performs per-console-family
checksum/header fixup where the ROM format requires one (SNES's internal
header checksum; NES has none). That fixup is keyed off the console family
the base manifest already declares — a small, closed set reviewed once per
new console, not something that grows per game or per contributor.

**Structural or geometric ROM generation is explicitly out of scope.** A
custom challenge that needs new objects, new geometry, or relocated/expanded
data cannot be expressed in this manifest schema. There is no partial or
opt-in path for it. If it's ever wanted, that is a new decision superseding
this one — not an extension of it.

This engine lives in its own crate, `content-gen`, rather than inside a
libretro harness crate. It has no libretro/FFI/audio/windowing dependency —
it is a pure `(rom_bytes, patch_slots, seed) -> patched_rom_bytes` transform
plus the checksum step — so it can be exercised with golden-hash unit tests
and reused by contributor-facing tooling (e.g., a CLI that previews a patched
ROM) without linking an emulator core. Harness crates depend on `content-gen`,
never the reverse; a harness only ever receives an already-patched ROM
through the ordinary `EntryPoint` path and has no idea it was generated,
consistent with 0013 and with keeping adapter vocabulary out of shared types.

Whether a given offset/value is *safe* — doesn't crash the level, doesn't
make it uncompletable — is the manifest author's responsibility, established
by playtesting during authoring. This is the same trust boundary a manifest's
RAM-watch addresses already sit on. The engine only guarantees the written
value is one of the declared legal ones, not that the declared ones are good.

## Alternatives considered

**Per-game generator code** (0013's original framing, and this project's own
initial answer earlier in this discussion). Rejected: it fails invariant 5
for exactly the case that matters most to the project's contribution model,
and it's unnecessary — the wanted feature set is value substitution, not
structural generation.

**Support both value-substitution and structural generation now, the latter
behind an opt-in flag.** Rejected on explicit instruction: no unbuilt opt-in
path. The schema should describe only what the engine actually does.

**Embed the patch engine inside a libretro harness crate instead of a
standalone crate.** Rejected: the engine has no emulator dependency, needs to
run in contributor tooling and golden-hash tests without one, and 0013
already named `content-gen` as generator logic's home. Putting it there keeps
that boundary intact instead of tangling it with FFI/audio/input code.

## Consequences

**Narrows one specific claim in 0013.** Its stated consequence "adding a
generator is code" is not true for this class: the shared engine is written
once, and adding a *custom challenge* is a manifest change. 0013's core
mechanism (seed, derivation key, client-local determinism, hash-mismatch
abort) is unaffected and still stands.

**The manifest schema needs a documented `patch_slots` shape** — offset,
width/endianness, and a closed list of `value_source` kinds (range, pool,
grouped-pool permutation). That belongs with the rest of Phase 2's manifest
schema work, not duplicated here (per this project's own convention of
encoding rules as schema rather than prose).

**Permanently rules out** any custom challenge needing new geometry or
inserted objects, unless a future ADR supersedes this one. Contributors
hitting that wall are hitting an intentional boundary.

**Gives `content-gen` scope before Phase 3.** CLAUDE.md's crate table still
describes it as not existing until Phase 3's procedural generators. This ADR
means it has a reason to exist earlier, scoped narrowly to the generic patch
engine — worth reconciling when the crate is actually created.

**A small, explicit per-console checksum table is needed** as new consoles
are added — reviewed once per console family, not per game.
