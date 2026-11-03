# U.N. Squadron (SNES)

Two boss fights as survival runs: Minks, mission 5's battleship, flown the
length of once; and Wolfpack, mission 3's fighter squadron, shot down.
`manifests/unsquad.toml` is the manifest; this is where its numbers came from
and what has been seen to work.

## The entry states

`assets/states/unsquad_minks.state` and `unsquad_wolfpack.state` were made in
RetroArch and arrived as RZIP containers, as `sf2_vs.state` did (see
`docs/games/sf2.md`). `tools/rzip-unpack.py` takes the `MEM ` section out: a
`#!s9xsnp:0014` Snes9x snapshot, the format this tree's core writes. Minks
starts on the approach to the ship; Wolfpack at the start of the fight. The two
saves fly different planes: the jet's condition is 3 when whole on Minks' save
and 6 on Wolfpack's.

## How the addresses were found

No RAM map for the game was found published. cbroker1/jev-un-squadron
(`docs/notes/MEMORY_MAP.md`) mapped the same dump (sha1 `a2dd4857…`) under
BizHawk's Snes9x: the object table at `$1000`, 0x40 bytes a record, the
player first, a 24-bit routine pointer at +1..+3; and the level scroll at
`$007B`. It mapped no health, death or clear.

Everything else was found by restoring the entry states in this tree's
`snes9x_libretro.dll` from a scratch ctypes driver that played scripted input,
logged the whole of WRAM every few frames and wrote contact sheets of the
picture, so every candidate byte was read beside the frame it moved on. To see
the parts of a stage an unattended jet does not live to reach, the driver held
the player's invulnerability timer, `$1033` (player +0x33, 94 down to 0 one a
frame through the blink after a hit), at 90. That poke is research only; no
number below was taken from a run that relied on it for anything but staying
alive.

| Address | Width | What | Seen |
|---|---|---|---|
| `$1008` | u8 | player +8, the jet's condition | Minks 3 → 2 on the first hit (frame 288), → 0 on the fatal one (468). Wolfpack 6 → 4 (118), → 0 (225). Enters 0 on the frame the jet explodes; the routine leaves `$04:DF96` on the same frame |
| `$1003` | u8 | player +3, the routine's bank | 4 in flight and through the death. 0 only when the game wipes the object table, which zeroes `$1008` on the same frame — 2119 frames into Wolfpack, after the clear — so `died` and `damaged` are guarded on it |
| `$00AE` | u8 | mission number | 3 on Wolfpack, 5 on Minks. Poked to 4 before Wolfpack's clear screen, it read MISSION 4 CLEAR! |
| `$00A9` | u8 | the clear sequence | 0 → 1 at 1731 on Wolfpack, 93 frames after the last fighter's slot (`$14E9`) emptied, as the screen dims and the game takes the jet; 0 again at 1974 as the tally ends. Quiet through a death and through 30,000 frames of Minks |
| `$14F5` | u8 | Minks' camera direction | 0 → 1 at 2454 as the camera turns at the stern, 1 → 0 at 4855 at the bow, every pass after (twelve seen in 30,000 frames). Quiet through a death and the map. In Wolfpack it is another object's countdown, so the rule is guarded on mission 5 |
| `$00F4` | u8 | lives, one more than the HUD's `=N` | falls about 120 frames after the death as the screen fades; up one with the clear's bonus |
| `$007B` | u16 | scroll (jev) | half a pixel a frame; falls once Minks reverses; reset to 0 when the stage is left |

A death leaves the stage for the target map ("Select your target area!"), as a
clear does, so leaving is not a fact that says which happened; the jet's
condition and the clear sequence are.

The jet does turn round at Minks' stern: its X velocity (`$1017/$1018`) goes
negative and its sprite flips (`$1038` bit 3) about 180 frames after `$14F5`
sets. `$14F5` is the earliest of them, and the one the rule reads.

## The challenges

- `minks-survival`: `objectives = ["passed-minks"]`, `$14F5` entering 1 on
  mission 5. Reached 41 s in at the stage's own pace, whatever the player does,
  so the run is about staying alive under the ship's guns; the limit is 75 s.
- `wolfpack-survival`: `cleared = true`, `$00A9` entering 1. A jet firing
  straight ahead and never dodging cleared it in 29 s; the limit is 120 s.
  A stray shot between the last fighter falling and the clear sequence still
  kills, and `$00A9` comes after that window, so it counts against the run.

## What has been run

With `harness --challenge … --frames`:

- Both from their entry states with no input: hits reported as `damaged`, the
  death at the frames above, verdict `failed: died`.
- Both from scratch states captured shortly before the finish, invulnerability
  still running, passed with `--state`: Minks completed on `passed-minks` 74
  frames in, Wolfpack on `stage_cleared` 51 frames in.

Not yet: either fight played through by hand, or dealt in a match. Not found
and not needed yet: a boss health bar for either fight, and whether `$00A9`
means the same in the other missions.
