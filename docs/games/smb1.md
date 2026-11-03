# Super Mario Bros. (NES)

One challenge: beat Bowser on the bridge at the end of World 8-4, as a
survival run. `manifests/smb1.toml` is the manifest; this is where its entry
state and its numbers came from, and what has been seen to work.

The ROM is `assets/ROMs/Super Mario Bros. (World).nes`, 40,976 bytes: the
No-Intro dump (headerless sha1 `facee9c5…`) behind an NES 2.0 header. The
manifest's sha1 is the whole file, as SMB3's is. The core is the same
`cores/mesen_libretro.dll`.

## The entry state

`assets/states/smb1_8-4-bridge.state` was captured headless, without a
player. A scratch ctypes driver over the Mesen core did it, in the same way
`docs/games/unsquad.md`'s addresses were found. It only poked values the game
writes itself, and the game's own routines did the rest:

1. **To 8-4.** On the title screen, `WorldNumber` (`$075F`) = 7,
   `LevelNumber` (`$075C`) = 3, `AreaNumber` (`$0760`) = 3, then Start. A
   plain Start keeps whatever world is in RAM and loads the area pointer from
   it, so the game came up on WORLD 8-4 with three lives, as small Mario. The
   castle's area pointer (`$0750`) read `$65`.
2. **Through the pipes.** SMB1 keeps a pipe's destination in the area's
   *enemy* data, as a three-byte object on row `$E`: the area pointer, then the
   entrance page with the world it applies to. 8-4's enemy data, read from the
   ROM through the pointer at `$E9/$EA`, sends the pipe on page 14 to the
   water area `$02`. The water area's one exit sends Mario back to `$65` at
   page 16. That is the last stretch: a Hammer Bro, podoboos, Bowser's
   flames, and the bridge.
   Each hop was made as `ChgAreaPipe` makes it when its timer runs out:
   `AreaPointer` (`$0750`) and `EntrancePage` (`$0751`) are set to the
   destination, and `AltEntranceControl` (`$0752`) to 2, the castle and
   side-pipe mode. Then `DisableScreenFlag` (`$0774`) is incremented and
   `OperMode_Task` (`$0772`) and `Sprite0HitDetectFlag` (`$0722`) are
   cleared. The game ran `InitializeArea` itself, and Mario rose out of the
   pipe (`$000E` 7, then 8).
3. **To the bridge.** A search over random inputs, restoring states, carried
   Mario past the Hammer Bro to the last stone pillar. The pillar ends at
   page 17 x 252, and Bowser's bridge starts at page 18 x 0. Several hundred
   ways of stopping on it were tried, each finished with a tap of right so
   Mario faces the bridge. Each was then left idle to see how long a player
   has before a flame arrives. Standing still in the flame's line is death
   after a few frames at a bad moment.

The state as captured: small Mario stopped at page 17 x 244, y 128, facing
right, on the ground, three lives, TIME 363. Left alone, a flame kills him
308 frames (5.1 s) in. Bowser is just off the right edge of the screen and
comes into view as Mario walks on.

A clock of 363 is about 145 s at the game's 24 frames a tick. The run's limit
is 60 s, so the time-up death cannot be reached inside a run.

## How the addresses were found

All of them are internal RAM, so `prefer = "system_ram"`. SMB1 has no
cartridge WRAM, unlike SMB3. The names are the community disassembly's;
the behaviour was measured here from the entry state:

| Address | What | Seen |
|---|---|---|
| `$0770` | `OperMode`: 0 title, 1 playing, 2 victory, 3 game over | 1 → 2 on the frame the axe was touched, as the bridge began to fall |
| `$000E` | `GameEngineSubroutine`, the player routine this frame | 7 rising from a pipe, 8 in control. 8 → 11 on the frame a flame or Bowser killed Mario (idle: frame 307; walking into Bowser: 109). → 6 about 220 frames later as the life went, then 0 as the area reloaded |
| `$0712` | `DeathMusicLoaded` | 0 → 1 on the frame Mario dropped below the screen into the lava (Y high byte `$00B5` 1 → 2, frame 97 of holding left). `$000E` stayed 8 until → 6 245 frames later. Untouched by both flame and Bowser deaths |
| `$075A` | lives − 1 | 2 at the entry; 1 as `$000E` reaches 6 |
| `$075F` / `$075C` | world − 1, level − 1 | 7 and 3 throughout |

So a death has two first signs, and the manifest reads both:

- **A hit** (`$000E` enters 11): a flame, a hammer, or Bowser.
- **The lava** (`$0712` leaves 0).

Neither fires for the other kind of death, so neither needs a guard.

## The challenge

`8-4-bowser-survival`: `objectives = ["bowser-defeated"]`, emitted when
`$0770` enters 2. A `stage_cleared` rule on the same edge makes it terminal.
Touching the axe is how SMB1 defeats Bowser. It drops the bridge, and he goes
with it. It is also the only way 8-4 ends from this state: small Mario has no
fireballs, and the last stretch has no item block.

The random search reached the axe 154 frames (2.6 s) after the entry by
jumping over Bowser. The limit is 60 s, which leaves a human room to wait for
an opening.

## What has been run

With `harness --challenge 8-4-bowser-survival --frames`:

- From the entry state, no input: `DEATH` at frame 308, hit by a flame;
  `failed: died in 308 frames`.
- From a scratch state 12 frames short of the axe, Mario in the air and
  carried by his own momentum: `OBJECTIVE bowser-defeated` and
  `STAGE CLEARED` on frame 12; `completed in 12 frames`.
- From a scratch state taken over the lava after holding left: `DEATH` on
  frame 8, fell into the lava; `failed: died`.

Not yet: the fight played by hand in the harness window, or dealt in a match.
