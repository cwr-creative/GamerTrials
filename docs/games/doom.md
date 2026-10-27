# DOOM (shareware, DOS)

The manifest is [`manifests/doom.toml`](../../manifests/doom.toml). This document
is the part that does not belong in it: where the addresses came from, what is
playtested and what is only documented, and what is different about a DOS game.

The first game in the library whose machine is a PC rather than a console. ADR
0018 is why it is here at all and why it arrives through `session-libretro`
under `dosbox-pure` rather than through an adapter of its own; ADR 0014 is the
RAM-watching decision it shares with the two consoles.

**The evidence here is better than DKC's, and it has been playtested.** Every
address was measured on a running machine rather than transcribed from a wiki —
several by typing a cheat and diffing 16 MB, which is a controlled write and
therefore strong evidence — and a human finishing E1M1 from the `state` entry
fires `stage_cleared`. The edge cases still worth watching in a long session are
listed under [Verifying the addresses](#verifying-the-addresses).

## The content bundle

Doom shareware v1.9, the freely redistributable `doom19s.zip` from the idgames
archive. Its `DEICE`/`DOOMS_19.*` installer is a split PKZIP self-extractor, so
no DOS-side install is needed: concatenating the parts gives an ordinary zip.

The bundle the manifest names is those two files plus two small text files it
does not ship with, rezipped:

| Member | SHA-1 |
|---|---|
| `DOOM.EXE` | `aa0668fae2f743ee5e3e5634ee42d3c8ace7d907` |
| `DOOM1.WAD` | `5b2e249b9c5133ec987b3ea77596381dc0d6bc1d` |
| `dosbox.conf` | `[autoexec]` running `C:` then `DOOM.EXE -warp 1 1` |
| `DEFAULT.CFG` | `f193767bde9b9b36a27d14d23780b27db79a74e3` — see [Sound](#sound) |

`dosbox-pure` mounts the zip as `C:` and runs the `[autoexec]`, so the bundle is
**one file with one hash** — the same shape as "one manifest per base ROM".

`[rom].sha1` is left unset on purpose. The archive's own hash depends on how it
was zipped, so `asset-store` will want to hash the members rather than the
container (ADR 0018).

Note what that member list means: **a DOS bundle carries the game's
configuration, not only its files.** A console ROM is the whole machine's
content and there is nothing else to get right. Here two of the four members are
ours, neither ships with the game, and one of them is the difference between the
game making a sound and not.

## Sound

`DOOM.EXE` and `DOOM1.WAD` alone boot correctly and play in **silence**, which
is the game working as designed rather than anything being broken. DOS DOOM does
not probe for a sound card: it reads one out of `DEFAULT.CFG`, which `SETUP.EXE`
normally writes at install time, and with no such file both devices fall back to
`0` — `snd_none` in DOOM's `cardenum_t`. An installation that never ran SETUP is
mute.

So the bundle carries one, and it is the whole of it:

```
sfx_volume		8
music_volume		8
snd_channels		3
snd_musicdevice		3
snd_sfxdevice		3
snd_sbport		544
snd_sbirq		7
snd_sbdma		1
snd_mport		0
```

Tab-separated, CRLF, 146 bytes. `3` is `snd_SB`; `544` is `0x220`. Those three
port/IRQ/DMA numbers are why `dosbox_pure_sblaster_conf` is now pinned in the
manifest next to the cycles — DOOM talks to the address it was told rather than
finding the card, so if the core's default ever moved, the game would go silent
again with nothing in any log to say why.

Music is the SoundBlaster's OPL, not MIDI, and that is deliberate.
`dosbox_pure_midi` defaults to `frontend`, meaning "ask the frontend for a MIDI
device"; there is no frontend MIDI interface in this harness, so it resolves to
nothing and the core logs `MIDI: Opened device:none` on every boot. The manifest
pins `disabled` so that line stops looking like a failure. `snd_musicdevice 2`
(`snd_Adlib`) measures identically to `3` and either is fine.

### How this was measured, and how to re-check it

Counting samples rather than listening, because "I can't hear anything" does not
distinguish a silent game from a broken output path. Over 900 frames the core
handed the harness 720,353 stereo sample-frames — exactly 900 × 48000/60, so the
mixer was running and `session_libretro::audio` was draining it correctly — at a
**peak amplitude of 0.0000**. Adding `DEFAULT.CFG` and changing nothing else took
that to **0.1840**. That is the whole diagnosis: the emulator's audio path was
never the problem.

### The entry state has to be recaptured with it

`assets/states/doom_e1m1.state` was recaptured after this change, and had to be.
A DOS state carries the whole machine, including a DOOM that has already
initialised its sound devices — so restoring the *old* state onto the *fixed*
bundle still measures 0.0000. Fixing the bundle without recapturing fixes
nothing a race would ever see, because a race enters at the state.

**Any change to the bundle means recapturing every state that entered on it.**
That is not special to sound; it is what "the state is the whole machine" costs.

## What is different about a DOS game

Four things, all in `[runner.libretro]`, and nothing below that line — plus the
bundle carrying configuration of its own, which is [Sound](#sound) above:

1. **No system-RAM slice.** `retro_get_memory_data` answers nothing for every
   id, `SYSTEM_RAM` included. `dosbox-pure` publishes three `SET_MEMORY_MAPS`
   descriptors instead — conventional memory, the HMA, and 15.4 MiB of extended
   memory at `0x200000` where a DOS/4GW program lives. So `prefer =
   "memory_map"` is the route that works, which is the route SMB3 already uses.
2. **A keyboard, not a pad.** `controller = "keyboard"`, and every host key
   passes through unchanged. Doom maps its own keys.
3. **Pinned core options.** See below. This is the load-bearing one — eight of
   them now: six that decide what machine is built, and two
   (`dosbox_pure_sblaster_conf`, `dosbox_pure_midi`) that the bundle's
   `DEFAULT.CFG` is written against.
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

### 26800 is not a performance ceiling, so do not raise it to fix a feel

Doom felt laggy long before anyone measured it, and `dosbox_pure_cycles` is the
obvious suspect: it is the setting that says how fast the machine is. It is the
wrong one, and raising it costs a real thing — every frame count in the library
becomes incomparable with every previous one — in exchange for nothing.

Measured per 16.67 ms frame, release build, entering at `e1m1-hangar`:

| | ms |
|---|---|
| `retro_run` plus the watcher (`--no-probe`) | 0.53 |
| `scan_window`, the probe's 64 KB read | 0.33 |
| `Probe::tick` | 0.01 |

Under 0.9 ms of 16.67 — the emulated machine has roughly eighteen times the
headroom it needs, at 26800, on the probe's default settings.
`dosbox_pure_perfstats = "simple"` shows the same thing on screen if you want to
watch it rather than take this table's word.

What the lag actually was is latency rather than throughput, all of it in the
harness's own loop, and it is fixed there: the loop used to read the keyboard
*before* waiting for the frame's turn, so a whole frame of pacing sat between a
keypress and the frame it reached; minifb only refreshes what `is_key_down`
reports when the window is pumped, which used to happen only at the bottom of
the loop; the audio queue that paces the loop was three frames (~50 ms) deep and
is now two; and minifb's own 250 fps limiter was still on underneath ours,
sleeping out the remainder of 4 ms inside every update. See the comments around
the loop in `src/bin/harness.rs` and `TARGET_QUEUED_FRAMES` in `src/audio.rs`.

One thing there is worth watching rather than fixing yet. `scan_window` reads
its window a byte at a time through `AddressSpace::read_u8`, and for a core with
no system-RAM slice every one of those bytes takes the `MEMORY_MAP` mutex and
walks the descriptor list — 65,536 lock/scan pairs per frame for the 64 KB
window below. That is the 0.33 ms, which is affordable. It is also linear in the
window, and the advice below is to *widen* the window when hunting for a new
address, so it is the first thing to hoist if a search ever feels slow.
`--no-probe` removes it entirely.

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
| `killcount` | `0x00389F44` | `players[0].killcount` | **predicted from `player_t`, then moved on every kill and nothing else** |
| `itemcount` | `0x00389F48` | `players[0].itemcount` | predicted; moved on two armor bonuses, not on two ammo clips |
| `secretcount` | `0x00389F4C` | `players[0].secretcount` | predicted; moved on entering a secret area |

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

### The level statistics: predicted, then played

`killcount`, `itemcount` and `secretcount` are what the tally screen reports,
and they sit in `player_t` straight after `refire`. With the struct pinned at
`0x00389E78` by `playerstate` and `armortype`, that puts them at `+0xCC`,
`+0xD0` and `+0xD4`. They were written down *before* anyone played, and then a
human played against a watch on exactly those twelve bytes:

| Event | Frame | What moved |
|---|---|---|
| armor bonus | 1756 | `itemcount` 0→1, with `armorpoints` 0→1 on the same frame |
| kill | 6543 | `killcount` 0→1 |
| *(new session)* kill | 3787 | `killcount` 0→1 |
| kill | 5449 | `killcount` 1→2 |
| ammo clip, twice | — | nothing |
| armor bonus | 14759 | `itemcount` 0→1, with `armorpoints` 0→1 |
| entered a secret | 18890 | `secretcount` 0→1 |

A prediction that then tracks every event and nothing else is a controlled test,
and is why no exhaustive search was needed. One was run alongside the kills
anyway: F3, kill, F7 took 65533 candidates to 1474, F4 to 673, and the second
kill's F7 to 222. It was reset before it could finish, which is why the evidence
is the watch rather than the search.

The ammo clips are the negative control, and they agree with Doom's own rules:
an item counts only if its thing carries `MF_COUNTITEM` — bonuses and
power-ups — never ammo, weapons, stimpacks or medikits. The manifest names the
item `tally_item` for that reason; it is not "anything picked up".

Doom zeroes all three when a level loads, so a restored entry state starts them
at 0 and a death's reload is a decrease, which the `increases` rules ignore.
`IDDQD` does not touch them, which makes god mode safe for measuring.

With the watches in the manifest and a rule on each, a third session played
with god mode on saw every rule fire on its event and nothing else:
`ENEMY DEFEATED` at frame 3504 (a kill), `ITEM tally_item` at 4012 (an armor
bonus, `armorpoints` 0→1 on the same frame), nothing for an ammo clip,
`COUNT secret` at 5373, and `ENEMY DEFEATED` again at 5410 — **a monster killed
by another monster**, not by the player.

That last one is Doom's rule rather than a false positive: in single player,
every countable monster's death is credited to the player's tally, whoever
caused it. So on Doom `enemy_defeated` means *a monster died*. A "kill N"
challenge is "N monsters die", infighting included — the same thing the tally
screen reports — and anyone wanting "N monsters the player killed" would need a
different detector, which this game does not obviously offer.

Not yet checked: a frame on which two monsters die together (the rule reports
the delta, so it should count two), and that the totals on the tally screen
match the counts. Both are worth a look before a challenge depends on an exact
number.

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
    --stage e1m1-hangar-boot --rom base=assets/ROMs/dos/doom-shareware.zip \
    --frames 16000 --no-audio
```

A 16 000-frame run of that reported 41 damage events, two deaths (frames 8002
and 15082) and two map changes (E1M5 -> E1M3 -> E1M7, each ~500 frames after a
death, which is the next demo starting). That is every rule in the manifest except `stage_cleared`,
which needs someone to reach an exit — and has since been seen firing when a
human finished E1M1.

Those frame numbers moved by six when `DEFAULT.CFG` was added — the deaths were
at 7996 and 15079 before it. Nothing about the rules changed; initialising a
sound device takes the boot a few frames longer, so the demos are a few frames
further along when the stage's own clock starts. Worth knowing before reading a
shift like that as a regression.

## Verifying the addresses

Doom is playtested: **finishing E1M1 from the `state` entry fires
`stage_cleared`.** What follows is the edge cases a long session should still
keep an eye on:

1. **Die deliberately, several times**, and confirm `died` fires exactly once per
   death with both fields agreeing, including deaths by falling and by barrel.
2. **Pick up armour and health** and confirm `damaged` never fires on an
   increase — the `decreases` edge should make that impossible, which is worth
   proving rather than assuming.
3. **Leave the tally screen up** for a long time and confirm `stage_cleared` does
   not repeat. `enters` should make that impossible; DKC's history says prove it.
4. **Take the last hit** and confirm the death frame reports `died` and not
   `damaged` — the `unless = ["death"]` guard is what stops both, and it is the
   one rule here whose interaction has not been watched live.
