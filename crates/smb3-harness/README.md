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

## Reaching 1-4

There is no savestate in the repo — one has to be made from your own ROM.

```sh
# play to the start of 1-4, then press F1
cargo run --release -p smb3-harness -- --state assets/states/smb3_1-4.state

# from then on, that same command boots straight into 1-4 at frame 0
```

Frame counting restarts at 0 every time a state loads, so the number in
`STAGE COMPLETE at frame N` is frames since stage entry — not since launch.
That is what makes two players comparable (ADR 0005).

The stage label ("1-4") comes from the savestate filename, not from RAM. Stage
identity belongs to the entry point that put you there; RAM only confirms world
and lives.

## Verifying the RAM addresses

This is the part most likely to be wrong, and the part worth checking first.

| Address | Meaning | Confidence |
|---|---|---|
| `$0736` | Mario lives | documented |
| `$0727` | world − 1 | documented |
| `$0014` | "flag to return to map" | documented as a flag; **exact semantics unverified** |
| `$0078` / `$0079` | map position, used as the level id | documented as position |

SMB3 has no documented "current level number" — a level is identified by the map
tile you entered from, so the map position pair *is* the level id. If that reads
wrong in practice, `--watch` is how to find the right byte:

```sh
cargo run --release -p smb3-harness -- --watch 0x0014,0x0727,0x0736,0x0078,0x0079
```

Every watched address prints on change with its frame number. All six constants
live at the top of `src/smb3.rs`.

## Stage completion

`$0014` rising from zero means the game is leaving the level — which covers both
finishing it and dying. The two are separated by whether the life counter
dropped, so a death prints `STAGE FAILED` instead. A rise only counts if the
flag has been clear for 120 frames, otherwise map-screen activity reads as a
stage end.

That heuristic is exactly what unwritten ADR 0014 has to replace with something
that also answers for Doom, which has no addressable core RAM. Do not build
scoring on it yet.

## Deliberately absent

No `StageRunner`, no traits, no config files. Phase 1 is one game and stays ugly
until DKC gives Phase 2 something real to abstract against (ADR 0012).
