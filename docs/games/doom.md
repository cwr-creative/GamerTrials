# DOOM (shareware, DOS)

The manifest is [`manifests/doom.toml`](../../manifests/doom.toml). This document
is the part that does not belong in it: where the addresses came from, what is
playtested and what is only documented, and what is different about a DOS game.

The first game in the library whose machine is a PC rather than a console. ADR
0018 is why it is here at all and why it arrives through `session-libretro`
under `dosbox-pure` rather than through an adapter of its own; ADR 0014 is the
RAM-watching decision it shares with the two consoles.

**The evidence here is better than DKC's and worse than a full playtest.** Every
address was measured on a running machine rather than transcribed from a wiki —
several by typing a cheat and diffing 16 MB, which is a controlled write and
therefore strong evidence. What has *not* happened is a long session of ordinary
play with the watcher attached looking for false positives.

## The content bundle

Doom shareware v1.9, the freely redistributable `doom19s.zip` from the idgames
archive. Its `DEICE`/`DOOMS_19.*` installer is a split PKZIP self-extractor, so
no DOS-side install is needed: concatenating the parts gives an ordinary zip.

The bundle the manifest names is those two files plus a `dosbox.conf`, rezipped:

| Member | SHA-1 |
|---|---|
| `DOOM.EXE` | `aa0668fae2f743ee5e3e5634ee42d3c8ace7d907` |
| `DOOM1.WAD` | `5b2e249b9c5133ec987b3ea77596381dc0d6bc1d` |
| `dosbox.conf` | `[autoexec]` running `C:` then `DOOM.EXE -warp 1 1` |

`dosbox-pure` mounts the zip as `C:` and runs the `[autoexec]`, so the bundle is
**one file with one hash** — the same shape as "one manifest per base ROM".

`[rom].sha1` is left unset on purpose. The archive's own hash depends on how it
was zipped, so `asset-store` will want to hash the members rather than the
container (ADR 0018).

## What is different about a DOS game

Four things, all in `[runner.libretro]`, and nothing below that line:

1. **No system-RAM slice.** `retro_get_memory_data` answers nothing for every
   id, `SYSTEM_RAM` included. `dosbox-pure` publishes three `SET_MEMORY_MAPS`
   descriptors instead — conventional memory, the HMA, and 15.4 MiB of extended
   memory at `0x200000` where a DOS/4GW program lives. So `prefer =
   "memory_map"` is the route that works, which is the route SMB3 already uses.
2. **A keyboard, not a pad.** `controller = "keyboard"`, and every host key
   passes through unchanged. Doom maps its own keys.
3. **Pinned core options.** See below. This is the load-bearing one.
4. **Four-byte fields.** Doom's globals are C `int`s, so `default_width =
   "u32"`.

## The cycles setting is part of the score

`dosbox_pure_cycles` defaults to `auto`, which is *defined* as tracking host
performance.

Addresses do not move with it — measured across 13400, 26800 and 77000, and
across memory sizes of 8/16/32 MB and both CPU cores. **Frame counts do.** Two
independent processes running the same attract demo produced byte-identical
event logs at a fixed cycles value, and byte-identical logs at `auto` — but the
same demo damage lands on frame 501 at 26800 and frame 452 on `auto`.

So emulated-frame counting is sound (invariant 4 holds), and a frame count is
only comparable between players on the same cycles value. That makes the setting
part of the game's identity rather than a preference, which is why
`[runner.libretro.options]` exists at all.

`auto` agreeing with itself proves less than it looks: both runs were on one
machine under matched load. It is repeatable here and not portable, which is the
worst combination — it would pass a single-machine test and diverge in a race.

## The addresses

All `u32`, little-endian, in the emulated physical space the descriptors
describe. Confirmed byte-identical across three cold boots.

| Field | Address | Meaning | Evidence |
|---|---|---|---|
| `health` | `0x00389E98` | `players[0].health` | neighbour of the `IDKFA` hit; `100` at spawn |
| `armorpoints` | `0x00389E9C` | `players[0].armorpoints` | **`IDKFA` diff: the only 0→200 in 16 MB besides one coincidence** |
| `armortype` | `0x00389EA0` | `players[0].armortype` | `2` after `IDKFA` (blue armour) |
| `playerstate` | `0x00389E7C` | 0 live, 1 dead, 2 reborn | struct offset, then confirmed live |
| `gamemap` | `0x0038A3E8` | current map number | **`IDCLEV12` diff: the only 1→2 in 16 MB** |
| `gameepisode` | `0x0038A3F8` | current episode | `1` throughout episode 1 |
| `gamestate` | `0x0038A3F4` | 0 level, **1 intermission**, 3 demoscreen | title↔demo diff, then confirmed by finishing E1M1 |

### Why this is a `player_t` and not a coincidence

The `IDKFA` hit is one address. What makes it a struct is the neighbourhood:

```
0x00389E88  2686976   viewz / viewheight — 41<<16, Doom's exact VIEWHEIGHT
0x00389E98      100   health
0x00389E9C      200   armorpoints   <- the cheat's write
0x00389EA0        2   armortype
0x00389EA4        0   powers[0..]
```

`41<<16` is the corroboration that matters: a constant from Doom's source
appearing at exactly the offset the struct layout predicts.

### Why `gamestate == 1` is the stage clear

This was the last thing that was inference rather than measurement, and it is
now measured. A human finished E1M1 with the watcher attached, twice:

```
frame   1014  health         97 -> 76
frame   1149  gamestate       0 -> 1
frame   1800  alive, gamestate=1 gamemap=1 health=76 playerstate=0
```

What rules out every other reading:

| Alternative | Ruled out by |
|---|---|
| a death | `playerstate` stayed 0 and `health` stayed 76 |
| a level change or warp | `gamemap` stayed 1 |
| back to the title | that is 3, measured independently |
| ordinary play continuing | health and armour frozen for 1183 frames |

The screenshot taken on the transition frame shows the player standing at the
exit switch, **not** the tally screen — which is correct and worth
understanding: Doom sets `gamestate` in `G_DoCompleted()`, one frame before the
intermission draws. That is the frame a time trial's clock should stop on (ADR
0005), not the frame the tally finishes animating.

`GS_FINALE` (2) is still unseen. It only occurs at the end of an episode and
nothing depends on it.

### A false positive worth remembering

An early unattended run logged `gamestate 0 -> 1` with nobody playing. It was
not a spurious value: that window had stolen focus and *was* being played.
Re-running it headless with no input held `gamestate` at 0 for 2000 frames.

Worth recording because it is exactly the shape of a false positive — the right
value for the wrong reason — and a screenshot is what distinguished the two.

## What the schema cannot express, and does not need to

The player's `mobj` pointer holds a DOS/4GW **linear** address, which is not the
emulated **physical** address the descriptors expose. Following it would need
pointer indirection *and* a base translation, and the rule language has neither
by design (ADR 0016).

It does not matter: everything above lives in the program's static data
(`players[]` and the `g_game.c` globals) and is reachable at a fixed address with
no indirection. Position-based criteria would not be, and that is worth knowing
before anyone proposes one.

## Running it

```sh
# what the core publishes: every option, its legal values, its default
cargo run --release -p session-libretro --bin harness -- --game doom --dump-options

# the three memory descriptors, and which are readable
cargo run --release -p session-libretro --bin harness -- --game doom --dump-memory-map

# boot from cold to E1M1 and capture the state a race entry restores
cargo run --release -p session-libretro --bin harness -- --game doom \
    --stage e1m1-hangar-boot --frames 120 --capture assets/states/doom_e1m1.state

# play it, from that state
cargo run --release -p session-libretro --bin harness -- --game doom --stage e1m1-hangar
```

Every key goes to the game, Escape included — Doom's menu needs it. **F12
quits**, and F1-F11 stay with the harness (F1 captures a state, F2 re-enters,
F3-F8/F10 are the RAM search, F11 the probe).

The RAM tools work over `[runner.libretro.memory.scan]` — 64 KB around the
player struct and the `g_game.c` globals. Widen it in the manifest when hunting
for something new.

### Exercising the rules without a human

Booting the plain bundle instead of the warping one lands in Doom's attract
demos, which play themselves and reliably produce damage, deaths and map
changes:

```sh
cargo run --release -p session-libretro --bin harness -- --game doom \
    --stage e1m1-hangar-boot --rom assets/ROMs/dos/doom-shareware.zip \
    --frames 16000 --no-audio
```

A 16 000-frame run of that reported 41 damage events, two deaths (frames 7996
and 15079) and two map changes (E1M5 -> E1M3 -> E1M7, each ~500 frames after a
death, which is the next demo starting). That is every rule in the manifest except `stage_cleared`,
which needs someone to reach an exit.

## Verifying the addresses

What would move this from "measured" to "playtested":

1. **Finish E1M1 from the `state` entry** and confirm `stage_cleared` fires once,
   at the exit switch, with the frame count the run actually took.
2. **Die deliberately, several times**, and confirm `died` fires exactly once per
   death with both fields agreeing, including deaths by falling and by barrel.
3. **Pick up armour and health** and confirm `damaged` never fires on an
   increase — the `decreases` edge should make that impossible, which is worth
   proving rather than assuming.
4. **Leave the tally screen up** for a long time and confirm `stage_cleared` does
   not repeat. `enters` should make that impossible; DKC's history says prove it.
5. **Take the last hit** and confirm the death frame reports `died` and not
   `damaged` — the `unless = ["death"]` guard is what stops both, and it is the
   one rule here whose interaction has not been watched live.
