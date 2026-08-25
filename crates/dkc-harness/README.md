# dkc-harness

Phase 2, first half. Loads a SNES libretro core directly (ADR 0001), runs
Donkey Kong Country in a window with sound and a gamepad, enters a stage from a
savestate, watches RAM for death / damage / stage clear / enemy kills / items,
and prints what it sees.

This is the *second* single-game harness. It is deliberately written as a
sibling of `smb3-harness` rather than on top of it: ADR 0012 says the
`StageRunner` interface gets extracted once two independent implementations
exist to read it off. Merging the two is the next job, not this one.

**Nothing here has been playtested yet.** Almost every address is a lead from a
secondary source. The harness is built to make checking them cheap — see
"Verifying the addresses" below, which is the part that matters right now.

## Prerequisites

1. **Rust toolchain.** `rustup` from <https://rustup.rs>, plus the MSVC linker —
   Visual Studio Build Tools with "Desktop development with C++". Every
   dependency here is pure Rust.
2. **A SNES core**, in `cores/snes/`:
   - `snes9x_libretro.dll` — what this was developed against (1.63)
   - `bsnes_libretro.dll` / `bsnes_mercury_*` — more accurate, heavier
   - `snes9x2010_libretro.dll` — lighter

   From the RetroArch core downloader, or
   <https://buildbot.libretro.com/nightly/windows/x86_64/latest/>.

   The SNES core lives in its own folder rather than in `cores/` alongside the
   NES core, so "the single `*_libretro.dll`" stays unambiguous now that there
   are two harnesses.
3. **The ROM**, at `assets/ROMs/snes/Donkey Kong Country (U) (V1.0) [!].smc`.
   Any revision will run, but the addresses below were reported against the US
   1.0 ROM, so a different revision is one more reason for a value to read
   wrong. `.smc` and `.sfc` are the same bytes under different conventional
   extensions; the core reads either.

## Run

```sh
cargo run --release -p dkc-harness
```

Use `--release`. The debug build compares 128 KB of RAM every frame for the
probe and does not hold 60 fps.

| Key | |
|---|---|
| Arrows | d-pad |
| Z / X | B / A |
| A / S | Y / X |
| Q / W | L / R |
| Enter / RShift | Start / Select |
| F1 / F2 | save / load state |
| F5 | dump every watched address now |
| F9 | reset core, restart frame count |
| Esc | quit |

Plus the RAM discovery keys, which are the subject of their own section below.

## Launching from a savestate

```sh
cargo run --release -p dkc-harness -- --state assets/states/dkc_jungle-hijinxs.state
```

Frame counting restarts at 0 every time a state loads, so the number in
`STAGE COMPLETE at frame N` is frames since stage entry — not since launch.
That is what makes two players comparable (ADR 0005).

The stage label comes from the savestate filename, not from RAM. Stage identity
belongs to the entry point that put you there. What RAM contributes is the level
id read on the first frame after the load, which is the value the watcher then
watches for *leaving*.

## Verifying the addresses

Every default is printed at startup with where it came from, and every one can
be retargeted without a rebuild:

```sh
cargo run --release -p dkc-harness -- --list-addrs
cargo run --release -p dkc-harness -- --addr lives=0x7E08A6 --addr hit_timer=0x08A5:u8
```

`--addr <name>=off` turns a detector off. Addresses may be written as WRAM
offsets (`0x0577`) or full SNES addresses (`0x7E0577`); both mean the same byte.
Values default to 16-bit little-endian, because DKC's game logic runs the 65816
with a 16-bit accumulator — append `:u8` if a value turns out to be one byte
with a noisy neighbour.

| Address | Meaning | Confidence |
|---|---|---|
| `$7E003E` | current level | **confirmed** — RainbowZ Editor transcribes `LDA $3E` from game code; see `levels.rs` |
| `$7E0577` | lives | reported, **could not be confirmed at source** |
| `$7E057B` | bananas | reported, **could not be confirmed at source** |
| `$7E057F` | KONG letters (bitfield) | reported, **could not be confirmed at source** |
| `$7E051B` | fade type; 0x82 = leaving a stage | from `Lui37/dkc1-hack`; playtested — set on both the exit and bonus-room entry, never on a death |
| `$7E0579` | pause flags, bit 6 = paused | same |
| `$7E1929` | accept inputs | same |
| `$7E0B19` | object 1 (DK) X | documented (Data Crystal) |
| `$7E0BC1` | object 1 (DK) Y | documented (Data Crystal) |
| — | Kong count / hit timer / enemy kills / animal buddy | **no address at all** |

A note on sourcing, because it changes how much these are worth. The three
`$057x` values are widely quoted and attributed to TCRF and DKC Atlas, but both
of those pages currently serve unrelated content and DKC Atlas is unreachable,
so nothing could be confirmed at the source. Treat them as leads. The three
`dkc1-hack` values are the strongest here: they come from an assembling
speedrun-practice hack, so they are load-bearing in code that runs, not
transcribed from a wiki.

`$0577`, `$057B` and `$057F` sit four bytes apart, which reads like a stats
block. If one of them is wrong, sweep the neighbourhood first:

```sh
cargo run --release -p dkc-harness -- --watch 0x0570-0x05a0
```

Every watched address prints on change with its frame number, and `--watch`
takes ranges as well as single addresses.

## Finding an address nobody wrote down

`--watch` over a range is how the NES harness hunted for a byte, and it works
there because SMB3 has 2 KB of RAM. DKC has 128 KB, so the same approach prints
tens of thousands of lines a second. Two tools replace it, and they answer
different questions.

**RAM search** (`ramsearch::Search`) is the classic cheat-search, for values
that *persist* — lives, bananas, a Kong count, an enemy-kill counter if one
exists:

| Key | |
|---|---|
| F3 | snapshot / restart the search |
| F4 | keep candidates that did **not** change |
| F6 | keep candidates that changed |
| F7 | keep candidates that increased |
| F8 | keep candidates that decreased |
| F10 | list what is left |

Each filter compares against the previous step, not against the first snapshot.
A worked example — finding the life counter:

1. F3 to snapshot.
2. Die. F8 (decreased).
3. Play a while without dying. F4 (unchanged).
4. Collect 100 bananas, or die again. F8 or F7 as appropriate.
5. F10. Two or three candidates by now. Confirm with
   `--addr lives=$ADDR` and a fresh run.

`--search-width u8` narrows the compare to single bytes if 16-bit is finding
nothing.

**RAM probe** (`ramsearch::Probe`) answers *what changed just now?* — for
one-shot events that cannot be nudged up and down at will: taking a hit, an
item pickup, the moment a stage ends.

| Key | |
|---|---|
| F11 | report bytes that just changed after holding still |

It records, per byte, the frame it last changed and the frame before that, and
reports only bytes that moved inside the last `--probe-recent` frames (default
20) after holding still for at least `--probe-stable` (default 180). The stable
window is what throws out positions, animation timers and the RNG, which are
never quiet. So: take a hit, press F11, and read a list of a dozen instead of
thousands.

Costs about 2.3 MB and a full RAM compare per frame. `--no-probe` turns it off.

Every address the harness prints is in `$7Exxxx` form, ready to paste into
`--addr` or `--watch`.

## Stage completion

Two signals, each doing the job it is good at. Neither works alone.

**`$7E051B` rising to `0x82` is the timing.** It is the fade-to-black speed,
set the moment a transition out of the stage begins. A death sets it never, so
it cannot fire on one, and no grace window is needed. What it cannot tell you
is *what kind* of transition — a bonus barrel raises it exactly as the exit
does.

**`$7E003E`, the level code, is the verdict.** It changes when the next room
loads, shortly after the fade. `levels.rs` maps all 230 of its values to names,
and every way of being inside one level — the start, the checkpoint, its bonus
rooms, a return from one — shares a level *family*. The family changing is the
stage actually ending.

So a clear is: **a fade, then a level code outside the stage's family.** The
frame reported is the fade's, not the level change's, so the hand-off is not
charged to the player (ADR 0005).

```
[stage] in 0x16 Jungle Hijinx (level start) — stage is `Jungle Hijinx`
[event] frame 1442  FADE  00 -> 82  — leaving the stage, waiting for the level code
[stage] level 0x16 Jungle Hijinx (level start) -> 0x06 Jungle Hijinx - Bonus 1
[stage] still inside `Jungle Hijinx` — attempt continues
[event] frame 4780  FADE  00 -> 82  — leaving the stage, waiting for the level code
[stage] level 0x08 Jungle Hijinx (from Bonus 1) -> 0x01 Reptile Rumble (level start)
STAGE COMPLETE at frame 4780
```

This is **route-independent**, which is the whole reason it replaced fade
counting. Whether a player takes both bonus barrels, one, or none, the verdict
and the time are the same. The previous `--complete-on-fade N` scheme silently
assumed every player took the same number of detours, and is gone.

Cases it is built to get right, all covered by tests:

- A bonus room, and a return from one — inside the family, attempt continues.
- A checkpoint re-entry — same.
- The shared animal rooms (`Winky Bonus`, `Rambi Bonus`, `Expresso Bonus`,
  `Enguarde Bonus`, `Animal Token Room`) belong to no level's family, so they
  are recognised as side rooms explicitly. Without that they would read as a
  clear.
- A death and the level reload it causes — no fade, so nothing is armed, and
  the reload is refused as a clear rather than guessed at.
- A fade nothing follows — discarded after `--transition-window` frames
  (default 120) so it cannot mis-time a later, unrelated level change.
- An entry level code outside the table — the stage cannot be delimited, so no
  clear is claimed at all.

Death is independent of all of it: still a decrease in the (unverified) `lives`
counter.

Unwritten ADR 0014 is where this has to be reconciled with something that also
answers for Doom, which has no addressable core RAM.

## What each event currently rests on

| Event | Signal | Status |
|---|---|---|
| Death | lives decreased | live by default, address unverified |
| Stage left | fade type ($7E051B) rises to 0x82 | live, playtested |
| Stage clear | that fade, then $7E003E leaves the stage family | live, route-independent |
| Item | bananas up, a KONG bit set, lives up | live, addresses unverified |
| Damage | Kong count down, or a hit timer rising from 0 | **off — no address** |
| Enemy kill | a kill counter increasing | **off — no address, may not exist** |

Damage and enemy kills print nothing until an address is supplied. Startup says
which detectors are off rather than leaving it to be inferred from silence.

DKC has no score, so it may simply have no enemy-kill counter. If a `Probe`
report on a stomp turns up nothing that holds still, the honest conclusion is
that enemy kills need the sprite table, which is a bigger job than this phase.

## Reaching the bytes

Simpler than the NES side. snes9x publishes all 128 KB of work RAM
(`$7E:0000-$7F:FFFF`) as `RETRO_MEMORY_SYSTEM_RAM`, and every address here lives
in it — verified at startup, which prints the region size and says so if it is
not 131072 bytes.

`SET_MEMORY_MAPS` is still answered and still recorded, but only as a fallback
for addresses outside work RAM. Note that the `select`/`disconnect` bit-matching
form in libretro.h is deliberately not implemented, and on the SNES that is
what describes bank mirroring — so a mirrored address may not resolve through
the map even when the core published it. Nothing here takes that path.

DKC's cartridge SRAM is battery-backed, so unlike SMB3 it does appear as
`RETRO_MEMORY_SAVE_RAM`. Nothing watches it: it holds the save file, not live
stage state.

## Tests

```sh
cargo test -p dkc-harness
```

The watcher and both discovery tools are unit-tested against synthetic WRAM
buffers, so they need no ROM, no core and no savestate. Anything needing a real
ROM stays a manual test.

The tests worth reading first are the stage-boundary ones:
`the_verdict_does_not_depend_on_the_route` (two routes through one stage, same
answer — the property the whole design exists for),
`a_bonus_room_is_not_a_clear_and_the_real_exit_still_is`,
`a_death_and_its_level_reload_are_never_a_clear`, and
`a_shared_animal_room_is_not_a_clear`. They use real level codes, so they
exercise the same table the harness does.

`levels.rs` has its own tests pinning the transcription: spot-checked names at
known codes, and that every Jungle Hijinx entrance and sub-room resolves to one
family.

## Partial progress within a stage

Not implemented. The groundwork is done and written up in
[PROGRESS-HANDOFF.md](PROGRESS-HANDOFF.md): DKC keeps per-level X bounds in a
ROM table indexed by the level code, the lookup is verified against the real
ROM, and player X is at `$7E0B19`. That file has the algorithm, the self-test
numbers, and the caveats — vertical levels, bonus-room spans, non-monotonicity.

## Deliberately absent

No `StageRunner`, no traits, no manifest, no config files. This crate and
`smb3-harness` duplicate their libretro FFI, audio and input layers on purpose;
that duplication is the raw material for the extraction, and deleting it early
would mean guessing at the interface instead of reading it (ADR 0012).

## Notes for future manifest

No savestates exist yet. Once they do, the same shape as the SMB3 notes applies:
one state per stage entry point, each recording whether the stage is viable as a
time trial, an endurance run, or both.

Good news for the manifest: stage completion needs **nothing** from it. The
level-code table is a property of the ROM, not of the stage or the route, so
`levels.rs` answers for every DKC stage without a per-stage constant. That is
invariant 5 holding — adding a stage stays a data change.

What a manifest still has to carry is the savestate per entry point, and
whether each stage is viable as a time trial, an endurance run, or both.
