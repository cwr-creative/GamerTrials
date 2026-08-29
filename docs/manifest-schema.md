# Game manifest schema

**Status: proposed, `schema_version = 3`. Implemented by `session-libretro`;
three manifests in `manifests/` are the worked examples — two consoles and a
DOS game, one of which also carries a hand-authored hack and two generated
challenges.**

A manifest is everything the platform knows about one game: the content it runs
on, how to read it, and every challenge a player can be dealt in it. Adding a
libretro game is adding one of these files and nothing else — no Rust, no
rebuild (invariant 5), and that now covers a ROM hack and a generated challenge
as well. The loader (`crates/session-libretro/src/manifest.rs`) is the authority
on what parses; this document is the authority on what the fields *mean*.

One manifest per **base ROM**, and a manifest may carry ROMs *derived* from that
base — hand-authored hacks, each declared as a BPS patch against it — with every
`[[stage]]` saying which one it runs on (ADR 0020). The condition is that the
telemetry holds unchanged across all of them: a hack that moves an address is
still two manifests. A game with two revisions whose addresses differ is two
manifests for the same reason.

Generated artifacts get no `[[rom]]` entry of their own. They are identified by
a derivation key (ADR 0013) and derived per attempt from a seed, out of whatever
ROM their challenge's stage names.

Two of the fields below reach `content-gen` rather than the adapter —
[patch slots](#patch-slots) and a derived `[[rom]]`'s `patch`. That crate is the
authority on what a slot may say, and `crates/content-gen/src/slots.rs` is where
the shapes are defined.

## Three words

Used precisely throughout, and worth learning before reading further (ADR 0017):

* **stage** — a place, and the entry point that reaches it. Setup, not scoring.
* **challenge** — what a player is actually dealt: one stage, one kind, and the
  criteria that complete it. A stage may carry several.
* **attempt** — one entry-to-terminal-event span, which is what the watcher
  latches. A **run** is a player's whole go at one challenge: for survival that
  is one attempt, for a time trial it is every attempt summed.

## Contents

| Table | What it settles |
|---|---|
| [`[game]`](#game) | identity, console family, framerate |
| [`[[rom]]`](#rom) | the content this game runs on: one base, and any hacks of it |
| [`[runner]`](#runner) | which adapter runs it, how it is controlled, and how it reaches memory |
| [`[watch.<name>]`](#watchname) | the addresses to read, and where each came from |
| [`[label.<field>]`](#labelfield) | what a value *means*, for the log |
| [`[display]`](#display) | which fields to print when one of them moves |
| [`[[marker]]`](#marker) | a transition that *times* an event announced late |
| [`[[rule]]`](#rule) | which transition means what |
| [`[attempt]`](#attempt) | what re-opens scoring after an ending |
| [`[[stage]]`](#stage) | where a run starts |
| [`[[challenge]]`](#challenge) | every challenge this game offers, and what completes each |
| [patch slots](#patch-slots) | how a generated challenge's ROM is rolled from a seed |
| [alternative ROMs](#alternative-roms) | hand-authored hacks, as patches against the base |

Every table rejects unknown keys. That is deliberate: a permissive loader turns
a mistyped key into a detector that silently never fires, which is
indistinguishable from a wrong address and much harder to notice.

---

## `[game]`

```toml
[game]
id = "dkc"                     # matches the manifest's filename by convention
title = "Donkey Kong Country"
console = "snes"               # closed, small set: one entry per console family
region = "usa"                 # optional
framerate = 60.0988
```

`framerate` is the manifest's own number, not the core's. It is what durations
authored in seconds are converted at, so it must be the same on every machine —
a core reporting something different is a warning, not an override.

`console` names the machine family, once per family, never per game. It is what
`content-gen` keys its two per-console rules off: how many bytes of header a
patch-slot offset is counted past, and what checksum a substitution has to repair
(SNES has an internal header checksum; NES has none; a DOS bundle is refused
outright — ADR 0015, ADR 0020). `nes`, `snes` and `dos` are what the library uses
today.

Outside those two rules it is documentation rather than a switch: nothing else in
the loader matches on it, and nothing behaves differently because a manifest says
`dos`. What actually makes a DOS game a DOS game is `[runner.libretro]` — the
memory route, the controller, the pinned options — and that is the point. A
console is a description; the behaviour is declared. A game that derives no
content never has its `console` read at all, so an unrecognised one is only an
error where it would decide something.

## `[[rom]]`

The content this game runs on. **Exactly one entry omits `from`, and that one is
the base** — the dump a player supplies for themselves. Every other entry is a
derivation from it (ADR 0020).

```toml
[[rom]]
id = "base"                    # what a [[stage]] names
name = "Donkey Kong Country (U) (V1.0) [!]"
path = "assets/ROMs/snes/Donkey Kong Country (U) (V1.0) [!].smc"
sha1 = "..."                   # optional on the base today; `asset-store` will require it
header = "copier-optional"     # "none" | "ines" | "copier-optional"
min_bytes = 32768              # optional size floor
```

`header` is per console family, and it is all that can be checked without a
hash: the NES has a magic number, the SNES has none at all — its header is
in-band, at a position that depends on the mapper — so the SNES gets a size floor
and a note when the size implies a 512-byte copier header, which shifts every
ROM offset and makes an address table look wrong for no visible reason.

A second `[[rom]]` is a hand-authored hack; see
[alternative ROMs](#alternative-roms) for the derived form and the six rules that
make an array of them mean one thing.

## `[runner]`

```toml
[runner]
kind = "libretro"              # the only adapter there is (ADR 0018)
```

Everything an adapter needs that another adapter would not understand lives
under `[runner.<kind>]`. That is what keeps adapter vocabulary out of the shared
part of the schema (invariant 3) while still letting a manifest say `core_dir`.

`libretro` is currently the only `kind`, and ADR 0018 is why: a game qualifies
by running on an *emulated machine*, so DOS games arrive here under dosbox-pure
rather than through an external-process adapter. There is no `[runner.external]`
and there will not be one. The `kind` field stays because the shape is right,
not because a second value is expected.

**DOS games load.** `manifests/doom.toml` is the worked example, and what it
needed beyond the two console manifests is four fields, all in this section:
`controller`, `boot_seconds`, `[runner.libretro.options]` and
`[runner.libretro.memory.scan]` — plus `u32` as a watch width. Nothing in
`[watch]`, `[[marker]]`, `[[rule]]`, `[attempt]`, `[[stage]]` or
`[[challenge]]` changed, which is the result ADR 0018's spike was run to
establish.

### `[runner.libretro]`

```toml
[runner.libretro]
core_dir = "cores/snes"        # must hold exactly one *_libretro.dll
controller = "pad"             # optional, default "pad"; "pad" | "keyboard"
pad_layout = "snes"            # "nes" | "snes"; only with controller = "pad"
boot_seconds = 16.0            # optional; required by a `named` entry point
```

`controller` says how port 0 is driven.

* **`pad`** — `RETRO_DEVICE_JOYPAD`. `pad_layout` then decides how a modern
  controller's face diamond maps onto the console's. A two-button console takes
  South *or* East as A so both habits work; a four-button one cannot, because
  every position is already taken.
* **`keyboard`** — `RETRO_DEVICE_KEYBOARD`, and every host key passes straight
  through as its `RETROK_*` code. There is deliberately nothing to lay out: a
  DOS game maps its own keys, Doom's controls are configured inside Doom, and a
  manifest that remapped them would be overriding a choice the player has
  already made. Declaring `pad_layout` alongside it is an error rather than
  ignored.

`boot_seconds` is how long a cold boot takes to reach the game — DOS starting,
the bundle's `[autoexec]` running. It converts to frames at `game.framerate` at
load, like every other duration, and it is only consulted by a `named` entry
point. A console manifest omits it.

### `[runner.libretro.options]`

```toml
[runner.libretro.options]
dosbox_pure_cycles = "26800"
dosbox_pure_force60fps = "true"
```

Core options this game is pinned to, checked at startup against what the core
publishes: an unknown key, or a value the core does not list, is a load error
rather than a silently kept default.

**This table is part of a game's identity, not a preference**, and it is a new
kind of thing in a manifest — not an address, not a hash, but a machine
configuration without which two players are not playing the same game.
`dosbox_pure_cycles` is the case that forced it: its default is `auto`, which is
*defined* as tracking host performance. Addresses do not move with it, but frame
counts do — the same demo damage lands on frame 501 at 26800 cycles and frame
452 on `auto` — so a frame count is only comparable between players on the same
value (ADR 0018, invariant 4).

Keep the list short and reviewed. It is not a place to park preferences, and
anything in it that does not change what a frame *means* probably does not
belong.

Run `harness --game <name> --dump-options` to see what a core actually
publishes: every key, its legal values, its default, and every environment call
the core asked the frontend for and did not get.

### `[runner.libretro.memory]`

```toml
[runner.libretro.memory]
base = 0x7E0000                # where the system-RAM slice starts, in console numbering
size = 0x20000                 # how much of it there is
prefer = "system_ram"          # "system_ram" | "memory_map"
endian = "little"              # optional, default little
default_width = "u16"          # optional, default u8
system_ram_bytes = 131072      # optional; startup warns if the core disagrees
```

`prefer` is the field most likely to be got wrong, and getting it wrong is
invisible. It says which of the two routes to a byte is tried first:

* **`memory_map`** — the descriptors the core published through
  `SET_MEMORY_MAPS`. Correct for the NES: SMB3's card counter is in cartridge
  WRAM, Mesen does not publish that as save RAM (it is not battery-backed on
  this cart), and the map is the only route to it.
* **`system_ram`** — the `RETRO_MEMORY_SYSTEM_RAM` slice. Correct for the SNES:
  snes9x hands over all 128 KB of work RAM and publishes no descriptors at all.

The other route is always the fallback, so both work; the order is what decides
whether an address outside system RAM resolves.

`base` also gives an address its second spelling. With `base = 0x7E0000`,
`0x0577` and `0x7E0577` name the same byte, so a manifest can quote a published
RAM map verbatim whichever convention its author used.

A core may publish **no system-RAM slice at all** — dosbox-pure answers
`retro_get_memory_data` with nothing for every id, and publishes three
`SET_MEMORY_MAPS` descriptors instead. That is not a failure: `prefer =
"memory_map"` is the route the NES manifest already uses, and it works
unchanged, disjoint regions included. Such a manifest leaves `system_ram_bytes`
unset and declares a scan window below. A core offering *neither* route is
refused at startup, because then there is no way to read a byte at all.

### `[runner.libretro.memory.scan]`

```toml
[runner.libretro.memory.scan]
at = 0x380000
size = 0x10000
```

Optional. Where the harness's RAM discovery tools — the F3-F8 search and the
F11 probe — look.

Consoles need none of this: the system-RAM slice *is* the window, it is small,
and every interesting byte is inside it. A DOS machine offers 16 MB, which is
both too big to snapshot every frame and mostly not worth searching, so a
manifest points the tools at the part that matters. Startup says how much of a
declared window no route reaches, because a window aimed at unmapped space
looks exactly like a search that keeps finding zeros.

Widen it while hunting for something new and narrow it again afterwards. It
affects no detector and no event — only where a human goes looking.

## `[watch.<name>]`

```toml
[watch.lives]
at = 0x0575                    # offset or full address; omit if unknown
width = "u16"                  # "u8" | "u16" | "u32"; default memory.default_width
note = "RA notes: lives. $0577, which every older map quotes, is the display copy"
```

The `name` is what every rule, marker, guard and `--addr` override refers to.

`width` reads that many bytes in the space's declared byte order. `u32` exists
for DOS rather than for a console: a 16-bit program's globals are C `int`s, so
every field `manifests/doom.toml` watches is four bytes wide. A value with any
unreachable byte reads as nothing rather than as a partial number.

**Omitting `at` is a first-class state**, not a missing value. It means nobody
has found the address yet; every rule over the field then stays quiet rather
than guessing. `--addr <name>=off` puts a field into that state at runtime, and
`--addr <name>=0x1234` takes it out of it — both without a rebuild.

`note` is where the address came from. Write it. The difference between "two
independent sources agree" and "a wiki said so" is the difference between a
detector and a rumour, and six months later the note is the only record.

## `[label.<field>]`

```toml
[label.animal]
0x00 = "none"
0x09 = "Rambi"
```

What a value means, for the log. Keys may be hex (`0x09`) or decimal. This is
where a name table goes: DKC's 230 level names were 347 lines of Rust and are
now 230 lines of this.

It is also where a manifest expresses arithmetic it is not allowed to do. SMB3
keeps *world − 1* at `$0727`; there is no `+1` in the rule language on purpose,
so `[label.world]` maps `0x00` to `"world 1"` and the log stays readable.

## `[display]`

```toml
[display]
fields = ["in_stage", "level_id", "lives", "bananas"]
```

The running commentary: one line whenever any listed field changes, with labels
applied. Leave position fields out — they change every frame the player moves.

## `[[marker]]`

A transition that **times** an event the game announces late. Not a verdict of
its own.

```toml
[[marker]]
name = "fade"
field = "fade_type"
edge = "enters"
value = 0x82
window_frames = 120

[[marker.capture]]             # optional, repeatable
name = "paused"
field = "state_flags"
test = "bit_set"
bit = 6
```

DKC is the case this exists for. The screen starts fading before the overworld
finishes loading, and ADR 0005 says the hand-off is not the player's time — so
a clear is reported at the frame of the fade, not the frame the map appeared. A
marker arms on its edge, and expires after `window_frames` if nothing comes of
it, because a marker left armed will mis-time a later, unrelated ending.

A **capture** records whether a test held *at the moment the marker armed*.
That is the only way to tell a pause-menu quit from a real exit in DKC: both
reach the overworld the same way, and by the time the map is up the menu is long
gone.

## `[[rule]]`

One transition, and what it means. Rules are evaluated in declaration order.

```toml
[[rule]]
name = "death"                 # optional; only needed if another rule cites it
field = "lives"
edge = "decreases"
emit = "died"
```

| Key | |
|---|---|
| `field` | a `[watch.<name>]` |
| `edge` | how it has to move — see below |
| `value` / `bit` | the edge's argument, where it takes one |
| `bit_labels` | names for the bits of a `bits_set` rule, low bit first |
| `guard` | an inline test on *any* field's current value, e.g. `{ field = "animal", test = "ne", value = 0 }` |
| `unless` | rule names that suppress this one when they fired on the same frame |
| `timed_to` | a `[[marker]]` name; the event reports that marker's frame |
| `if_marked` | a capture name on that marker; the rule fires only if it held |
| `emit` | what happened — see below |
| `reason` | `stage_exited` only: `death`, `quit` or `unknown` |
| `item` | `item` only: the item's name |
| `objective` | `objective` only: the milestone's name |
| `log` | a sentence appended to this rule's console line |

### Edges

| `edge` | Fires when | Argument |
|---|---|---|
| `increases` | `cur > prev` | — (the event's count is the delta) |
| `decreases` | `cur < prev` | — |
| `changes` | `cur != prev` | — |
| `enters` | `prev != v && cur == v` | `value` |
| `leaves` | `prev == v && cur != v` | `value` |
| `bit_set` | that bit went 0 → 1 | `bit` |
| `bit_cleared` | that bit went 1 → 0 | `bit` |
| `bits_set` | one event per bit newly set | `bit_labels` |

Everything is an **edge**, never a level: a flag held at a value is one event,
not one per frame. `leaves` with `value = 0` is how "rose from zero" is spelled,
which is what a hit timer or a transit countdown needs — a timer still counting
down from an earlier hit is not a second hit.

A `value` or `bit` on an edge that does not read one is an error, not something
ignored, because it means the author meant a different edge.

### Emits

| `emit` | `SessionEvent` | Terminal |
|---|---|---|
| `stage_cleared` | `StageCleared` | yes |
| `stage_exited` | `StageExited { reason }` | yes |
| `died` | `Died` | no |
| `damaged` | `Damaged` | no |
| `enemy_defeated` | `EnemyDefeated { count }` | no |
| `item` | `ItemCollected { item, count }` | no |
| `objective` | `ObjectiveReached { objective }` | no |
| `note` | nothing — logged only | no |

An emit names a **fact**, never a verdict. "The player died" is something an
adapter can observe; "the run failed" depends on the challenge — a survival run
and a time trial answer it differently — and belongs to `challenge-core`. There
is deliberately no `stage_failed`.

`objective` is a **named milestone** the game announced: a boss's health
reaching zero, a gate opening, a progress counter crossing a threshold. It is
how "defeat the boss" and "reach the halfway point" are expressed, and it is a
fact in the same sense the others are — whether reaching it wins anything is
`[[challenge]]`'s question, not this rule's.

```toml
[[rule]]
field = "boss_hp"
edge = "enters"
value = 0
emit = "objective"
objective = "boss-defeated"
```

`note` is for a signal worth watching during a playtest that is not yet known to
mean anything. SMB3's pipe-transit countdown is one: inventing a `SessionEvent`
for it would be a guess.

**Terminal rules end the attempt**, and the first one to fire wins — later ones
are suppressed until the attempt re-opens. That is what stops a game walking its
own menus back to the map from reporting twice, and it is why the quit rule in
`dkc.toml` is declared *before* the clear rule that watches the same edge.

## `[attempt]`

```toml
[[attempt.reset_on]]
field = "return_to_map"
edge = "enters"
value = 0
```

What re-opens scoring after a terminal event, short of re-entering the stage.
Optional, and SMB3 is why it exists: the return-to-map flag clearing means the
hand-off finished, so the next attempt gets a clean slate. Without it the
verdict latches for the whole process, and one card early in a run silences
every later death — a bug this project has already shipped once.

## `[[stage]]`

Where a run starts, and nothing else. A stage is not a challenge and carries no
completion condition of its own.

```toml
[[stage]]
id = "mine-cart-carnage"
name = "Mine Cart Carnage"
rom = "base"                   # a [[rom]] id; required on every stage
entry = { state = "assets/states/dkc_mine-cart-carnage.state" }
note = "forced-scrolling: everyone finishes in the same number of frames"
```

`rom` is **required in every manifest, including one that declares a single
ROM**. Nothing about a stage is inferred, for the same reason nothing about a
challenge is (ADR 0017), and the field is not decoration: an entry state is bound
to one ROM's bytes, which ADR 0019 measured rather than assumed. ROM and entry
point live in the same table because they are chosen together — and because
several challenges share a stage and must not be able to disagree about which
machine they are on.

`entry` takes exactly one of:

* `state = "<path>"` — a captured machine state to restore. An emulator
  savestate is one form of this, and it is what a real session uses: measured
  at 60 frames for Doom, against ~960 for a cold boot. That difference is what
  a death in a time trial costs.
* `named = "<name>"` — a location the game can reach by itself. Nothing is
  passed to the core: a DOS bundle carries its own `dosbox.conf`, whose
  `[autoexec]` is the command line that does the warping, so the name
  identifies the place and the content knows the way. The runner reloads the
  content and runs `runner.libretro.boot_seconds` worth of frames, which a
  manifest using this form must declare.

**The two are not alternatives; they are a pipeline.** A `named` entry is how a
state gets captured in the first place (ADR 0002), which is why both spellings
survive. `manifests/doom.toml` declares one stage of each kind for the same
place, and the authoring loop is one command:

```sh
# boot to where the game starts itself, play 120 frames, capture the state
harness --game doom --stage e1m1-hangar-boot --frames 120         --capture assets/states/doom_e1m1.state
```

or interactively: enter at the `named` stage, play to wherever the stage should
begin, and press F1.

Stage identity always comes from the entry point that put the player there,
never from RAM. SMB3 has no "current level number" at all — a level is
identified by the map tile it was entered from.

A stage with no `[[challenge]]` pointing at it is dead weight: it can be entered
with the harness, and a player will never be dealt it.

## `[[challenge]]`

**What a player is actually dealt.** One stage, one kind, and the criteria that
complete it. This is the table a contributor spends their time in, and it is the
reason adding a challenge is a data change (ADR 0017).

```toml
[[challenge]]
id = "mine-cart-carnage-survival"
name = "Mine Cart Carnage"     # optional; defaults to the stage's name
stage = "mine-cart-carnage"    # a [[stage]] id
kind = "survival"              # "time_trial" | "survival"
complete_when = { cleared = true }
note = "optional, for whatever the id and the criteria do not say"
```

**Nothing is inferred.** Every challenge that can be dealt is written down. A
game whose 100 levels should all be playable as time trials carries 100 entries;
if they should also all be playable as survival runs, that is 200. There is no
`kind = "both"` shorthand, because the two kinds rarely want the same criteria —
the survival version of a level is usually "reach the end", while the time trial
version may be "reach the end" or may be "stomp five enemies as fast as you can"
— and a shorthand that only works when they coincide hides the challenges that
exist.

Several challenges may share one stage. That is the normal case, not an edge
case: one stage plus three sets of criteria is three challenges.

A challenge may additionally carry `generator_version` and a `patch_slot` table,
which makes it a *generated* challenge: same stage, same kind, same criteria, on
a ROM rolled per attempt from a seed. See [patch slots](#patch-slots).

### The two kinds

They differ in what a death costs and what gets scored, **not** in what
completes them.

| | `time_trial` | `survival` |
|---|---|---|
| On death | re-enter at the stage's entry point; the clock keeps running | the run is over; the player spectates |
| Scored on | total frames across every attempt | whether the criteria were met at all |
| Ends when | the criteria are met — that is when the clock stops | the criteria are met, or the player dies |

Because a death ends a survival run by definition, **"without dying" is not a
criterion** — it is what the kind means. That is why one criteria vocabulary
serves both.

Some challenges only make sense as one kind, and the manifest says so by simply
not declaring the other: a forced-scrolling level where everyone finishes in the
same number of frames has no time trial, and a room where the player cannot die
has no survival run.

### `complete_when`

A closed set of predicates over facts the manifest's own `[[rule]]`s emit.
**Every criterion listed must hold** — there is no `any`, no negation, no
nesting, no arithmetic. At least one is required.

| Criterion | Satisfied when | Needs |
|---|---|---|
| `cleared = true` | a rule emitting `stage_cleared` fired | some `emit = "stage_cleared"` rule |
| `survive_seconds = 45.0` | the run reached that many frames. **Survival only** | — |
| `enemies_defeated = 20` | the `EnemyDefeated` counts sum to at least 20 | some `emit = "enemy_defeated"` rule |
| `items_collected = { banana = 50 }` | each named item's counts sum to at least its number | a rule emitting each item |
| `objectives = ["boss-defeated"]` | every named objective was reached | a rule emitting each objective |

```toml
# Reach the end, without dying.
[challenge.complete_when]
cleared = true

# Clear it having stomped five enemies on the way — both, not either.
[challenge.complete_when]
cleared = true
enemies_defeated = 5

# Kill the boss, as fast as possible. The clock stops on the objective; there is
# no need to reach the end of anything.
[challenge.complete_when]
objectives = ["boss-defeated"]
```

`survive_seconds` is **converted to frames at load**, at `game.framerate`.
Seconds never reach a `SessionEvent` or a wire message. It is refused on a time
trial, where "survive 30 seconds fastest" is not a thing that can be won.

Every criterion is checked against the rules in the same manifest at load time.
An `items_collected` naming an item nothing emits, an objective nothing
announces, or `cleared` in a manifest with no `stage_cleared` rule is a load
error, not a challenge that quietly cannot be completed — the same argument that
makes every table reject unknown keys.

### What is deliberately missing: a failure bound

Neither kind has one yet. A time trial where a death only restarts has no losing
state, and a survival challenge whose criterion the player can simply decline to
pursue (`enemies_defeated`, with the player standing somewhere safe) has the
same problem — `survive_seconds` is the only criterion that terminates itself.

A per-challenge time limit, a per-round cap, or a forfeit will be needed before
either kind ships. It is not in this schema because nothing enforces it yet, and
a field that parses but does nothing is exactly what ADR 0015 refused. It is
`challenge-core`'s to settle, along with the round clock. See ADR 0017.

---

## Patch slots

**What makes a challenge *generated*.** Everything else on a `[[challenge]]` is
unchanged: same `kind`, same `complete_when`, same stage. A generated challenge
additionally says how the ROM it runs on is derived, per attempt, from a seed the
session issues (ADR 0013). The engine is `content-gen`, and it is generic — a
contributor writes offsets and legal values, never Rust (ADR 0015).

```toml
[[challenge]]
id = "bro-roulette-time-trial"
name = "Bro Roulette"
stage = "bro-roulette"
kind = "time_trial"
complete_when = { enemies_defeated = 4 }
generator_version = 1          # bumped, never edited in place (ADR 0013)

# Roll one value from an explicit pool of legal ones.
[[challenge.patch_slot]]
offset = 0xC6CB                # into the ROM the stage names, after any header
width = "u8"
values = { pool = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87] }

# Or from an inclusive numeric range.
[[challenge.patch_slot]]
offset = 0x1A2B7
width = "u16"
endian = "little"              # only on a u16 or u32; on a u8 it is an error
values = { range = { min = 1, max = 8 } }

# Or as one member of a group, whose pool is consumed by permutation: which
# enemy occupies which of an existing set of spawn slots, rather than each slot
# rolled independently.
[[challenge.patch_slot]]
offset = 0x1A2BB
width = "u8"
values = { group = "enemy-shuffle" }

[challenge.group.enemy-shuffle]
pool = [0x01, 0x02, 0x03, 0x04]
mode = "permutation"           # the only mode there is
```

`manifests/smb3.toml` carries two worked examples, and `docs/games/smb3.md` has
the disassembly they were read out of.

### The four keys

| Key | |
|---|---|
| `offset` | into the ROM **the stage names**, counted **after any header** — 16 bytes for iNES, 512 for a SNES copier header, and the engine sniffs which from the content rather than trusting the manifest |
| `width` | `u8`, `u16` or `u32` |
| `endian` | `little` (default) or `big`. Declaring it on a `u8` is an error rather than ignored, because it decides nothing there |
| `values` | exactly one of `pool`, `range` or `group` |

`generator_version` sits on the challenge, and the three keys go together: a
table with no version cannot be corrected without silently invalidating every
artifact ever derived from it, and a version with no table describes a generator
that does nothing. Both are load errors.

### Duplicates in a pool are weights

This is the single most consequential line in the schema, and an engine that
"cleaned it up" would change what challenges mean without saying so.

```toml
values = { pool = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87] }
```

Four goombas against one each of four bro variants: a coin flip on whether this
goomba is replaced, then an even choice among the four. Deduplicating that pool
turns a 50% roll into a 20% one. **Duplication is the only way this schema can
express a non-uniform roll at all** — there are no weights, and a pool is a
multiset, not a set.

### A group is a permutation, not a correlation

A group's pool is drawn **without replacement**, so each slot in the group gets a
different member. That is how "shuffle which enemy stands in which existing spawn
slot" is spelled. The pool may be longer than the group, in which case the roll
is a random subset in random order; it may not be shorter, and that is a load
error.

It is the wrong tool for "these three slots should agree", and there is
deliberately no tool for that. SMB3's Sun Storm wants a uniform one-to-three
suns, which would need the three slots to share one roll; the schema cannot say
it, so the challenge takes the 25/50/25 that independent pools give and
`docs/games/smb3.md` records why.

### The derivation key

Everything an artifact's identity depends on (ADR 0013):

```
(hash of the ROM the stage names, challenge id, generator_version, seed)
```

The `generator_id` is the **challenge's id**, because under ADR 0015 the
generator is not a piece of code — the engine is shared — it is this table. Two
challenges with different slots are different generators even though they run the
same Rust, and two challenges with *identical* slots still derive different
artifacts from one seed, which is what stops playing one from revealing the other.

The first element is a hash, not "the base ROM": a generated challenge on a hack
names the hack's hash, and nothing else about the mechanism changes.

Every player derives the artifact locally, from the seed the server issued, and
reports what it hashed to. A mismatch aborts the challenge rather than letting two
people race on different content. So the generator's output is a wire contract in
everything but name — the RNG is written out by hand in `content-gen` rather than
taken from a crate, and any change to it is a `generator_version` bump for every
challenge in the library.

### What the engine will not do

It writes one declared-legal value per slot into an otherwise-untouched copy of
the ROM, then applies the console-family checksum fixup. It creates no slots,
relocates nothing, and never changes the file's length: **structural or geometric
generation is out of scope permanently, not pending** (ADR 0015). A challenge that
needs a new object or new geometry wants a [hack](#alternative-roms), authored by
a human in a level editor — and the two compose, which is the expected shape
rather than an exotic one.

That restriction is also load-bearing rather than tidy. ADR 0019 enters a
generated ROM by rewriting the ROM hash in a copy of the entry state, and the
transplant is only sound because the derived ROM has the same length, mapper and
geometry as the one the state was captured on. A structural patch would break a
rebound state *silently*.

Whether a declared value is *safe* — does not crash the level, does not make it
uncompletable — is the manifest author's responsibility, established by
playtesting. That is the same trust boundary the `[watch]` addresses already sit
on. The engine guarantees only that what it wrote is one of the values the
manifest declared.

---

## Alternative ROMs

**A hand-authored custom challenge.** A contributor builds a ROM hack in a level
editor — new geometry, new objects, a room that plays well because they
playtested it — and the manifest carries it as a **BPS patch against the base
ROM**, never as a second ROM file (ADR 0020).

```toml
[[rom]]
id = "base"                    # what a [[stage]] names
name = "Super Mario Bros. 3 (USA) (Rev 1)"
path = "assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes"
sha1 = "..."
header = "ines"                # base only
min_bytes = 40960              # base only

[[rom]]
id = "hack1"
name = "SMB3: Gamer Trials"    # what a player is shown, over [game].title
from = "base"                  # its presence is what makes this ROM derived
patch = "assets/patches/smb3-rom-hack1.bps"
sha1 = "..."                   # of the artifact, *after* applying

[[stage]]
id = "sun-storm"
rom = "hack1"                  # required on every stage, in every manifest
entry = { state = "assets/states/smb3_sun-storm.state" }
```

Six rules make that shape mean one thing, and each is a load error with its own
message:

- **Exactly one `[[rom]]` omits `from`, and it is the base.** `from` must name
  the base: a patch against a patch is refused, so a derivation stays two deep.
- **The patch format is BPS.** It records the source size, the target size, and
  CRC32s of source, target and the patch itself, so a patch applied to the wrong
  revision fails at apply time rather than producing a plausible wrong ROM. It
  also delta-encodes, so a hack is a small file.
- **`sha1` on a derived ROM is the artifact's, and it is required.** BPS's CRC32s
  catch an accident cheaply; a CRC32 is not what ADR 0013's cross-player
  comparison should rest on. Both, for different jobs.
- **`path` on a derived ROM is refused.** A field holding a path to a hacked ROM
  is the platform distributing a derivative of a copyrighted dump, which is the
  thing ADR 0013 generates client-side to avoid. A hack *is* its patch.
- **`header` and `min_bytes` belong to the base only.** They exist to recognise a
  good dump. A hack may legitimately be a different size, and its `sha1` is a
  stricter check than either; declaring one on a derived ROM is an error rather
  than something ignored.
- **A derived ROM is a base ROM in every other respect.** Its entry states are
  captured on it, its `[[challenge]].patch_slot` offsets are into it, and it may
  carry generated challenges of its own — a hack is exactly where value
  substitution becomes useful, rerolling enemies inside a level someone designed.

### What is *not* per-ROM

Everything else. There are no per-ROM `[watch]` addresses, no per-ROM rules, and
no per-ROM labels.

That is the price of admission, and it is the same fact as the reason for
co-locating in the first place: the telemetry block is the expensive half of a
manifest to author and the half that gets corrected months later, so two copies
drift. A second ROM may share a manifest **only if `[runner]`, `[watch]`,
`[[marker]]`, `[[rule]]` and `[attempt]` hold against it unchanged.** A hack that
moves an address, changes the memory route, or needs a different completion rule
is a different game and gets its own manifest.

Nothing checks this. It is established by playtesting, on the same trust boundary
`[watch]` addresses and patch-slot safety already sit on. The visible cost is
`[label.*]`: a hack that renames DKC's levels logs the base game's names. Labels
change no detector, no event and no verdict, so that is cosmetic — and a per-ROM
label table is the first thing a future ADR should add if it starts to hurt.
`[[rom]].name` is *not* in that category: it is what a player is shown for a
stage, over `[game].title`.

### Cartridge ROMs only

`patch` is refused on a manifest whose `console` is `dos`, and so is a
`patch_slot`. A BPS patch against a DOS bundle is meaningless — the bundle is a
zip and its hash depends on how it was zipped, which `docs/games/doom.md` already
records. A DOS alternative is a member-level overlay instead: a PWAD added, a
`dosbox.conf` replaced. That shape is blocked on the member hashing `asset-store`
owes (ADR 0018), and it is deferred rather than rejected.

### Rebinding does not cross ROMs

ADR 0019 rebinds an entry state onto a generated ROM by rewriting the ROM hash in
the state's header, and it is only safe because ADR 0015 keeps a generated ROM the
same length, mapper and geometry as its base. A hand-authored hack breaks all
three. So a state is rebound only onto an artifact the seeded patch engine derived
from **the ROM its own stage names** — never from the base onto a hack. The core
refusing that pairing is the check working, not a problem to route around.

That is a property of how content was produced rather than a preference, so it is
not a flag: stage prep turns rebinding on for exactly that case and for nothing
else. A hack's entry states are captured on the hack.

---

## Deliberately absent

**Arithmetic and expressions.** A rule reads one field's transition, optionally
guarded by one other field's current value; a `complete_when` is a conjunction
of a closed predicate set. There is no `+`, no `or`, no `not`, no
subexpression. Everything both shipped games needed fits; anything that does not
is a sign the game wants a real address found, not a cleverer manifest. The
moment this grows an expression parser it has become a scripting language living
in a data file, with none of a language's tooling.

**Per-stage rule overrides.** Rules are per game. If a stage needs a different
completion rule, that is evidence the general rule is wrong — DKC's used to need
a 230-entry table and a list of special-case rooms, and the reason it does not
any more is that `$7E0527` answers the question directly. A *challenge* that
needs different completion criteria is a different matter, and is what
`complete_when` is for.

**Scoring.** `[[challenge]]` says what completes a run. Nothing here says what a
completed run was *worth* — no points, no ranking, no comparison between
players, no verdict on a run that ended without completing. Manifests report
facts and declare criteria; `challenge-core` compares one against the other
(ADR 0017).

**Pointer indirection.** A rule names an address; it cannot follow one. This was
confirmed impossible *and* unnecessary for DOS: a DOS/4GW program's internal
pointers are linear addresses, not the emulated physical addresses the core's
descriptors expose, so following one would need indirection plus a base
translation. Every detector the slice needs lives in static data and is reachable
at a fixed address. Position-based criteria would not be, and proposing one means
reopening ADR 0016.

**Input remapping for keyboard games.** `controller = "keyboard"` passes the
host keyboard through unchanged. A DOS game maps its own keys, and a manifest
that rebound them would silently override a choice the player made inside the
game.
