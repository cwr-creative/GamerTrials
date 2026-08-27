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

Four entry points, all in world 1, all cleared by the standard `stage_cleared`
signal — which here is the card rule, so it holds only for card-awarding levels.
A fortress or an airship awards no card and would need ADR 0014's missing
detector before it could carry a `cleared = true` criterion.

| Stage | Challenges on it |
|---|---|
| `1-1` | time trial, survival, and `1-1-stomp-rush` |
| `1-2` | time trial, survival |
| `1-3` | time trial, survival |
| `1-4` | survival only — forced-scrolling, so everyone finishes in the same number of frames |

Nothing above is declared as a constraint anywhere: a stage with no time trial
simply has no time-trial `[[challenge]]` naming it (ADR 0017). Read the current
list back with `--list-challenges` rather than trusting this table.

`1-1-stomp-rush` is a worked example of a criterion other than "reach the end":
five `enemy_defeated` events and the clock stops, wherever the player is. It is
**not playtested** — `$05F4` counts stomps only, so a fireball, shell or star
kill does not advance it, and five stomps means five stomps.

## Generated content

`content-gen` is not written. Two custom challenges were authored ahead of it,
so the engine answers to real slot tables rather than a guess, and everything
below was derived and verified against the ROM. ADR 0015 scopes a patch to
declarative value substitution; ADR 0019 depends on that scope holding.

### The disassembly reassembles byte-for-byte

`vendor/smb3-disassembly/`, NESASM 3.01:

```sh
cd vendor/smb3-disassembly/smb3-master && ./nesasm.exe smb3.asm
```

Output is identical to `assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes`,
`sha1 6bd518e85eb46a4252af07910f61036e84b020d1`. That is stronger provenance
than a database lookup, and `[rom].sha1` in the manifest is still empty — a
loose end.

Working from the disassembly was chosen over hacking a ROM and diffing it, for a
reason worth keeping: **the deliverable is a slot table, not a patch.** A diff
yields an offset, a width and exactly one sample value; the *pool* — the
substance of a reroll — is not recoverable from it. The disassembly supplies the
pool directly, as the `OBJ_*` constants. SMB3 level editors re-serialise and
repack object lists, producing structural diffs that ADR 0015 puts permanently
out of scope.

Every offset below was derived by changing one symbol in the disassembly,
reassembling, and diffing: each probe produced exactly one differing byte, and
the original value at each offset matched the source constant. The whole patch
was then built two independent ways — assembled from edited source, and
byte-patched into the base ROM — and the two were identical. Offsets are into the
base ROM **after the 16-byte iNES header**, per the schema.

### Sun Storm — World 2 Sand Trap, survival, 30 seconds

One to three extra Angry Suns, rolled into spawn slots that already exist.
`complete_when = { survive_seconds = 30.0 }`, which is count-independent — so a
rolled count causes no criterion trouble here. Source: `PRG/objects/SandTrap.asm`.

| Offset | Byte | Was | Roll |
|---|---|---|---|
| `0xC856` | slot A id | `$5D` `OBJ_TORNADO` | `pool = [0xAF]` — always a sun |
| `0xC857` | slot A column | `$51` | `pool = [0x5B]` |
| `0xC859` | slot B id | `$6E` `OBJ_PARATROOPAGREENHOP` | `pool = [0xAF, 0x6E]` |
| `0xC85A` | slot B column | `$7E` | `pool = [0x5C]` |
| `0xC85C` | slot C id | `$6E` `OBJ_PARATROOPAGREENHOP` | `pool = [0xAF, 0x6E]` |
| `0xC85D` | slot C column | `$9E` | `pool = [0x5D]` |

Row bytes (`0xC858`, `0xC85B`, `0xC85E`) are deliberately **not** slots — a sun
repositions itself, so patching them adds risk for nothing.

Slot A's single-value pool is what makes the roll 1–3 rather than 0–3. If B or C
rolls back to its original paratroopa, that paratroopa still moves to screen 5,
because the column slot is fixed; harmless for a 30-second survival run, but a
behaviour change and not a no-op.

### Bro Roulette — World 8 Hammer Bro hand trap, time trial *and* survival

`PRG/objects/W8HTrap1.asm` already holds five bro slots containing all four
variants, so the pool is playtested by Nintendo rather than by us. Four slots are
permuted over `pool = [0x81, 0x82, 0x86, 0x87]` with `mode = "permutation"`; the
fifth is parked off the level. `complete_when = { enemies_defeated = 4 }`.

| Offset | Byte | Was | Roll |
|---|---|---|---|
| `0xD0B1` | bro 1 id | `$87` `OBJ_FIREBRO` | `group = "bro-shuffle"` |
| `0xD0B4` | bro 2 id | `$81` `OBJ_HAMMERBRO` | `group = "bro-shuffle"` |
| `0xD0B7` | bro 3 id | `$81` `OBJ_HAMMERBRO` | `group = "bro-shuffle"` |
| `0xD0BA` | bro 4 id | `$82` `OBJ_BOOMERANGBRO` | `group = "bro-shuffle"` |
| `0xD0BE` | bro 5 column | `$37` | `pool = [0x7F]` — parks it off-level |

The level is four screens wide (`LEVEL1_SIZE_04`), so column `$7F` never spawns.
The per-screen index table is built for 16 screens regardless of level width, so
this is safe.

Two decisions here were taken deliberately and should not be silently revisited:

- **The count is fixed at four, not rolled.** `complete_when` is static TOML and
  `enemies_defeated = N` means *at least* N. A rolled count cannot be expressed —
  set it to the minimum and a short roll wins early, set it to the maximum and a
  short roll is unwinnable. There is no arithmetic and nothing that can reference
  the seed, by design. **A rolled quantity cannot reach `complete_when`.** That is
  the first real boundary a generator hits, and it is unsettled.
- **"Kill all four" means "stomp all four."** `enemy_defeated` comes from `$05F4`,
  which counts stomps only. Accepted for now; better SMB3 detectors are owed
  anyway (ADR 0014).

### Golden hashes

Base ROM patched with sand trap `AF/5B, AF/5C, AF/5D` and bros `86, 87, 82, 81`
plus the park — exactly 11 bytes differ:

    sha1 b60ba24bca183b8c707d01b3f7b835ec7e38614d

That is `content-gen`'s first golden test, and it depends on none of the
unresolved questions. A second roll (`AF/5B, 6E/5C, AF/5D`, bros `81, 82, 86,
87`, same park) gives `bf8e21ff2e4e5958f7f7391f40187534c54bb671`; it exists only
to prove that a rebind works for more than one seed.

Neither slot table is in `manifests/smb3.toml`, on purpose: the loader **refuses**
a challenge carrying `patch_slot` keys (ADR 0015 rejected an inert opt-in path),
so adding them now would fail the whole manifest and take all seven working
challenges with it.

`patch_slot` tables hang off `[[challenge]]`, so a stage played as both a time
trial and a survival run duplicates the entire table and the copies can drift.
Unresolved.

## Five mechanism findings

All confirmed in the disassembly, none of them guessable from a RAM map. They
are what makes the two challenges above work, and they generalise to any SMB3
generator.

1. **The Angry Sun activates only at screen 5 exactly.** `Sun_WaitFarEnough`
   (`PRG/prg005.asm:2718`) gates on `CMP #$05 / BNE` — equality, not "at least" —
   plus `X >= 176`. A sun that spawns past screen 5 never activates at all. This
   is why the extra suns sit at columns `$5B`–`$5D`: they spawn already inside the
   gate and activate on the frame they spawn.
2. **Objects spawn from a look-ahead of +272 pixels** (`LOSBS_LookAhead`,
   `prg005.asm:4918`), and **only one ordinary object spawns per frame** — the
   normal path falls through to `RTS` (`prg005.asm:5155`). Three suns on one
   column would need the scroll to dwell in a single 16-pixel window for three
   frames; on consecutive columns each gets its own boundary. The sand trap entry
   needs ~32 pixels of scroll to spawn all three, which a full P-meter coast
   supplies regardless of input — that is what stops a player defeating a survival
   challenge by standing still.
3. **The object list must stay sorted ascending by column.** The spawn scan stops
   at the first entry on a different screen, and `Level_ObjIdxStartByScreen` is
   built at load by walking the list in order. Any column pool must be bounded by
   its neighbours' values or everything after it silently stops spawning.
4. **There are five object slots** for ordinary enemies (`LDX #$04`,
   `prg005.asm:5110`); the end-level card forces slot 6. If no slot is free the
   object is simply lost — the column alignment has passed and it is not retried.
   Sun Storm uses four of the five.
5. **`Level_ObjectID` at `$0671`–`$0678` is the live actor ID table**, and
   `Objects_State` at `$0661`–`$0668`. This is a far better foundation for "kill
   all enemies" than the stomp counter, at least in a scroll-locked arena. Not
   proposed yet: a slot emptying conflates a kill with an off-screen delete.

## Capturing an entry state for a generated challenge

Entry states are captured on the **base** ROM and rebound to the generated one at
entry (ADR 0019). A patch is only ever visible for objects that have not yet
spawned when the state was taken, so capturing on the base ROM is what makes a
state usable for every seed rather than for one.

`tools/smb3-roll.py` applies one roll from the tables above, so there is
something to check a state against before `content-gen` exists. It is
scaffolding and the crate replaces it.

```sh
# either of the two rolls with a recorded hash, or any seed
python tools/smb3-roll.py --roll golden --out build/smb3-golden.nes
python tools/smb3-roll.py --seed 7 --out build/smb3-seed7.nes
```

```sh
# play to the place the challenge starts, from whatever state is nearest.
# F1 saves to --state's path, so name the file you are authoring and press F1
# at intermediate landmarks too - it is a long way to World 8
harness --game smb3 --stage 1-1 --state assets/states/smb3_w2-sand-trap.state \
        --watch 0x0012,0x00FD,0x0671-0x0675

# check the state against a generated ROM, without touching the file
harness --game smb3 --state assets/states/smb3_w2-sand-trap.state \
        --rom <patched.nes> --rebind-state --frames 600
```

For Sun Storm, capture with `Horz_Scroll_Hi` (`$0012`) `= $04` and `Horz_Scroll`
(`$00FD`) just under `$A0`, running right with a full P-meter — finding 2 above
is why. Watch `$0671`–`$0675` to see the suns take slots as they spawn.

`--rebind-state` never writes to the state file. A state captured *on* a
generated ROM carries that ROM's board and is 52224 bytes rather than 35840; it
is not portable back to the base ROM, so do not ship one.

### What the first capture session actually tests

A state carries RAM, and RAM holds derived copies of ROM data.
`Level_ObjIdxStartByScreen` is built when the level loads (finding 3), so a state
captured *inside* the sand trap carries an index built from the **base** object
list, and the rolled list is then read against it.

It should still work: the table records where each screen starts, the scan runs
forward from there until the screen changes, and the rolled columns `$5B`–`$5D`
all sit on screen 5, which slot A already indexed. That is reasoning, not a
measurement. The measurement is:

```sh
harness --game smb3 --state assets/states/smb3_w2-sand-trap.state \
        --rom build/smb3-golden.nes --rebind-state \
        --frames 600 --watch 0x0671-0x0675
```

Three suns taking actor slots means the stale index is harmless and a mid-level
entry state is fine. One sun means it is not, and the entry state has to be
captured on the world map before the level loads — costing the frames a map
entry takes, which is the price of the patch being read fresh.

Either way, **a generator must never move an object to a screen its slot was not
already indexed under.** That is a constraint on `content-gen`, and it is why
both column pools above are single-valued.
