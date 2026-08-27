# 0019. Rebind an entry state to a generated ROM rather than regenerating it

Date: 2026-08-27
Status: Accepted

## Context

ADR 0013's model for generated content is that the **ROM is derived per seed**
while the **entry state is a shipped asset**. Testing that against a real patched
ROM found the two mutually exclusive on the NES core:

    error: retro_unserialize refused 35840 bytes

Mesen binds a savestate to the exact ROM bytes. Change one byte of a 393232-byte
ROM and every existing state for it is refused. Since every seed changes ROM
bytes, every seed invalidated every entry state, and `content-gen` would have
shipped correct output that could not be entered.

The binding turns out to be a **pairing check in a plaintext header**, not
anything the emulated machine depends on:

    00: "MST"                 magic
    03: 09 09 00 00           emulator version 0x000909 = Mesen 0.9.9
    07: 0d 00 00 00           state format version 13
    0e: 40 ASCII hex chars    uppercase SHA-1 of the ROM *file*, header included
    36: u32 + bytes           length-prefixed ROM filename

`SaveStateManager::LoadState` compares those 40 bytes against the loaded ROM's
hash, falls back to trying to load the ROM the header names, and refuses if
neither works. Nothing downstream of the check consumes ROM bytes.

## Decision

**Stage prep rebinds the entry state to the artifact it is about to run on.**
Compute the SHA-1 of the generated ROM, overwrite the 40-byte hash field, hand
the rewritten bytes to the core. The rebind happens **in memory, per entry**: the
state on disk stays bound to the base ROM and remains the canonical shipped
asset, exactly as ADR 0013 assumed.

Three things this decision fixes in place:

- **Entry states for generated challenges are captured on the base ROM.** A
  patch is only ever visible for objects that have not yet spawned at the entry
  point — nothing in ADR 0013/0015 said so, and it constrains every generated
  challenge. Capture on the base ROM and the constraint is enforced by
  construction.
- **The rebind is core-specific knowledge and stays in `session-libretro`.**
  No `savestate`, no `sha1`, no Mesen header offset reaches `session-api`,
  `challenge-core`, or the manifest schema (invariants 1 and 3). The
  developer-facing spelling is `--rebind-state` on the harness.
- **It is opt-in and it validates.** It refuses anything that is not an `MST`
  format-13 header, refuses a hash field that is not 40 hex characters, and is a
  no-op when the state is already bound to the ROM in hand. A silent rebind
  would mask a genuinely mismatched state/ROM pairing, which is the thing the
  original check exists to catch.

### Measured, 2026-08-27

| ROM | State | Result |
|---|---|---|
| patched A | base state, untouched | refused |
| patched A | base state, 40 bytes rewritten | loads |
| patched B (different roll) | base state, rewritten to B's hash | loads |

Base ROM and patched-A-plus-rebound-state ran 600 frames from
`smb3_1-1.state` with identical watched RAM every frame, across a death and a
`return_to_map`, and with 512 bytes of cartridge WRAM (`$6000-$60FF`,
`$7D00-$7DFF`, non-zero) identical throughout — so the state's WRAM block really
is restored onto the patched board, which is what the `$7D9C` card detector
depends on.

## Alternatives considered

**A different NES core.** fceumm or nestopia may not bind states this way. It was
the cheapest test and it is now unnecessary, but the reason not to reach for it
stands on its own: `docs/games/smb3.md` records that Mesen is the only NES core
confirmed to publish the memory map the card detector needs. Trading
`stage_cleared` for portable states is a bad trade. Kept as a fallback if a
future core is wanted for an unrelated reason.

**Cold-boot `named` entries for generated challenges.** Honest and expensive:
SMB3 has no warp mechanism, so a cold boot is the title screen. A death in a time
trial would cost a boot plus a full traversal to the level, and the entry would
not be a stage at all. It also makes generated challenges structurally unequal to
vanilla ones, which enter at a state.

**Generate the state per seed, by deterministic input replay.** ADR 0013 does put
generators behind "whatever prepares a stage", so there is room. Rejected as far
larger than the patch engine it would sit beside: it needs a recorded input film
per stage, replayed deterministically, and every generator author would owe one.

**Patch the core.** Removing the check inside `mesen_libretro.dll` would mean
shipping and maintaining a modified core. Rejected outright — the check is
correct, and we would rather satisfy it than delete it.

**Make the generated ROM hash the base ROM's.** Not possible, and listed only so
nobody spends an afternoon on it.

## Consequences

**ADR 0013's entry-point model survives intact.** States stay shipped assets,
ROMs stay derived, nothing generated crosses the wire.

**ADR 0015's value-substitution restriction becomes load-bearing.** Same file
length, same mapper, same geometry is what makes the transplant safe. It was
adopted as a scoping preference; it is now a correctness requirement. A
structural patch would break a rebound state *silently* rather than loudly,
which is worse than what we started with. If structural patching is ever
proposed again, this is a reason to refuse it.

**A generated ROM runs on a slightly different board than the base ROM.** Mesen's
game database is keyed on the ROM, so an unknown hash falls back to header
settings. Measured: 8192 bytes of save RAM appear that the base ROM withholds,
and a state captured *on* a generated ROM is 52224 bytes against 35840. Loading a
35840-byte base state onto that board works — the absent block keeps its
power-on contents — and nothing observable diverged over 600 frames. We live
with it. If it ever needs removing, the lever is a NES 2.0 header on the
generated ROM declaring the RAM layout exactly; the generated ROM's hash is ours
to choose anyway, since we rewrite the state to match it.

**A state captured on a generated ROM is not portable back to the base ROM.** It
carries the generated board. Capture on the base ROM, always.

**"Visible for objects that have not yet spawned" is not quite the whole
constraint, and the remainder is unmeasured.** A state carries RAM, and RAM can
hold *derived* copies of ROM data. SMB3 builds `Level_ObjIdxStartByScreen` when
a level loads, by walking that level's object list; a state captured inside the
level therefore carries an index built from the **base** list, and the generated
list is read against it. For the Sun Storm slot table it should not matter — the
table records where each screen *starts*, the scan runs forward from there until
the screen changes, and every rolled column stays on the screen its slot already
indexed — but that is reasoning, not a measurement, and it is exactly what the
first capture session tests. If it turns out to matter, the answer is to capture
the entry state on the world map, before the level loads, and pay the extra
frames. Either way a generator must not move an object to a screen its slot was
not already indexed under. See `docs/games/smb3.md`, finding 3.

**The trick is per-core, and each core owes its own answer.** `content-gen`'s
first target is SMB3, so this unblocks the work in front of us and nothing more.
The other two slice games have not been tested, and their headers say only that
they are different problems: a snes9x state opens `#!s9xsnp:0014` and its first
chunk is `NAM:000008:Removed`, so the SNES core appears to record no ROM identity
at all, and a dosbox-pure state opens with a three-byte binary magic and nothing
hash-shaped. Neither has been run against a modified ROM. `rebind` refuses both
by name rather than guessing at them, and no generated content ships for another
game until that game's core has been measured the same way.

**Unchanged by this ADR:** whether a rolled quantity may reach `complete_when`.
`complete_when` is static TOML with no arithmetic and nothing that can reference
the seed, so a generator cannot roll a count that a criterion then refers to.
That is still the first real boundary a generator hits and it is still unsettled;
see `docs/games/smb3.md` for the two challenges that ran into it.
