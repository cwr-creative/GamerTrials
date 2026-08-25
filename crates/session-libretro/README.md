# session-libretro

The libretro adapter, and the developer harness that a manifest is authored
with. One crate for every libretro game — there is no per-game Rust here, and
there must not be one (invariant 5, ADR 0016).

```sh
cargo run --release -p session-libretro --bin harness -- --game dkc --stage jungle-hijinx
cargo run --release -p session-libretro --bin harness -- --help
```

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
| `watch` | the interpreter: markers, rules, edges, attempts |
| `runner` | `LibretroRunner`: `StageRunner` + `Presentable` |
| `input` | keyboard and pad → one libretro joypad bitmask |
| `audio` | resample to the device rate; doubles as the frame clock |
| `ramsearch` | two ways to find an address nobody wrote down |
| `bin/harness` | the window, the keys, the flags |

`manifest_tests.rs` is worth reading before changing any of it: it is every
behavioural test the two old harnesses had, re-run against the manifests this
repository actually ships.

## The harness is not the client

It prints `SessionEvent`s and scores nothing. Deciding what a run was worth
needs a challenge, and that is `challenge-core`'s job. What this binary is for
is the other half of the work: checking that an address is the right address.

- `--list-addrs` — every watched quantity and where its address came from
- `--addr name=0x1234[:u8|u16]` / `--addr name=off` — retarget or silence one,
  without a rebuild
- `--watch 0x0570-0x05a0` — report any address, or a range, on change
- `--dump-memory-map` — what this core is willing to hand over. Worth running
  once per core: what lies outside system RAM decides which detectors are
  possible at all
- `--frames 600` — run headless and print what fired. Same frame numbers as on
  screen, because they are emulated frames (ADR 0005)
- F3-F8/F10 hunt for a value that persists; F11 reports what a one-shot event
  just wrote

## Adding a game

1. Copy a manifest in `manifests/` and change the identity, the ROM and the
   `[runner.libretro]` block.
2. Find the addresses. `--watch`, then F3-F8/F10 for a counter, F11 for a
   one-shot event. Write down where each one came from in its `note`.
3. Write the rules. Start with death and stage completion; everything else is
   decoration until those two are right.
4. `--frames` against the real ROM, then add the cases you care about to
   `manifest_tests.rs`.

If step 3 makes you want arithmetic or a second rule for one stage, stop: that
is usually a sign a better address exists and has not been found yet. It was,
twice, for DKC.
