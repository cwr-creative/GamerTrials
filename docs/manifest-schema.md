# Game manifest schema

**Status: proposed, `schema_version = 1`. Implemented by `session-libretro`;
two manifests in `manifests/` are the worked examples.**

A manifest is everything the platform knows about one game. Adding a libretro
game is adding one of these files and nothing else — no Rust, no rebuild
(invariant 5). The loader (`crates/session-libretro/src/manifest.rs`) is the
authority on what parses; this document is the authority on what the fields
*mean*.

One manifest per **base ROM**. A game with two revisions whose addresses differ
is two manifests. Generated artifacts get no manifest of their own — they are
identified by a derivation key (ADR 0013).

## Contents

| Table | What it settles |
|---|---|
| [`[game]`](#game) | identity, console family, framerate |
| [`[rom]`](#rom) | which ROM this manifest is written against |
| [`[runner]`](#runner) | which adapter runs it, and how it reaches memory |
| [`[watch.<name>]`](#watchname) | the addresses to read, and where each came from |
| [`[label.<field>]`](#labelfield) | what a value *means*, for the log |
| [`[display]`](#display) | which fields to print when one of them moves |
| [`[[marker]]`](#marker) | a transition that *times* an event announced late |
| [`[[rule]]`](#rule) | which transition means what |
| [`[attempt]`](#attempt) | what re-opens scoring after an ending |
| [`[[stage]]`](#stage) | entry points, and which challenge types each can carry |
| [`[[challenge]]`](#challenge-patch-slots--not-implemented) | generated-content patch slots — documented, **not implemented** |

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

`console` is a closed set reviewed once per console family, never per game. It
is also what ADR 0015's checksum fixup will key off (SNES has an internal header
checksum; NES has none).

## `[rom]`

```toml
[rom]
name = "Donkey Kong Country (U) (V1.0) [!]"
path = "assets/ROMs/snes/Donkey Kong Country (U) (V1.0) [!].smc"
sha1 = "..."                   # optional today; `asset-store` will require it
header = "copier-optional"     # "none" | "ines" | "copier-optional"
min_bytes = 32768              # optional size floor
```

`header` is per console family, and it is all that can be checked without a
hash: the NES has a magic number, the SNES has none at all — its header is
in-band, at a position that depends on the mapper — so the SNES gets a size floor
and a note when the size implies a 512-byte copier header, which shifts every
ROM offset and makes an address table look wrong for no visible reason.

## `[runner]`

```toml
[runner]
kind = "libretro"              # which adapter; "external" is Doom's
```

Everything an adapter needs that another adapter would not understand lives
under `[runner.<kind>]`. That is what keeps adapter vocabulary out of the shared
part of the schema (invariant 3) while still letting a manifest say `core_dir`.

### `[runner.libretro]`

```toml
[runner.libretro]
core_dir = "cores/snes"        # must hold exactly one *_libretro.dll
pad_layout = "snes"            # "nes" | "snes"
```

`pad_layout` decides only how a modern controller's face diamond maps onto the
console's pad. A two-button console takes South *or* East as A so both habits
work; a four-button one cannot, because every position is already taken.

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

## `[watch.<name>]`

```toml
[watch.lives]
at = 0x0575                    # offset or full address; omit if unknown
width = "u16"                  # optional, defaults to memory.default_width
note = "RA notes: lives. $0577, which every older map quotes, is the display copy"
```

The `name` is what every rule, marker, guard and `--addr` override refers to.

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
| `note` | nothing — logged only | no |

An emit names a **fact**, never a verdict. "The player died" is something an
adapter can observe; "the run failed" depends on the challenge — an endurance
run and a time trial answer it differently — and belongs to `challenge-core`.
There is deliberately no `stage_failed`.

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

```toml
[[stage]]
id = "mine-cart-carnage"
name = "Mine Cart Carnage"
entry = { state = "assets/states/dkc_mine-cart-carnage.state" }
challenges = ["endurance"]
endurance = { survive_seconds = 45 }   # optional
note = "forced-scrolling: everyone finishes in the same number of frames"
```

`entry` takes exactly one of:

* `state = "<path>"` — a captured machine state to restore. An emulator
  savestate is one form of this.
* `named = "E1M1"` — a location the game itself can be told to start at. Doom's
  `-warp`, which is why the entry point is a choice and not a path (ADR 0002).

`challenges` is the closed set `time_trial` / `endurance`, and listing them is a
real constraint rather than metadata: a forced-scrolling level cannot be a time
trial, because everyone finishes it in the same number of frames.

`endurance.survive_seconds` is **converted to frames at load**, at
`game.framerate`. Seconds never reach a `SessionEvent` or a wire message.

Stage identity always comes from the entry point that put the player there,
never from RAM. SMB3 has no "current level number" at all — a level is
identified by the map tile it was entered from.

---

## `[[challenge]]` — patch slots — *not implemented*

ADR 0015 settled the shape of a generated custom challenge: declarative
value-substitution, and nothing else. The schema below is what a manifest will
carry. **No crate reads it today** — `content-gen` will — and the loader
*refuses* a manifest containing it rather than accepting a table that does
nothing. ADR 0015 explicitly rejected shipping an unbuilt opt-in path, and an
accepted-but-inert table is one.

```toml
[[challenge]]
id = "enemy-reroll"
name = "Reptile Roulette"
stage = "reptile-rumble"
generator_version = 1          # bumped, never edited in place (ADR 0013)

# Roll one value from an explicit pool of legal ones.
[[challenge.patch_slot]]
offset = 0x1A2B3               # into the base ROM, after any header
width = "u8"
values = { pool = [0x01, 0x02, 0x05] }

# Or from a numeric range.
[[challenge.patch_slot]]
offset = 0x1A2B7
width = "u16"
endian = "little"
values = { range = { min = 1, max = 8 } }

# Or as one member of a group consumed by permutation: which enemy occupies
# which of an existing set of spawn slots, rather than each slot rolled
# independently.
[[challenge.patch_slot]]
offset = 0x1A2BB
width = "u8"
values = { group = "enemy-shuffle" }

[challenge.group.enemy-shuffle]
pool = [0x01, 0x02, 0x03, 0x04]
mode = "permutation"
```

The engine writes one declared-legal value per slot into an otherwise-untouched
copy of the base ROM, deterministically from the seed, then applies the
console-family checksum fixup. It creates no slots and relocates nothing:
structural or geometric generation is out of scope permanently, not pending
(ADR 0015).

Whether a declared value is *safe* — does not crash the level, does not make it
uncompletable — is the manifest author's responsibility, established by
playtesting. That is the same trust boundary the `[watch]` addresses already sit
on.

---

## Deliberately absent

**Arithmetic and expressions.** A rule reads one field's transition, optionally
guarded by one other field's current value. There is no `+`, no `and`, no
subexpression. Everything both shipped games needed fits; anything that does not
is a sign the game wants a real address found, not a cleverer manifest. The
moment this grows an expression parser it has become a scripting language living
in a data file, with none of a language's tooling.

**Per-stage rule overrides.** Rules are per game. If a stage needs a different
completion rule, that is evidence the general rule is wrong — DKC's used to need
a 230-entry table and a list of special-case rooms, and the reason it does not
any more is that `$7E0527` answers the question directly.

**Scoring.** Nothing here says what a run was worth. Manifests report facts;
`challenge-core` decides what they mean.
