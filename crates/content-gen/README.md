# content-gen

Content derivation, and the only crate that touches ROM bytes. Two transforms,
no game knowledge, no emulator:

| | | |
|---|---|---|
| **a hack** | a contributor's BPS patch applied to the player's own dump | ADR 0020 |
| **a roll** | declared values re-decided from a seed, in place | ADR 0013, ADR 0015 |

They compose in the useful direction: a human designs a level in an editor, and
the engine rerolls the enemies inside it. That is the expected shape of a
generated challenge, not an exotic one.

```sh
cargo test -p content-gen           # unit tests, plus golden hashes if the ROMs are here
```

## What is where

| Module | |
|---|---|
| `bps` | apply a BPS patch, checking all three CRC32s it carries |
| `slots` | the patch-slot schema: offsets, widths, pools, ranges, groups |
| `roll` | the derivation key, the generator, and writing a roll into a ROM |
| `console` | header size and checksum fixup, per console family |
| `hash` | SHA-1 (identity) and CRC32 (BPS's own accident check) |

`tests/golden.rs` is the one that matters. It reproduces the three SMB3 hashes
in [`docs/games/smb3.md`](../../docs/games/smb3.md), which were recorded against
a Python script before this crate existed — a different implementation, in a
different language, from a table a human wrote down. It also checks the SNES
checksum rule against a real DKC dump. A checkout without ROMs skips them and
says so.

## Three things not to undo

**A pool's duplicates are weights.** `[0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86,
0x87]` is a coin flip on whether a goomba is replaced, then an even choice among
four bros. Deduplicating that turns a 50% roll into a 20% one and nothing says
so. Duplication is the only way this schema can express a non-uniform roll at
all.

**The generator is written out by hand.** splitmix64 seeded from one documented
string, not `rand`. Every player in a session derives the artifact locally and
the server aborts on a hash mismatch (ADR 0013), so the stream is a wire format
in everything but name — and `rand`'s never promised to be stable across
versions.

**Nothing structural.** No inserted objects, no new geometry, no relocated or
grown tables. ADR 0015 rules that out permanently rather than pending, and ADR
0019 made it a correctness requirement: an entry state is rebound onto a derived
ROM by rewriting a hash in the state's header, and that transplant is only sound
because the derived ROM has the same length, mapper and geometry. A structural
patch would break a rebound state *silently*, which is worse than the loud
refusal it replaces.

## Boundaries

`challenge-core` and `session-api` never learn this crate exists. A derived ROM
is an ordinary piece of content, and whoever prepares a stage
(`session-libretro::prep` today) hands the bytes to a `StageRunner` that has no
idea where they came from.

The dependency list is `serde` and nothing else, on purpose: ADR 0015 put the
engine in its own crate so that golden-hash tests and contributor tooling could
run it without linking a libretro core, an audio backend or a window. Anything
added here that a core would pull in undoes the reason the crate exists.

DOS is refused by name. A BPS patch against a zip bundle is meaningless — the
bundle's hash depends on how it was zipped — and a DOS alternative is a
member-level overlay, waiting on the member hashing `asset-store` owes
(ADR 0018, ADR 0020).
