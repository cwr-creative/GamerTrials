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

## Stages

Four entry points, all in world 1, all cleared by the standard `stage_cleared`
signal.

| Stage | Challenges |
|---|---|
| `1-1` | time trial, endurance |
| `1-2` | time trial, endurance |
| `1-3` | time trial, endurance |
| `1-4` | endurance only — forced-scrolling, so everyone finishes in the same number of frames |

That constraint is in the manifest now (`challenges = ["endurance"]`), not in
this document's prose.
