# Super Mario Bros. 3 (NES)

The manifest is [`manifests/smb3.toml`](../../manifests/smb3.toml). This
document is the part that does not belong in it: where each address came from,
what is playtested and what is only documented, and the reasoning behind the
completion rule.

This used to be `crates/smb3-harness`, the Phase 1 harness — ~900 lines of
deliberately SMB3-specific Rust. ADR 0016 turned all of it into the manifest;
the code that runs it is `session-libretro`, which has no idea which game it is
running.

## Prerequisites

1. **Rust toolchain.** `rustup` from <https://rustup.rs>, plus the MSVC linker —
   Visual Studio Build Tools with "Desktop development with C++". Every
   dependency is pure Rust, so nothing else needs a C compiler.
2. **A NES core**, in `cores/` (the manifest's `core_dir`):
   - `mesen_libretro.dll` — most accurate, best RAM/savestate behaviour, and
     the only one confirmed to publish the memory map the card detector needs
   - `fceumm_libretro.dll` — lighter, very widely used
   - `nestopia_libretro.dll`

   From the RetroArch core downloader, or
   <https://buildbot.libretro.com/nightly/windows/x86_64/latest/>. The
   directory must hold exactly one `*_libretro.dll`; ambiguity is an error
   rather than a guess.
3. **The ROM**, at `assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes`.

## Run

```sh
cargo run --release -p session-libretro --bin harness -- --game smb3 --stage 1-1

# what stages there are
cargo run --release -p session-libretro --bin harness -- --game smb3 --list-stages

# 600 frames with no window, for checking a rule against the real ROM
cargo run --release -p session-libretro --bin harness -- --game smb3 --stage 1-1 --frames 600

# a custom challenge: derive the hack from its patch, roll its slots, and enter
cargo run --release -p session-libretro --bin harness -- \
    --game smb3 --challenge sun-storm-survival --seed 3
```

| Key | |
|---|---|
| Arrows | d-pad |
| Z / X | B / A |
| Enter / RShift | Start / Select |
| F1 / F2 | save state / re-enter the stage |
| F5 | dump every watched address now |
| F9 | reset core, restart frame count |
| Esc | quit |

Frame counting restarts at 0 every time the stage is entered, so the number in
`STAGE CLEARED at frame N` is frames since stage entry — not since launch. That
is what makes two players comparable (ADR 0005).

The stage label comes from the entry point, not from RAM. SMB3 has no
documented "current level number" at all: a level is identified by the map tile
you entered it from, so the map position pair *is* the level id, and it is only
ever a cross-check.

## RAM addresses

This is the part most likely to be wrong, and the part worth checking first.
Every one of these is a `[watch.<name>]` in the manifest, with its provenance in
the `note` beside it.

| Address | Meaning | Confidence |
|---|---|---|
| `$0736` | Mario lives | verified |
| `$0727` | world − 1 | verified |
| `$0014` | "flag to return to map" | verified |
| `$0078` / `$0079` | map position, used as the level id | verified |
| `$070A` | level object set | verified |
| `$0578` | powerup/form (00-06: small, super, fire, raccoon, frog, tanooki, hammer) | documented (Data Crystal), not yet playtested |
| `$0552` | hit-invulnerability timer; rising from 0 means Mario just took damage | documented (Data Crystal), not yet playtested |
| `$0510` | pipe transit countdown; rising from 0 means entering/exiting a pipe | documented (Data Crystal), not yet playtested |
| `$05F4` | stomp counter; increments on a stomp kill only — fireball/shell/star kills don't touch it | documented (Data Crystal), not yet playtested |
| `$7D9C` | card count; increments when Mario is awarded a card at a level's end | verified (playtested) |

Any of them can be retargeted or switched off without a rebuild:

```sh
harness --game smb3 --addr lives=0x0736 --addr powerup=off
harness --game smb3 --list-addrs
```

And any address can be watched raw, including a range to sweep for something
with no known address yet:

```sh
harness --game smb3 --watch 0x0014,0x0727,0x0736,0x0578,0x7D9C
harness --game smb3 --watch 0x0090-0x00ff
```

An SMB3 RAM map with other addresses to test is at
<https://datacrystal.tcrf.net/wiki/Super_Mario_Bros._3/RAM_map>.

## Reaching cartridge WRAM

The card count lives at `$7D9C`, in the `$6000-$7FFF` cartridge WRAM — and
`retro_get_memory_data` cannot reach it. Mesen exposes exactly one region,
`RETRO_MEMORY_SYSTEM_RAM` (2048 bytes, the internal RAM at `$0000-$07FF`);
querying `RETRO_MEMORY_SAVE_RAM` and every other region id returns nothing,
because libretro only classifies WRAM as "save RAM" when it's battery-backed
and SMB3's isn't.

The route that works is `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` (`36 | 0x10000`),
which the frontend answers. Mesen publishes 64 descriptors covering
`$0000-$1FFF` (internal RAM and its mirrors) and `$6000-$7FFF` (cartridge WRAM)
as flat 256-byte pages; `$2000-$5FFF` is PPU/APU/IO registers and is correctly
absent.

**This is why the manifest sets `prefer = "memory_map"`.** It is the one field
in `[runner.libretro.memory]` whose being wrong is invisible: with
`prefer = "system_ram"` every low address still resolves, and the card detector
simply never fires. If a core publishes no map at all, startup says which
addresses went unreadable and stage completion falls back to the `$0014`
heuristic alone.

Only descriptors with `select == 0` are resolved. The `select`/`disconnect`
bit-matching form in libretro.h is deliberately not implemented: no NES core
needs it here, and a wrong guess would return the wrong byte rather than
nothing.

```sh
# what this core actually publishes
harness --game smb3 --dump-memory-map
```

## Stage completion

A card increase (`$7D9C`) is the primary success signal: it fires seconds before
`$0014` moves, and it never fires on death, so no lives check is needed to
disambiguate it. Measured against a real run, the card lands at frame 11 and
`$0014` doesn't rise until frame 359 — a 348-frame (5.8 s) difference, all of it
noise that used to be counted as stage time.

Not every level awards a card — fortresses, castles, and airships don't run the
card-match bonus — so this only covers the levels that do.

For everything else, `$0014` rising from zero means the game is leaving the
level, and the manifest reports that as `stage_exited` with reason `unknown`.
That reason is honest rather than provisional: a card-less level cleared
normally is genuinely indistinguishable from a death by watching `$0014`.

The two rules are ordered so the card wins on a frame where both could fire, and
a terminal event ends the attempt — so the ordinary `$0014` exit a few seconds
after a card reports nothing. `[[attempt.reset_on]]` re-opens scoring when
`$0014` falls again, which is the hand-off to the map finishing.

That last part is load-bearing and was a real bug. When the "already resolved"
latch held for the whole process instead of for one attempt, a single card early
in a run silenced every later death. `a_death_after_a_card_still_exits` and
`two_deaths_both_report` in `session-libretro/src/manifest_tests.rs` pin it down,
and they run against this manifest.

Closing the card-less gap — and unifying this with something that also answers
for Doom, which has no addressable core RAM — is what unwritten ADR 0014 has to
do. Do not build scoring on the `unknown` reason.

## Tests

```sh
cargo test -p session-libretro
```

Every behavioural test the old harness had is in
`crates/session-libretro/src/manifest_tests.rs`, driving this manifest rather
than a fixture. They need no ROM, no core and no savestate. Anything needing a
real ROM is a manual test, or `--frames`.

## Stages and challenges

Six entry points, all in world 1, all cleared by the standard `stage_cleared`
signal — which here is the card rule, so it holds only for card-awarding levels.
A fortress or an airship awards no card and would need ADR 0014's missing
detector before it could carry a `cleared = true` criterion.

| Stage | Runs on | Challenges on it |
|---|---|---|
| `1-1` | `base` | time trial, survival, and `1-1-stomp-rush` |
| `1-2` | `base` | time trial, survival |
| `1-3` | `base` | time trial, survival |
| `1-4` | `base` | survival only — forced-scrolling, so everyone finishes in the same number of frames |
| `sun-storm` | `hack1` | time trial, survival — both generated |
| `bro-roulette` | `hack1` | time trial, survival — both generated |

Nothing above is declared as a constraint anywhere: a stage with no time trial
simply has no time-trial `[[challenge]]` naming it (ADR 0017). Read the current
list back with `--list-challenges` rather than trusting this table.

The last two run on a hand-authored hack rather than on the base ROM, and their
challenges carry patch slots. They are written up under
[generated content](#generated-content), which is where the offsets and pools are
argued for.

`1-1-stomp-rush` is a worked example of a criterion other than "reach the end":
five `enemy_defeated` events and the clock stops, wherever the player is. It is
**not playtested** — `$05F4` counts stomps only, so a fireball, shell or star
kill does not advance it, and five stomps means five stomps.

## Generated content

Two custom challenges, authored *before* `content-gen` was written so that the
engine answered to real slot tables rather than a guess. Everything below was
derived and verified against the ROM, and all of it is now in
`manifests/smb3.toml` and runs. ADR 0015 scopes a patch to declarative value
substitution; ADR 0019 depends on that scope holding.

What is *not* done is playing one: no human has taken either challenge to its
criteria on a rolled ROM. The mechanism below is measured; the level design on
top of it is not.

Both challenges run on a **hand-authored hack**, not on the base ROM (ADR 0020).
Their levels were built in a level editor, so every object list is the
contributor's own and every slot in it exists to be rolled. That is the shape
CLAUDE.md calls expected rather than exotic — a human designs the level, the
generator rerolls enemies inside it — and it is why the base-ROM tables that used
to be here (World 2 Sand Trap, World 8 hand trap) are gone. Pinning a sun to
column `$5B` so it spawned already inside its activation gate, and parking a
fifth bro off the end of the level so that only four could be counted, were both
workarounds for not owning the level. Owning it removes the constraint instead of
routing around it.

### The two ROMs

| | sha1 | |
|---|---|---|
| base | `6bd518e85eb46a4252af07910f61036e84b020d1` | `assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes` |
| hack | `55c34fe15e7753ee6781f7f5de477f5610fc3428` | `assets/patches/smb3-rom-hack1.bps` applied to it |

The patch is BPS and carries the base's CRC32, so it refuses a different revision
at apply time. No derived ROM is in the repo and none should be: the harness
applies the patch whenever a stage names the hack, and caches the result under
`build/derived/`, which is ignored.

```sh
cargo run -p session-libretro --bin harness -- --game smb3 --stage sun-storm --frames 1
```

The hack rewrites World 1's structure tables so both custom levels are entered
from the World 1 map, and rewrites most of the object bank at `$C000-$DFFF`. Its
BPS metadata names the editor it came out of:
`{"project":"SMB3-hack2","tool":"Bank12"}`.

### The disassembly reassembles byte-for-byte

`vendor/smb3-disassembly/`, NESASM 3.01:

```sh
cd vendor/smb3-disassembly/smb3-master && ./nesasm.exe smb3.asm
```

Output is identical to `assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes`,
`sha1 6bd518e85eb46a4252af07910f61036e84b020d1`. That is stronger provenance
than a database lookup, and `[rom].sha1` in the manifest is still empty — a
loose end.

**The disassembly is still what supplies the pool**, and that is the part a diff
cannot give you. A diff yields an offset, a width and exactly one sample value;
the *legal values* — the substance of a reroll — come from the `OBJ_*` constants
in `smb3.asm`, and whether a value is safe comes from reading the routines that
consume it. What changed is where the *offsets* come from: they are read out of
the hack rather than derived by editing source and reassembling, because these
levels do not exist in the source.

### Finding the slots in a hack

Worth writing down, because it is the same procedure for the next hack.

`PRG/prg006.asm` is `.bank 6 / .org $C000`, so the object bank maps one to one
onto ROM offsets `0xC000-0xDFFF` after the iNES header — an object list's CPU
address *is* its offset. The bank is a plain concatenation of lists, each one a
lead byte, then three-byte `(id, column, row)` entries, then `$FF`. Parsing it
forward from `0xC000` recovers every list in either ROM; on the base it
reproduces `SandTrap.asm` at `$C843` exactly, which is the check that the parse
is aligned before trusting it on a hack.

Two lists in the hack are then unmistakable — three `$AF` and a card, four `$72`
and a card — and both are confirmed twice over:

* `W1_ObjSets` (a `.word` table at `$B452`, located by searching for
  `W1_ByRowType`'s bytes) points entry 0 at `$C6BC` and entry 8 at `$C6CA`. Both
  entries' row types were changed by the hack, which is how the levels became
  enterable.
* Each entry state carries the pointer the map had already prepared, in cartridge
  WRAM at `Level_ObjPtrOrig` (`$7EBB`): `$C6BC` in one, `$C6CA` in the other.

### Sun Storm — `W2_32`, entered from the World 1 map

Object list `$C6BC`: three Angry Suns and the end-level card. The roll decides
how many of the three are actually suns, and the answer is one, two or three.

| Offset | Byte | Is | Roll |
|---|---|---|---|
| `0xC6BD` | slot A id, column `$01` | `$AF` `OBJ_ENEMYSUN` | `pool = [0xAF, 0xBB]` |
| `0xC6C0` | slot B id, column `$03` | `$AF` `OBJ_ENEMYSUN` | `pool = [0xAF, 0xBB]` |
| `0xC6C3` | slot C id, column `$05` | `$AF` `OBJ_ENEMYSUN` | `pool = [0xAF]` — always a sun |

Columns and rows are **not** slots. The columns are load-bearing (finding 1
below) and a sun repositions itself anyway, so patching either adds risk for
nothing.

**`$BB` is `OBJ_CANCELEVENT`, and it is how an empty slot is spelled** under pure
value substitution. Object IDs from `$B4` to `$BB` are handled before any actor
exists (`prg005.asm:5053`): the ID is turned into a `Level_Event` number and the
routine returns. `$BB` becomes event 8, whose handler is `LevelEvent_Cancel`,
which does nothing but clear `Level_Event`. Nothing spawns, no object slot is
taken, no sprite appears. It is the closest SMB3 has to deleting a list entry
without deleting it — which ADR 0015 does not allow.

The distribution follows from the pools: **25% one sun, 50% two, 25% three.** A
uniform one-third each would need the three slots to share a single roll, and the
schema has no way to correlate them — `mode = "permutation"` fixes the multiset,
which is the opposite of what is wanted here. Biasing toward more suns is free by
weighting a pool (`[0xAF, 0xAF, 0xBB]` gives 11% / 44% / 44%); changing the
*shape* of the distribution is not.

Slot C is the fixed one deliberately, and it is the highest column rather than the
lowest. A sun activates when accumulated scroll carries it into the gate, so the
rightmost sun activates first — roughly 32 pixels of scroll ahead of slot B and 64
ahead of slot A. Fixing slot C means every roll delivers its first sun at the same
point in the level, and the roll only decides what follows it.

### Bro Roulette — `W1_03`, entered from the World 1 map

Object list `$C6CA`: four goombas, a hidden warp object, and the end-level card.
Each goomba independently either stays a goomba or becomes one of the four bro
variants.

| Offset | Byte | Is | Roll |
|---|---|---|---|
| `0xC6CB` | goomba 1 id, column `$02` row `$13` | `$72` `OBJ_GOOMBA` | the pool below |
| `0xC6CE` | goomba 2 id, column `$0A` row `$19` | `$72` `OBJ_GOOMBA` | ″ |
| `0xC6D1` | goomba 3 id, column `$0D` row `$19` | `$72` `OBJ_GOOMBA` | ″ |
| `0xC6D4` | goomba 4 id, column `$0D` row `$15` | `$72` `OBJ_GOOMBA` | ″ |

The pool is the same for all four and is a **multiset, not a set**:

```
[0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87]
```

Four goombas against one each of `OBJ_HAMMERBRO`, `OBJ_BOOMERANGBRO`,
`OBJ_HEAVYBRO` and `OBJ_FIREBRO`. Drawn uniformly, that is exactly the two-stage
roll the challenge wants — a coin flip on whether this goomba is replaced, then an
even choice among the four variants — expressed without the schema growing
weights.

**`content-gen` must therefore sample a pool with its duplicates, not
deduplicate it.** Small decision, large blast radius: duplication is the only way
a manifest can express a non-uniform roll at all, and an engine that dedupes turns
this table into 20% goomba without saying so.

This is not a permutation group. A group would force all four variants to appear
exactly once, which was the old World 8 table's constraint and is not this one:
two hammer bros is a legal outcome, and so is four goombas — 6.25%, a real roll
rather than a failure.

Two things have not changed and should not be silently revisited:

- **A rolled quantity still cannot reach `complete_when`.** It is static TOML,
  `enemies_defeated = N` means *at least* N, and nothing can reference the seed.
  Here the enemy *count* is fixed by the level and only the *types* roll, so the
  question does not arise — but it is still the first real boundary a generator
  hits, and the next challenge that wants a rolled count will hit it.
- **"Kill all four" means "stomp all four."** `enemy_defeated` comes from `$05F4`,
  which counts stomps only; a fireball, shell or star kill does not advance it.
  Accepted for now, and better SMB3 detectors are owed anyway (ADR 0014).

Two notes on the level as authored. Neither is the patch table's business, and
both are worth knowing before this is playtested:

- Goombas 3 and 4 share column `$0D`. Both still spawn: the look-ahead is snapped
  to a 16-pixel boundary and the level-load pass runs the spawn scan twice per
  step (finding 2), so one column window admits two objects. Mid-level at full
  running speed it would admit fewer — these are on the starting screen, which is
  what makes it safe.
- `$C6D7` holds `$07` `OBJ_WARPHIDE` at column `$96`, two columns before the card.
  It only fires with `Player_Behind` set, so it is a deliberate secret rather than
  an accident — but taking it ends the level with no card, and it is gated on
  `Map_Got13Warp`, which persists per save file. Two players with different saves
  therefore have different routes available, which is a scoring problem rather
  than a bug.

### Golden hashes

Each is the hack with its seven rollable object-list bytes written — four of them
actually differing in any given roll, since a slot may roll back to what it
already held. The input is `55c34fe1…`, never the base ROM.

| Roll | Suns | Bros | sha1 |
|---|---|---|---|
| `golden` | `BB AF AF` — two | `81 87 72 86` | `7b6989c1d511ade0a33a7e50aa035cade734d799` |
| `b` | `BB BB AF` — one | `72 72 82 81` | `10e69a037e821fea5a5a4c0822ff2ffe58bef4fe` |
| `max` | `AF AF AF` — three | `87 86 82 81` | `6333b57ff78e0bce43d8114d08279bbf93689918` |

`golden` is `content-gen`'s first test and depends on none of the unresolved
questions. `b` and `max` exist to prove more than one roll works: `b` is the
minimum sun count with the maximum goomba count, `max` the reverse.

**All three are now `content-gen`'s golden tests**
(`crates/content-gen/tests/golden.rs`), and the fact that they were recorded
first, by `tools/smb3-roll.py`, is what makes them worth anything: a different
implementation, in a different language, from a table a human wrote down. A
round-trip against the crate's own output would have proved nothing.

They pin the engine's *writing* — offsets, header skip, widths, checksum — and
not its *choosing*, so each is applied as an explicitly chosen roll rather than
drawn from a seed. `tools/smb3-roll.py` could never have pinned the choosing
either: its `--seed` is Python's RNG, not ADR 0013's derivation key.

```sh
cargo test -p content-gen                  # the three hashes, if the ROMs are here
cargo run -p session-libretro --bin harness -- \
    --game smb3 --challenge bro-roulette-time-trial --seed 3 --frames 30
```

The harness derives the hack from the patch, rolls the challenge's slots, rebinds
the entry state and enters — and prints every byte it wrote on the way. The
artifacts land in `build/derived/`, which is a cache and is not in the
repository. `tools/smb3-roll.py` is now redundant and kept only as the thing the
hashes were first taken with.

### The manifest block

Both slot tables are now in `manifests/smb3.toml`, along with the `[[rom]]` array
and the two stages, and the loader reads them. What follows is the same thing,
kept here because this document is where the offsets and pools are argued for and
a reader should not have to hold two files open.

```toml
schema_version = 3

[[rom]]
id = "base"
name = "Super Mario Bros. 3 (USA) (Rev 1)"
path = "assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes"
sha1 = "6bd518e85eb46a4252af07910f61036e84b020d1"
header = "ines"

[[rom]]
id = "hack1"
name = "SMB3: Gamer Trials"
from = "base"
patch = "assets/patches/smb3-rom-hack1.bps"
sha1 = "55c34fe15e7753ee6781f7f5de477f5610fc3428"

# Every existing [[stage]] also gained `rom = "base"` — twelve one-line changes
# across the three manifests, which is what made this a breaking change.
#
# The shipped manifest still leaves [[rom]].sha1 off the *base*, and that stays a
# loose end: prep enforces a declared hash, and no dump has been verified to
# declare one against. The disassembly reassembling to 6bd518e8… is the strongest
# evidence there is, and it is evidence about this checkout, not about a player's.

[[stage]]
id = "sun-storm"
name = "Sun Storm"
rom = "hack1"
entry = { state = "assets/states/smb3_sun-storm.state" }
note = "captured on the World 1 map, on the tile that enters W2_32"

[[stage]]
id = "bro-roulette"
name = "Bro Roulette"
rom = "hack1"
entry = { state = "assets/states/smb3_bro-roulette.state" }
note = "captured on the World 1 map, on the tile that enters W1_03"

# --- Sun Storm -------------------------------------------------------------
# One to three Angry Suns. The two kinds duplicate the table rather than share
# it: patch_slot hangs off [[challenge]], so the copies can drift. Unresolved,
# and the strongest argument for hanging it off [[stage]] instead.

[[challenge]]
id = "sun-storm-survival"
name = "Sun Storm"
stage = "sun-storm"
kind = "survival"
complete_when = { cleared = true }
generator_version = 1

[[challenge.patch_slot]]
offset = 0xC6BD                      # slot A, column $01
width = "u8"
values = { pool = [0xAF, 0xBB] }     # a sun, or nothing at all

[[challenge.patch_slot]]
offset = 0xC6C0                      # slot B, column $03
width = "u8"
values = { pool = [0xAF, 0xBB] }

[[challenge.patch_slot]]
offset = 0xC6C3                      # slot C, column $05 — always a sun
width = "u8"
values = { pool = [0xAF] }

[[challenge]]
id = "sun-storm-time-trial"
name = "Sun Storm"
stage = "sun-storm"
kind = "time_trial"
complete_when = { cleared = true }
generator_version = 1

# ... the same three patch_slot entries again.

# --- Bro Roulette ----------------------------------------------------------
# Four goombas, each independently a goomba or one of four bros. The repeated
# 0x72 entries are the weighting, and are not a mistake.

[[challenge]]
id = "bro-roulette-time-trial"
name = "Bro Roulette"
stage = "bro-roulette"
kind = "time_trial"
complete_when = { enemies_defeated = 4 }
generator_version = 1
note = "the clock stops on the fourth stomp; reaching the card is not required"

[[challenge.patch_slot]]
offset = 0xC6CB                      # goomba 1, column $02
width = "u8"
values = { pool = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87] }

[[challenge.patch_slot]]
offset = 0xC6CE                      # goomba 2, column $0A
width = "u8"
values = { pool = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87] }

[[challenge.patch_slot]]
offset = 0xC6D1                      # goomba 3, column $0D
width = "u8"
values = { pool = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87] }

[[challenge.patch_slot]]
offset = 0xC6D4                      # goomba 4, column $0D
width = "u8"
values = { pool = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87] }

[[challenge]]
id = "bro-roulette-survival"
name = "Bro Roulette"
stage = "bro-roulette"
kind = "survival"
complete_when = { enemies_defeated = 4 }
generator_version = 1

# ... the same four patch_slot entries again.
```

Both levels award an end-level card, so `cleared = true` is available on either —
which the base-ROM tables could not offer, and which is why neither challenge
needs `survive_seconds` any more. `enemies_defeated = 4` is on Bro Roulette
because that is what the challenge is about; swap in `cleared = true` if it should
be a race to the card instead.

## Six mechanism findings

All confirmed in the disassembly, none of them guessable from a RAM map. They
are what makes the two challenges above work, and they generalise to any SMB3
generator.

1. **The Angry Sun activates on accumulated scroll, not on where it was placed.**
   `Sun_WaitFarEnough` (`PRG/prg005.asm:2718`) gates on the sun's *live* position:
   `CMP #$05 / BNE` against `Objects_XHi` — equality, not "at least" — plus
   `Objects_X >= 176`. But `ObjNorm_Sun` moves the sun by `Level_ScrollDiffH`
   every frame (`prg005.asm:2727`), so it tracks the camera: its absolute X is
   *spawn column × 16 + total scroll*. A sun therefore activates once the level
   has scrolled it into `[1456, 1535]`, and the earlier its column, the more
   scroll that takes — the rightmost sun in a group activates first. A sun placed
   *past* the gate never activates, because scrolling right only carries it
   further away.

   This corrects what used to be recorded here. The old base-ROM table pinned its
   suns to columns `$5B`–`$5D` so they spawned already inside the gate; that
   worked, but it was one solution rather than the rule. Sun Storm's suns sit at
   columns `$01`–`$05` and activate about five and a half screens in, staggered
   two columns apart, which is the same mechanism used deliberately.
2. **Objects spawn from a look-ahead of +272 pixels** (`LOSBS_LookAhead`,
   `prg005.asm:4918`) **snapped down to a 16-pixel boundary** (`AND #$f0`), and
   **only one ordinary object spawns per call** — the normal path falls through to
   `RTS` (`prg005.asm:5155`). So a column window is open for as many frames as the
   camera spends inside it, and two objects on one column both spawn if it dwells
   two frames. At level load that is guaranteed: `prg005.asm:6385` walks the
   starting screen in 16-pixel steps and calls the spawn scan **twice per step**,
   which is why Bro Roulette's two goombas on column `$0D` both appear. Mid-level
   at speed it is not guaranteed, and consecutive columns are still the safe
   pattern.
3. **The object list must stay sorted ascending by column.** The spawn scan stops
   at the first entry on a different screen, and `Level_ObjIdxStartByScreen` is
   built at load by walking the list in order. Any column pool must be bounded by
   its neighbours' values or everything after it silently stops spawning. Rolling
   only IDs, as both tables above do, cannot violate this.
4. **There are five object slots** for ordinary enemies (`LDX #$04`,
   `prg005.asm:5110`); the end-level card forces slot 6. If no slot is free the
   object is simply lost — the column alignment has passed and it is not retried.
   Sun Storm needs three of the five, Bro Roulette four.
5. **`Level_ObjectID` at `$0671`–`$0678` is the live actor ID table**, and
   `Objects_State` at `$0661`–`$0668`. This is a far better foundation for "kill
   all enemies" than the stomp counter, at least in a scroll-locked arena. Not
   proposed yet: a slot emptying conflates a kill with an off-screen delete.
6. **Object IDs `$B4`–`$BB` never become actors**, so one of them is how a value
   substitution spells "nothing here". `prg005.asm:5053` turns such an ID into a
   `Level_Event` number and returns before a slot is claimed. `$BB`
   `OBJ_CANCELEVENT` is event 8, whose handler clears `Level_Event` and does
   nothing else — a list entry that costs a frame's spawn opportunity and produces
   no object. It is the only way to roll an enemy *out* of a level without
   deleting a list entry, which ADR 0015 forbids.

## Capturing an entry state for a custom challenge

Both entry states here are captured **on the hack, on the World 1 map**, standing
on the tile that enters the level — not inside it. That is the shape to copy, and
it settles a question the base-ROM version of this document had to leave open.

A state captured *inside* a level carries `Level_ObjIdxStartByScreen`, which is
built from the object list when the level loads (finding 3) — so a mid-level state
carries an index built from the *unrolled* list and the rolled list is then read
against it. It probably works, and "probably" is the problem. A map state has no
index yet: the level loads fresh when the player walks in, from whatever bytes the
roll wrote. The cost is the frames a map entry takes, and it is worth them.

Nothing is rebound *onto the hack*. ADR 0019's rebind exists for a ROM the seeded
engine derived, which is the same length, mapper and geometry as what it came
from; a hand-authored hack is none of those, so a state is rebound only onto an
artifact derived from the ROM its own stage names, never from the base onto a
hack. These states name the hack's hash (`55C34FE1…`) and load onto the hack
directly — and then onto a *roll* of the hack, which is a rebind stage prep turns
on by itself because it knows how those bytes were made.

```sh
# derive the hack, roll a challenge on it, and enter with its own state
cargo run -p session-libretro --bin harness -- \
    --game smb3 --challenge sun-storm-survival --seed 3 \
    --frames 900 --watch 0x0671-0x0675
```

One command now, rather than three: the harness applies the patch, rolls the
slots, rebinds the state and enters. To play the level *unrolled* — which is how
its entry state was captured in the first place — leave `--seed` off, and it says
so rather than quietly running a rolled ROM.

Walk right off the map tile, and watch `$0671`–`$0675` fill as the suns take actor
slots. Rolling `b` (one sun) and `max` (three) against the same state is the
cheapest end-to-end check that a slot table does what it claims.