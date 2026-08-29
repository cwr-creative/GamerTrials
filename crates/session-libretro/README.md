# session-libretro

The libretro adapter, and the developer harness that a manifest is authored
with. One crate for every libretro game — there is no per-game Rust here, and
there must not be one (invariant 5, ADR 0016).

```sh
cargo run --release -p session-libretro --bin harness -- --game dkc  --stage jungle-hijinx
cargo run --release -p session-libretro --bin harness -- --game doom --stage e1m1-hangar
cargo run --release -p session-libretro --bin harness -- --help
```

Three games, three manifests, no per-game code — including Doom, which runs on
an emulated PC under dosbox-pure rather than on a console (ADR 0018).

Games live in `manifests/`. What goes in one is
[`docs/manifest-schema.md`](../../docs/manifest-schema.md); the per-game notes
that do not belong in a manifest — address provenance, what is playtested — are
in [`docs/games/`](../../docs/games).

## What is where

| Module | |
|---|---|
| `libretro` | raw FFI plus a `Core` that loads a core DLL directly (ADR 0001) |
| `memory` | an address in a manifest → a byte, per the console's routing rules |
| `manifest` | TOML in, validated `Manifest` out. The authority on what parses |
| `prep` | manifest → bytes: the player's dump, a hack applied to it, a seeded roll |
| `watch` | the interpreter: markers, rules, edges, attempts |
| `runner` | `LibretroRunner`: `StageRunner` + `Presentable` |
| `input` | host keyboard and pad → a joypad bitmask, or keys passed through |
| `audio` | resample to the device rate; doubles as the frame clock |
| `ramsearch` | two ways to find an address nobody wrote down |
| `bin/harness` | the window, the keys, the flags |

`manifest_tests.rs` is worth reading before changing any of it: it is every
behavioural test the two old harnesses had, re-run against the manifests this
repository actually ships.

## The harness is not the client

It prints `SessionEvent`s and scores nothing — not even against a challenge's
own `complete_when`, which `--challenge` prints and then ignores. Comparing
facts to criteria is `challenge-core`'s job, and an adapter that decided a run
was won would be the mistake ADR 0017 exists to prevent. What this binary is for
is the other half of the work: checking that an address is the right address.

- `--list-addrs` — every watched quantity and where its address came from
- `--list-challenges` — every challenge the game offers and what completes each
- `--challenge <ID>` — start at that challenge's stage, with its criteria printed
- `--addr name=0x1234[:u8|u16]` / `--addr name=off` — retarget or silence one,
  without a rebuild
- `--watch 0x0570-0x05a0` — report any address, or a range, on change
- `--dump-memory-map` — what this core is willing to hand over. Worth running
  once per core: what lies outside system RAM decides which detectors are
  possible at all, and some cores hand over no system RAM whatsoever
- `--dump-options` — every core option this core publishes, with its legal
  values and default, plus every environment call it asked for and did not get.
  Also worth running once per core: an option left at its default is a machine
  configured differently from the next player's
- `--seed 3` — roll a generated challenge's patch slots and run the ROM that
  comes out, printing every byte it wrote. Without it, a generated challenge
  runs unrolled, which is what capturing its entry state wants
- `--rom base=<PATH>` — run one of the manifest's `[[rom]]` entries from a file.
  The id is required: a manifest may declare a base ROM and hacks of it
- `--frames 600` — run headless and print what fired. Same frame numbers as on
  screen, because they are emulated frames (ADR 0005)
- `--capture <PATH>` — write a savestate where a `--frames` run ended. With a
  `named` entry point that is the whole authoring loop in one command
- F3-F8/F10 hunt for a value that persists; F11 reports what a one-shot event
  just wrote

## Adding a game

0. If the core is one this repository has not run before, `--dump-memory-map`
   and `--dump-options` first. They answer the two questions everything else
   depends on: is there a route to a byte, and is the machine configured the
   same way twice.
1. Copy a manifest in `manifests/` and change the identity, the `[[rom]]` and
   the `[runner.libretro]` block.
2. Find the addresses. `--watch`, then F3-F8/F10 for a counter, F11 for a
   one-shot event. Write down where each one came from in its `note`.
3. Write the rules. Start with death and stage completion; everything else is
   decoration until those two are right.
4. Write the stages, then the challenges. Every challenge a player can be dealt
   is one `[[challenge]]`: nothing is inferred, so a level playable both ways is
   two entries (ADR 0017). `--list-challenges` reads them back.
5. `--frames` against the real ROM, then add the cases you care about to
   `manifest_tests.rs`.

If step 3 makes you want arithmetic or a second rule for one stage, stop: that
is usually a sign a better address exists and has not been found yet. It was,
twice, for DKC. Step 4 is where a per-stage difference legitimately goes — two
challenges on one stage is the normal case.

## Adding a hack, or a generated challenge

Also data, and also no Rust (ADR 0020, ADR 0015).

A **hack** is a `[[rom]]` with a `from` and a `patch`, plus the stages that name
it. It shares the manifest only if the telemetry holds against it unchanged; a
hack that moves an address is a different game and gets its own file. Its entry
states are captured on it, not on the base — `--stage <its stage>` derives it and
runs it, and F1 captures as usual.

A **generated challenge** is an ordinary `[[challenge]]` plus a `generator_version`
and a `patch_slot` table: an offset, a width, and the values that are legal there.
`content-gen` rolls it. Find the offsets in a disassembly, find the legal values in
the same place, and playtest them — the engine guarantees it wrote one of the
values you declared, and nothing more.

`docs/games/smb3.md` is the worked example for both, including how the object
list of a hack was located without source for it.

## Adding a game that has no savestate to start from

A stage entry is normally a captured state, and the state has to come from
somewhere. That is what `entry = { named = ... }` is for: a place the game can
reach by itself, which the runner reaches by reloading the content and running
`boot_seconds` worth of frames.

```sh
# boot to where the game starts itself, run on a bit, capture
harness --game doom --stage e1m1-hangar-boot --frames 120 \
        --capture assets/states/doom_e1m1.state

# or interactively: enter at the `named` stage, play to the right spot, press F1
harness --game doom --stage e1m1-hangar-boot
```

Declare both stages in the manifest — one `named` to author from, one `state`
to race on. They are a pipeline, not alternatives (ADR 0002). It matters
because the two cost very different amounts: Doom restores a state in ~60
frames and cold-boots in ~960, and a death in a time trial pays that difference
every time.
