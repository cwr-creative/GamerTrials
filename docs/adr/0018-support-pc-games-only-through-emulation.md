# 0018. Support PC games only through emulation, and retire the external-process adapter

Date: 2026-08-26
Status: Accepted

Supersedes 0008. Closes the half of 0014 that was still owed.

## Context

The structure table has carried `session-external` since the beginning — "Doom,
and other classic PC games in the future. Process launch + stats-file harvest."
Unwritten ADR 0008 accepted decomp/recomp PC ports as external-process adapters,
and unwritten ADR 0014 was left owing an answer to the question 0016 could not
reach: *what does an external-process adapter do instead*, since a native Doom
has no addressable core RAM.

Phase 2 made adding a libretro game a manifest (0016) and Phase 2's second half
made adding a challenge a manifest entry (0017). Doom was the one game in the
slice that would fit neither. The question that forced this decision was how to
support many classic PC games with one adapter without falling into a
per-game adapter, which is invariant 5's failure mode.

The obvious framing — *if a game does not run under libretro, this project does
not support it* — is a packaging rule, and it is not the property the
architecture actually depends on. `prboom-libretro` and `dsda-doom-libretro`
satisfy it and still cannot satisfy a single line of `[watch]`: they are native
C recompiled behind the libretro API, with no emulated address space, no usable
`RETRO_MEMORY_*`, and savestates that are the game's own save format. The
property that matters is whether **the machine is emulated**.

## Decision

**The platform runs games whose machine it emulates.** libretro is how a core is
delivered (ADR 0001); emulation is the criterion.

`session-external` is not built. It leaves the structure table, and no adapter
in this project ever launches a process and harvests a stats file.

**DOS games enter through `session-libretro`, under `dosbox-pure`**, as a
manifest and a content bundle. A game qualifies when its core offers three
things, all checkable at contribution time with the existing harness:

1. deterministic frame stepping,
2. an address space stable enough to write `[watch]` entries against,
3. an entry point — a machine state, or a location the game can be told to
   start at.

**"Classic PC games" narrows to "DOS games."** Modern source ports, their mods,
and anything Windows-era are out of scope, not deferred.

A spike (`crates/dos-spike`) measured this against Doom shareware 1.9 before the
decision was taken, and the findings are what it rests on:

* `retro_get_memory_data` answers nothing; the core publishes three
  `SET_MEMORY_MAPS` descriptors. `prefer = "memory_map"` — the route the NES
  manifest already uses — works unchanged, disjoint regions included.
* Addresses are byte-identical across cold boots and unchanged by cycles,
  memory size and CPU core. They are written against `DOOM.EXE`, which is what
  "one manifest per base ROM" already means.
* `stage_cleared`, `died`, `damaged` and stage identity are all edges over
  declared addresses. Death is corroborated by two independent fields on the
  same frame; completion was confirmed by finishing E1M1 with the watcher
  attached.
* Both `EntryPoint` kinds work: `named` as a command line in the bundle's own
  `dosbox.conf`, and `state` as a `retro_serialize` savestate that restores into
  a fresh process.

Four schema additions follow, and are owed before a Doom manifest can load:
`Width::U32`, `console = "dos"`, a keyboard `pad_layout`, and a
`[runner.libretro.options]` table that pins core options. Nothing in
`[[rule]]`, `[[challenge]]`, `SessionEvent` or the memory routing changes.

The options table is the only one of the four that is load-bearing rather than
tidy. `dosbox_pure_cycles` defaults to `auto`, which is defined as tracking host
performance. Addresses do not move with it, but **frame counts do** — the same
demo damage lands on frame 501 at 26800 cycles and frame 452 on `auto` — so the
setting is part of the game's identity and belongs beside the ROM hash rather
than in a user preference.

## Alternatives considered

**Build `session-external` as planned** — launch the process, harvest a stats
file. Rejected for three independent reasons, any one of which is sufficient.

*Invariant 4, as a fairness bug rather than a pedantry one.* A native process
has no emulated frame count; its frames are wall-clock frames. ADR 0008's note
about locking the framerate treats the symptom: on a time trial a host stutter
costs a native-process player real frames and costs an emulated player nothing.

*The manifest architecture forks.* A stats file is post-hoc and level-granular.
`damaged`, `enemy_defeated` and `item` either vanish or arrive with invented
frame numbers, and `[[marker]]`, `[[rule]]` and `[attempt]` have nothing to
read. It would mean a second telemetry vocabulary beside the one 0016 just
settled — two schemas, two loaders, two sets of contributor docs, for one game.

*Spectating dies, and it is a core requirement.* ADR 0003 requires every adapter
to be `Presentable` — to hand over a framebuffer it owns. An external process
has none to give, leaving only whole-window screen capture, which 0003 already
rejected as strictly worse.

**Make "runs under libretro" the criterion.** Attractive, and nearly right.
Rejected because it admits ports: `prboom-libretro` passes the test and fails
every requirement the test exists to protect. libretro is necessary, not
sufficient.

**Patch a native source port to expose a framebuffer and telemetry** — ADR
0008's decomp/recomp path taken seriously. Rejected on cost and on invariant 5:
it is a fork to maintain per title, and per-game engineering to add a game is
exactly the failure this project keeps refusing. It remains the escape hatch if
some title is ever important enough to justify it, and it would need its own
ADR.

**Keep Doom native and drop it from the vertical slice.** The fallback if the
spike had failed. Unnecessary: DOS Doom runs, and every detector the slice needs
was measured.

**Let cycles be a user setting.** Rejected on measurement. Two players on
different cycles settings produce different frame counts for identical play,
which makes their scores incomparable — and it would pass a single-machine test
before diverging in a race.

## Consequences

**There is no external-process adapter, and no `[runner.external]`.** The
structure table loses a crate that was never written. `session-api`'s vocabulary
is unaffected: it never named a process or a stats file, which is invariant 3
having done its job.

**ADR 0014 can now be written**, as RAM-watching and nothing else. The half it
was owing is answered by there being no external adapter to describe.

**A pinned core-options table becomes part of a game's identity.** That is a new
kind of thing in a manifest — not an address, not a hash, but a machine
configuration without which two players are not playing the same game. It is
also a precedent worth being careful with: it must stay a short, reviewed list,
not a place to park preferences.

**Content identity gets slightly harder.** A DOS game is a zip that the core
mounts as C:, so it is still one file with one hash — but the archive's hash
depends on how it was zipped, so `asset-store` will want to hash the members
rather than the container.

**We live with 320x200 software-rendered Doom**, no mouselook fidelity, no
mods, no source-port features. For a library whose other two games are 256x240,
that is arguably the right aesthetic rather than a concession.

**Some games are permanently out**, and the README should say so plainly rather
than implying a broader "PC" scope that will not be delivered.

**Pointer indirection is confirmed impossible and confirmed unnecessary.** A
DOS/4GW program's internal pointers are linear addresses, not the emulated
physical addresses the descriptors expose; following one would need indirection
plus a base translation, and the rule language has neither by design (0016).
Every detector the slice needs lives in static data. Position-based criteria
would not, and proposing one means reopening 0016.

**Head-to-head is unaffected.** ADR 0004 delegates it to per-game solutions, and
nothing here assumes it exists.
