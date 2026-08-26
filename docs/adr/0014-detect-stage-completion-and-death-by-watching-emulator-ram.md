# 0014. Detect stage completion and death by watching emulator RAM

Date: 2026-08-26
Status: Accepted

Completes the question ADR 0016 answered the *shape* of and ADR 0018 removed
the other half of.

## Context

The platform has to know two things about a run without the game's cooperation:
that a stage was finished, and that the player died. Nothing else in the design
works until it does — a challenge is criteria over facts (ADR 0017), and these
are the facts.

None of the three games in the slice will tell us. They are commercial binaries
from 1988, 1994 and 1993 with no hooks, no logs and no API. Whatever we learn,
we learn by looking at the machine while it runs.

This ADR was owed for a long time and kept shrinking. ADR 0016 settled how a
watch is *expressed* — declarative rules over declared addresses, in the
manifest, never code. ADR 0018 settled the half that was genuinely open: what an
adapter does for a game with no emulated address space. It answered by removing
the category — there is no external-process adapter, and a game qualifies only
if its machine is emulated. What is left is the decision itself, which is small
enough to state plainly and important enough to write down.

## Decision

**Stage completion and death are detected by watching the emulated machine's
memory, frame by frame, at addresses a manifest declares.**

Concretely, and all of it already implemented:

* An adapter reads declared addresses once per emulated frame, through
  whichever of the two libretro routes the manifest's `prefer` names —
  `RETRO_MEMORY_SYSTEM_RAM` or the `SET_MEMORY_MAPS` descriptors. A core
  offering neither is refused at startup; a core offering only the map is
  ordinary (the NES needs it for cartridge WRAM, and dosbox-pure has nothing
  else).
* A detector fires on an **edge** — a transition — never on a level. Holding a
  flag at a value is one event, not one per frame.
* An address with no value found yet is a first-class state. Every detector
  built on it stays quiet rather than guessing. DKC's damage detector spent a
  week honestly reporting nothing, and that was the correct behaviour.
* **Addresses are reused from RetroAchievements-style RAM maps** wherever one
  exists, and found with the harness's own search and probe tools where none
  does. Reuse is not a convenience; it is a correctness argument, because a
  published map has been exercised by thousands of players against the same
  binary.
* **Provenance is recorded per address**, in the manifest's `note`. "Two
  independent sources agree" and "a wiki said so" are different claims, and six
  months later the note is the only record of which one this was.
* **A death should be corroborated by two independent fields on the same
  frame** where the game offers them. Doom's is: `health` reaching 0 and
  `playerstate` entering `PST_DEAD`.

## Alternatives considered

**Watch the video output.** Read the screen — a "LEVEL COMPLETE" banner, a death
animation, a fade. Rejected as strictly harder and strictly less reliable: it
needs per-game template matching that breaks on palette swaps, resolution
changes and any overlay; it cannot distinguish a death from a cutscene showing
one; and it costs a full-frame comparison every frame where a RAM read costs a
pointer dereference. It also produces no useful *identity* — the screen does not
say which stage it is.

**Watch the audio.** A death jingle is distinctive and games are consistent
about them. Rejected for the same reasons plus one worse: audio is the thing
most likely to be muted, resampled, or turned off entirely by a player, and a
detector that stops working when someone mutes the game is not a detector.

**Instrument the game — patch a hook into the ROM.** Reliable, precise, and the
approach a speedrun timer would take. Rejected because it makes every stage
entry depend on a patched artifact, which collides with ADR 0002 (savestates
reach a stage) and ADR 0013 (patching is reserved for *generated* content, with
a derivation key). It would also mean per-game reverse engineering of a
different and much deeper kind than reading an address, which is invariant 5's
failure mode.

**Use the core's own achievement support.** libretro cores can expose
`RETRO_ENVIRONMENT_SET_SUPPORT_ACHIEVEMENTS`, and rcheevos already evaluates
conditions over RAM. Rejected as a dependency rather than as an idea: it would
put a third-party condition language between the manifest and the facts, and
that language is not ours to keep stable. We reuse rcheevos' *maps*, which are
data, and not its runtime. ADR 0016's rule vocabulary is deliberately smaller.

**Ask the core for a "level finished" signal.** There is no such thing in
libretro, and there is no reason there should be — it is a property of a game,
not of a machine.

## Consequences

**Adding a game means finding its addresses**, and that is the real cost of
this decision. It is per game, it is manual, and it is the one part of adding a
game that a manifest cannot make cheaper. The harness carries two tools for it —
a cheat-search over values that persist, a probe over one-shot events — and they
are the reason the cost is hours rather than days.

**A wrong address is invisible in exactly the way a working one is.** Both
produce silence. This is why provenance notes are mandatory, why a missing
address is a declared state rather than a zero, and why two independent fields
are wanted for a death: a mis-detected death is a lost run, and a survival
player has no recourse.

**Detectors are written against a binary, not against a machine.** Doom's
addresses live in `DOOM.EXE`'s static data, and a different build would need
every one found again. That is what "one manifest per base ROM hash" already
means, but it is sharper for a PC game, where a "version" is a much looser thing
than a cartridge revision.

**Games whose facts are not in memory cannot be supported.** If a game computes
its completion and keeps it only in a register or on the stack, this approach
has nothing to read. None of the three in the slice is like that. The escape
hatch is a real reverse-engineering effort on that title, and it would need its
own ADR.

**Position-based criteria stay out.** They would need pointer indirection,
which ADR 0016's rule language does not have by design, and which ADR 0018
confirmed is not merely unimplemented but wrong for DOS — a DOS/4GW program's
pointers are linear addresses, not the physical ones the descriptors expose.
Every fact the slice needs is in static data. Proposing a position criterion
means reopening 0016.

**This works identically for every emulated machine**, which is the payoff. The
NES, the SNES and a DOS PC differ in `[runner.libretro.memory]` and a controller
declaration, and in nothing else. There is one telemetry vocabulary, one loader,
one set of contributor docs.
