# smb3-harness

Phase 1. Loads a NES libretro core directly (ADR 0001), runs SMB3 in a window
with sound and a gamepad, watches RAM, enters a stage from a savestate, and
prints when the stage ends.

## Prerequisites

1. **Rust toolchain.** `rustup` from <https://rustup.rs>, plus the MSVC linker —
   Visual Studio Build Tools with "Desktop development with C++". Every
   dependency here is pure Rust, so nothing else needs a C compiler.
2. **A NES core.** Any libretro NES core DLL, dropped in `cores/`:
   - `mesen_libretro.dll` — most accurate, best RAM/savestate behaviour
   - `fceumm_libretro.dll` — lighter, very widely used
   - `nestopia_libretro.dll`

   Get them from the RetroArch core downloader, or from
   <https://buildbot.libretro.com/nightly/windows/x86_64/latest/>.
   The harness picks the core up automatically if `cores/` holds exactly one.

## Run

```sh
cargo run --release -p smb3-harness
```

The ROM defaults to `assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes`.

| Key | |
|---|---|
| Arrows | d-pad |
| Z / X | B / A |
| Enter / RShift | Start / Select |
| F1 / F2 | save / load state |
| F5 | dump watched RAM now |
| F9 | reset core, restart frame count |
| Esc | quit |

## Launching from savestate

```sh
# to launch from a specific state
cargo run --release -p smb3-harness -- --state assets/states/smb3_1-4.state

# from then on, that same command boots straight into 1-4 at frame 0
```

Frame counting restarts at 0 every time a state loads, so the number in
`STAGE COMPLETE at frame N` is frames since stage entry — not since launch.
That is what makes two players comparable (ADR 0005).

The stage label ("1-4") comes from the savestate filename, not from RAM. Stage
identity belongs to the entry point that put you there; RAM only confirms world
and lives.

## RAM addresses

This is the part most likely to be wrong, and the part worth checking first.

| Address | Meaning | Confidence |
|---|---|---|
| `$0736` | Mario lives | verified |
| `$0727` | world − 1 | verified |
| `$0014` | "flag to return to map" | verified |
| `$0078` / `$0079` | map position, used as the level id | verified |
| `$0578` | powerup/form (00-06: small, super, fire, raccoon, frog, tanooki, hammer) | documented (Data Crystal), not yet playtested |
| `$0552` | hit-invulnerability timer; rising from 0 means Mario just took damage | documented (Data Crystal), not yet playtested |
| `$0510` | pipe transit countdown; rising from 0 means entering/exiting a pipe | documented (Data Crystal), not yet playtested |
| `$05F4` | stomp counter; increments on a stomp kill only — fireball/shell/star kills don't touch it | documented (Data Crystal), not yet playtested |
| `$7D9C` | card count; increments when Mario is awarded a card at a level's end | verified (playtested against this harness) |

SMB3 has no documented "current level number" — a level is identified by the map
tile you entered from, so the map position pair *is* the level id. If any of the
above reads wrong in practice, `--watch` is how to check it:

```sh
cargo run --release -p smb3-harness -- --watch 0x0014,0x0727,0x0736,0x0078,0x0079,0x0578,0x0552,0x0510,0x05F4,0x7D9C
```

Every watched address prints on change with its frame number. All eleven
constants live at the top of `src/smb3.rs`. `--watch` takes any CPU address,
including `$6000-$7FFF` — see "Reaching cartridge WRAM" below. An entry can
also be a range, `lo-hi`, to sweep for a byte with no known address yet
(e.g. an enemy-death signal that isn't stomp-specific):

```sh
cargo run --release -p smb3-harness -- --watch 0x0090-0x00ff
```

An SMB3 RAM map containing other addresses to test can be found at https://datacrystal.tcrf.net/wiki/Super_Mario_Bros._3/RAM_map

## Reaching cartridge WRAM

The card count lives at `$7D9C`, in the `$6000-$7FFF` cartridge WRAM — and
`retro_get_memory_data` cannot reach it. Mesen exposes exactly one region,
`RETRO_MEMORY_SYSTEM_RAM` (2048 bytes, the internal RAM at `$0000-$07FF`);
querying `RETRO_MEMORY_SAVE_RAM` and every other region id returns nothing,
because libretro only classifies WRAM as "save RAM" when it's battery-backed
and SMB3's isn't.

The route that works is `RETRO_ENVIRONMENT_SET_MEMORY_MAPS` (`36 | 0x10000`),
which the harness now answers. Mesen publishes 64 descriptors covering
`$0000-$1FFF` (internal RAM and its mirrors) and `$6000-$7FFF` (cartridge
WRAM) as flat 256-byte pages; `$2000-$5FFF` is PPU/APU/IO registers and is
correctly absent. `libretro::read_addr` resolves a CPU address against those,
and `smb3::read_byte` prefers it, falling back to the system-RAM slice so a
core that publishes no map still resolves `$0000-$07FF`.

If a core publishes no map, startup says so on stderr and card detection is
silently unavailable — stage completion falls back to the `$0014` heuristic
alone.

Only descriptors with `select == 0` are resolved. The `select`/`disconnect`
bit-matching form in libretro.h is deliberately not implemented: no NES core
needs it here, and a wrong guess would return the wrong byte rather than
nothing.

## Stage completion

A card increase (`$7D9C`) is the primary success signal: it fires seconds
before `$0014` moves, and it never fires on death, so no lives check is needed
to disambiguate it. Measured against `assets/states/quick.state`, the card
lands at frame 11 and `$0014` doesn't rise until frame 359 — a 348-frame
(5.8 s) difference, all of it noise that used to be counted as stage time.

Not every level awards a card — fortresses, castles, and airships don't run
the card-match bonus — so this only covers the levels that do.

For everything else, `$0014` rising from zero means the game is leaving the
level. If a card already fired for this attempt, that rise is ignored (it's
just the level's ordinary exit). Otherwise it's read as `STAGE FAILED` —
which means a card-less level cleared normally is currently indistinguishable
from a death. `STAGE FAILED` in the console output says as much.

That "already fired" suppression is scoped to **one attempt**, and is cleared
on the falling edge of `$0014` — the point where the hand-off to the map has
finished. Scoping it any wider is a bug worth guarding: when it latched for
the whole process, a single card early in a run silenced every later death.
`a_death_after_a_card_still_fails` and `two_deaths_both_fail` in `src/smb3.rs`
pin that down.

That gap — and unifying this with something that also answers for Doom, which
has no addressable core RAM — is exactly what unwritten ADR 0014 has to
replace this heuristic with. Do not build scoring on it yet.

## Tests

```sh
cargo test -p smb3-harness
```

The watcher's stage logic is unit-tested against synthetic RAM buffers, so
these need no ROM, no core, and no savestate (`cargo test` publishes no memory
map, so `read_byte` falls back to the slice and the tests can write `$7D9C`
directly). Anything needing a real ROM stays a manual test.

## Deliberately absent

No `StageRunner`, no traits, no config files. Phase 1 is one game and stays ugly
until DKC gives Phase 2 something real to abstract against (ADR 0012).

## Notes for future manifest

There are currently 4 save states for SMB3 for the first four levels of world 1. The win condition for each is the `STAGE COMPLETE` success signal.

smb3_1-1.state
Viable as a survival challenge and a time trial challenge.

smb3_1-2.state
Viable as a survival challenge and a time trial challenge.

smb3_1-3.state
Viable as a survival challenge and a time trial challenge.

smb3_1-4.state
Only viable as a survival challenge (forced-scrolling). 