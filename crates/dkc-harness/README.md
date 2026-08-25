# dkc-harness

Phase 2, first half. Loads a SNES libretro core directly (ADR 0001), runs
Donkey Kong Country in a window with sound and a gamepad, enters a stage from a
savestate, watches RAM for death / damage / stage clear / enemy kills / items,
and prints what it sees.

This is the *second* single-game harness. It is deliberately written as a
sibling of `smb3-harness` rather than on top of it: ADR 0012 says the
`StageRunner` interface gets extracted once two independent implementations
exist to read it off. Merging the two is the next job, not this one.

**Barely any of this has been playtested.** What has changed is the *sourcing*:
almost every address now has a second, independent origin in the
RetroAchievements code notes for DKC, transcribed in
[RA-CODE-NOTES.md](RA-CODE-NOTES.md). Notes written by achievement authors
against running code are much better evidence than the wikis the first pass
relied on, several of which could not be reached at all. They are still not a
playtest — see "Verifying the addresses" below.

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

(`--dump-memory-map` is the companion to these: it says which regions exist to
point an address *at*. See "Reaching the bytes".)

`--addr <name>=off` turns a detector off. Addresses may be written as WRAM
offsets (`0x0577`) or full SNES addresses (`0x7E0577`); both mean the same byte.
Values default to 16-bit little-endian, because DKC's game logic runs the 65816
with a 16-bit accumulator — append `:u8` if a value turns out to be one byte
with a noisy neighbour.

| Address | Meaning | Source |
|---|---|---|
| `$7E0527` | in a stage (0) or on the overworld (1) | RA notes — the spine of stage completion |
| `$7E003E` | room id, last exit used | **two sources**: RainbowZ transcribes `LDA $3E`; RA agrees |
| `$7E0040` | last room id inside a level, else 0 | RA notes — a second in-a-stage indicator |
| `$7E0575` | lives | RA notes. `$0577`, which every older map quotes, is the *display copy* |
| `$7E057B` | bananas / mini animal tokens | RA notes, agreeing with the older map |
| `$7E057F` | KONG letters (bitfield, u8) | RA notes |
| `$7E051B` | fade type; 0x82 = fading out | **two sources**: `Lui37/dkc1-hack` and RA; playtested |
| `$7E0579` | state flags: b0 = 2 Kongs, b6 = paused | **two sources**; RA supplies the bit meanings |
| `$7E16D1` | DK flash timer, set on taking a hit | RA notes (Diddy `$16D3`; invuln `$16D5`/`$16D7`) |
| `$7E0516` | riding buddy; 9 Rambi, A Expresso, B Winky, C Enguarde | RA notes |
| `$7E1E17` | enemies stomped in a row | RA notes — a streak, not a running total |
| `$7E005E` / `$7E0060` | active character's X / Y | RA notes |
| `$7E0FC3` / `$7E0FF7` | DK's X / Y (Diddy `$0FC5` / `$0FF9`) | RA notes |
| `$7E1929` | accept inputs | `dkc1-hack` only — the one address here with no RA note |
| — | a dedicated Kong counter | still none; damage reads `$0579` bit 0 instead |

A note on sourcing. The first pass rested the `$057x` block on maps attributed
to TCRF and DKC Atlas, neither of which could be reached. The RA code notes
corroborate all three, and correct one: `$0577` is `Lives (Display)`, the copy
the HUD draws from, while `$0575` is the counter itself. The harness watches
`$0575` now, because that is the one that moves on the frame of the death.

Two widths also changed. The notes mark 16-bit values explicitly (`[2 Byte]`,
`[16-Bits]`), and that convention agrees with this harness everywhere the two
already agreed — so an unmarked note means a byte. By it, `kong_letters` and
`state_flags` are `u8`, not the `u16` they were. Pause was unaffected (bit 6 is
in the low byte), but a 16-bit `kong_letters` read was dragging in `$0580`.

`$0B19` / `$0BC1`, the old player-position pair, have **no** RA note at all.
They are still reachable with `--addr`, but the defaults are now the attested
`$005E` / `$0060`, and `$0FC3` / `$0FF7` are carried alongside as `dk_x` /
`dk_y` so a playtest can see both at once and settle which one progress should
be measured from. Note that [PROGRESS-HANDOFF.md](PROGRESS-HANDOFF.md) is
written against `$0B19`, which makes it the least trustworthy document here.

If a value still reads wrong, sweep its neighbourhood:

```sh
cargo run --release -p dkc-harness -- --watch 0x0570-0x05a0
```

Every watched address prints on change with its frame number, and `--watch`
takes ranges as well as single addresses.

## Finding an address nobody wrote down

**Look in [RA-CODE-NOTES.md](RA-CODE-NOTES.md) first.** 134 addresses for DKC,
already in the offset form `--addr` takes. Most of what this section was
written to find turned out to be in there.

For what is not: `--watch` over a range is how the NES harness hunted for a
byte, and it works there because SMB3 has 2 KB of RAM. DKC has 128 KB, so the
same approach prints tens of thousands of lines a second. Two tools replace it,
and they answer different questions.

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

**`$7E0527` is the whole rule.** RA's code notes call it `00=In a Stage
01=In Overworld`, and that is exactly the question the harness was trying to
answer the hard way. Every way of *staying* in a level leaves it at `00`: a
bonus room, the return from one, a checkpoint, a death and the reload it
causes. Only actually leaving flips it.

`$7E051B` is kept, but demoted to timing. It rises a little before the map is
up, and ADR 0005 says the hand-off is not the player's time — so a clear is
reported at the frame of the fade, not the frame the overworld finishes
loading. A fade that leads nowhere out of the stage is discarded after
`--transition-window` frames (default 120), and an exit with no fresh fade
behind it is timed to itself rather than refused.

One case needs more than that. Quitting from the pause menu reaches the
overworld exactly as the exit does, so the harness records whether `$7E0579`
bit 6 was set *at the moment the fade began* — by the time the map is up the
menu is long gone — and scores a paused fade as `StageFailed { Quit }`.

```
[ram] frame 1  in-stage  level 8  room 8  lives 6  bananas 59  KONG -ON-  2 kongs  fade 00  at (5376,503)
[stage] entered at 0x08 Jungle Hijinx (from Bonus 1) — in `Jungle Hijinx`
[event] frame 1442  FADE  00 -> 82
[stage] level 0x08 Jungle Hijinx (from Bonus 1) -> 0x06 Jungle Hijinx - Bonus 1
[event] frame 4780  FADE  00 -> 82
[stage] left the stage: in_stage 00 -> 01
STAGE COMPLETE at frame 4780
```

### What this replaced, and why

The previous rule paired the fade with the level code at `$7E003E` and asked
whether the new code belonged to the same level *family* as the entry code. It
needed all 230 level names, a derived family for each, an explicit list of the
shared animal rooms that belong to no family, and a per-attempt memory of which
family the stage was — all of it reconstructing from the outside a fact the
game already stores in one byte. It also inherited a failure mode: a stage
whose entry code fell outside the table could not be delimited at all, so no
clear was claimed for it.

Route-independence was the property that scheme existed to buy, and the new
rule has it for free rather than by construction — bonus rooms are not special
cases any more, they are simply not exits. `levels.rs` keeps its table for
naming and grouping in the log; `is_side_room` is gone, because the hole it
patched no longer exists.

Cases still covered by tests:

- A bonus room and the return from one — still inside the stage.
- A checkpoint re-entry — same.
- The shared animal rooms — same, and needing no special case to be so.
- A death and its level reload — no exit, so the attempt is failed, never
  cleared.
- A pause-menu exit — reaches the map, but scores as a quit.
- A stale fade — discarded, and it must not block a fresh fade arriving on the
  same frame (this ordering was a real bug the tests caught).
- Reaching the map twice — reported once.
- No `in_stage` address — no completion detector at all, rather than a guess.

Death is independent of all of it: a decrease in `lives` at `$7E0575`.

Unwritten ADR 0014 is where this has to be reconciled with something that also
answers for Doom, which has no addressable core RAM.

## What each event currently rests on

| Event | Signal | Status |
|---|---|---|
| Death | `$7E0575` lives decreased | live, address corroborated |
| Stage left | `$7E0527` leaves 0x00 | live, **not yet playtested** |
| Stage clear | that, timed to the preceding `$7E051B` fade | live, route-independent by construction |
| Stage quit | that, but the fade began while paused | live, **not yet playtested** |
| Damage | `$7E0579` bit 0 falls, or `$7E16D1` rises from 0 | live — was off for want of an address |
| Enemy kill | `$7E1E17` stomp streak rises | live — a streak, so it undercounts |
| Item | bananas up, a KONG bit set, lives up, buddy changed | live, addresses corroborated |

The four detectors that used to print nothing all have addresses now. What none
of them have is a playtest, and `$7E1E17` is known to be the wrong shape: it
counts a stomp *streak* and resets when the chain breaks, so it sees each stomp
happen but cannot total a stage. The better lead, if a real count is wanted, is
the per-object table RA documents at `$7E102F`-`$7E1049` — one flag per slot,
`01 = Object _N Defeated`, parallel to the object ids at `$7E0D4B`-`$7E0D61`.
That is the sprite table this harness was told it could not have, sitting in
work RAM the whole time.

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

### What a core actually publishes

```sh
cargo run --release -p dkc-harness -- --dump-memory-map
```

Lists both routes — all four `retro_get_memory_data` ids and every
`SET_MEMORY_MAPS` descriptor, with flags, `addrspace` name, and whether
`read_addr` can resolve it. Worth running once per core, because what lies
outside work RAM decides which detectors are possible at all: OAM would make
enemy kills and damage a sprite-table read instead of a hunt for a counter that
may not exist, and APU RAM would make "which music is playing" a plain watch.

For snes9x 1.63, the answer is narrow:

| Region | |
|---|---|
| `SAVE_RAM` | 2 KB — cartridge SRAM, the save file |
| `RTC` | not published |
| `SYSTEM_RAM` | 128 KB — work RAM, what everything here watches |
| `VIDEO_RAM` | 64 KB — VRAM |
| `SET_MEMORY_MAPS` | **no descriptors at all** |

So snes9x offers exactly one thing beyond work RAM, and it is VRAM: tiles and
tilemaps. No OAM, no CGRAM, no APU RAM, and no descriptor array — the map
fallback above is dead code against this core, though it stays for the next one.

VRAM is not nothing. The status bar is drawn from a tilemap, so a HUD value can
be read as the tile indices behind it rather than as the RAM byte the game kept
it in — which is a way to check the unverified `$057x` block against what the
player is actually shown. It does not answer for sprites.

Whether a more accurate core publishes more is untested; bsnes is the one to
try, and it is not in `cores/snes/`.

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
`a_death_and_its_level_reload_are_never_a_clear`,
`leaving_from_the_pause_menu_is_a_quit_not_a_clear`, and
`a_fade_that_led_nowhere_is_discarded_after_the_window` — that last one caught
a real ordering bug, where a stale fade blocked a fresh one arriving on the
same frame. They walk real level codes even though the codes no longer decide
anything, so the routes are the ones a player would take.

`levels.rs` has its own tests pinning the transcription: spot-checked names at
known codes, and that every Jungle Hijinx entrance and sub-room resolves to one
family.

## Partial progress within a stage

Not implemented. The groundwork is written up in
[PROGRESS-HANDOFF.md](PROGRESS-HANDOFF.md): DKC keeps per-level X bounds in a
ROM table indexed by the level code, the lookup is verified against the real
ROM, and player X is at `$7E0B19`.

**Read that file with the address caveat in mind.** `$7E0B19` has no RA code
note, and the attested position addresses are `$7E005E` (active character) and
`$7E0FC3` (DK specifically). The ROM-table half of that document is unaffected —
it is a property of the ROM — but the RAM half needs re-checking against
`player_x` and `dk_x`, which the harness now carries side by side for exactly
this reason.

## Deliberately absent

No `StageRunner`, no traits, no manifest, no config files. This crate and
`smb3-harness` duplicate their libretro FFI, audio and input layers on purpose;
that duplication is the raw material for the extraction, and deleting it early
would mean guessing at the interface instead of reading it (ADR 0012).

## Notes for future manifest

No savestates exist yet. Once they do, the same shape as the SMB3 notes applies:
one state per stage entry point, each recording whether the stage is viable as a
time trial, an endurance run, or both.

Stage completion still needs **nothing** from the manifest, and needs less than
it did: `$7E0527` is one address for every stage in the game, so not even the
level table is load-bearing any more. Invariant 5 holds — adding a stage stays
a data change.
