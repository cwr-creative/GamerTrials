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
| `$7E003E` | current level | documented (Data Crystal), not playtested |
| `$7E0577` | lives | reported, **could not be confirmed at source** |
| `$7E057B` | bananas | reported, **could not be confirmed at source** |
| `$7E057F` | KONG letters (bitfield) | reported, **could not be confirmed at source** |
| `$7E051B` | fade type | from `Lui37/dkc1-hack` — load-bearing in working code |
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

## Stage completion, and why it is not trustworthy yet

The only end-of-stage signal available is the level id leaving the value it had
at stage entry. That has three known problems, in increasing order of how much
they matter.

**It is late.** The level id moves when the level *unloads*, not when the exit
target is touched, so the reported time includes several seconds of hand-off.
This is exactly the gap the NES harness had before the card counter was found,
and the fix is the same: find the earlier signal. Do not build scoring on this.

**A bonus barrel looks like the exit.** Entering a bonus room changes the level
id too, so it currently reads as a clear. Coming back out reopens the attempt
and prints a warning saying the previous result may have been a false positive,
but it cannot retract what it already reported.

**Death ordering is unknown.** A death and the unload it causes are not
simultaneous, and with the life counter unverified it is not known which lands
first. If lives drops first, the death wins outright. If the level id moves
first, an immediate verdict would score a death as a clear — the failure mode
ADR 0003 calls the worst one available. So a clear is *held* for
`--result-grace` frames (default 120) and cancelled by a death arriving inside
that window. The verdict prints late; the frame it reports is still the frame
the stage ended, so nothing is lost from the measurement (ADR 0005).

If no `lives` address is configured at all, an unexplained level change is
reported as `STAGE FAILED`, not as a clear — with no death detector running,
"no death was seen" proves nothing. Same conservative choice the NES harness
makes.

Unwritten ADR 0014 is where this has to be replaced with something that also
answers for Doom, which has no addressable core RAM.

## What each event currently rests on

| Event | Signal | Status |
|---|---|---|
| Death | lives decreased | live by default, address unverified |
| Stage clear | level id left its entry value | live, late and bonus-room-confused |
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

The tests worth reading first are the stage-boundary ones: a death before the
unload, a death just after it, a bonus-room round trip, and a state load
landing mid-grace. Those are the four orderings the verdict logic exists to get
right.

## Deliberately absent

No `StageRunner`, no traits, no manifest, no config files. This crate and
`smb3-harness` duplicate their libretro FFI, audio and input layers on purpose;
that duplication is the raw material for the extraction, and deleting it early
would mean guessing at the interface instead of reading it (ADR 0012).

## Notes for future manifest

No savestates exist yet. Once they do, the same shape as the SMB3 notes applies:
one state per stage entry point, each recording whether the stage is viable as a
time trial, an endurance run, or both.

A DKC-specific wrinkle to record when those states are made: levels with a bonus
barrel are the ones where the current stage-clear signal misfires, so either
they need the earlier signal found first, or they are endurance-only until it
is.
