# 0020. Carry alternative ROMs as patches against the base, and let a stage name one

Date: 2026-08-27
Status: Accepted

## Context

ADR 0015 scopes generated content to declarative value substitution and rules out
structural generation permanently. That is the right boundary for a *seeded
generator* — inserting objects or growing a table needs per-game structural
knowledge, and a machine doing it unsupervised produces uncompletable stages.

It is the wrong boundary for a *human*. A contributor authoring a custom
challenge wants a level editor: new geometry, new objects, a hand-built room that
plays well because they playtested it. That is a ROM hack, and it is how this
scene has produced custom content for thirty years. In practice `content-gen` is
then not the author of a challenge at all — it rerolls enemy type or position
*within* an already-designed one, which is exactly the class ADR 0015 does
support.

So the platform needs a second piece of content per game, and the manifest is
where content is declared. Three forces shape how.

**A hack is a derivative of a copyrighted ROM.** ADR 0013 chose client-local
generation specifically so that "shipping a patched ROM between machines" never
happens, and `asset-store` verifies each player owns a matching base dump. A
manifest field holding a path to a hacked ROM reintroduces the exact problem 0013
was built to avoid, one layer up.

**The reason to co-locate a second ROM is telemetry reuse.** `[watch]`,
`[[marker]]`, `[[rule]]`, `[attempt]` and `[label.*]` are the expensive part of a
manifest to author and the part that gets corrected months later. A hack of SMB3
reads the same addresses as SMB3. Giving it its own manifest means copying all of
that and watching the two copies drift — the duplication ADR 0016 was written to
remove.

**An entry state is bound to a specific ROM.** ADR 0019 measured this: Mesen
refuses a state whose recorded ROM hash does not match the loaded one, and 0019's
rebind is safe only because ADR 0015 guarantees the derived ROM has the same
length, mapper and geometry. A hand-authored hack breaks every one of those
guarantees.

## Decision

A manifest declares **one or more ROMs**. Exactly one is the base; every other is
declared as a **derivation from it** — a patch file plus the SHA-1 of the artifact
the patch produces. **Each `[[stage]]` names which ROM it runs on**, alongside the
entry point it already names.

```toml
[[rom]]
id = "base"
name = "Super Mario Bros. 3 (USA) (Rev 1)"
path = "assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes"
sha1 = "..."
header = "ines"

[[rom]]
id = "sun-storm"
name = "SMB3: Sun Storm"
from = "base"
patch = "assets/patches/smb3-sun-storm.bps"
sha1 = "..."                   # of the artifact, after applying

[[stage]]
id = "sun-storm-1"
rom = "sun-storm"
entry = { state = "assets/states/smb3_sun-storm-1.state" }
```

Six rules make that shape mean one thing.

- **Exactly one `[[rom]]` omits `from`, and it is the base.** `from` must name the
  base; a patch against a patch is refused. One level keeps a derivation two deep,
  and hack-on-hack is not something anyone has asked for.
- **The patch format is BPS.** It carries the source size, the target size, and
  CRC32s of source, target and patch itself, so applying it to the wrong dump
  fails at apply time rather than producing a plausible wrong ROM. It also
  delta-encodes, so a hack is a small file.
- **`sha1` on a derived ROM is the artifact's, and it is the identity.** BPS's
  CRC32s catch an accident cheaply; a CRC32 is not what ADR 0013's cross-player
  comparison should rest on. Both, for different jobs.
- **`header` and `min_bytes` belong to the base only.** They exist to recognise a
  good dump. A hack may legitimately be a different size, and its `sha1` is a
  stricter check than either. Declaring either on a derived ROM is an error, not
  something ignored.
- **A derived ROM is a base ROM in every other respect.** Its entry states are
  captured on it. Its `[[challenge]].patch_slot` offsets are into it. It may carry
  generated challenges of its own.
- **`[[stage]].rom` is required, always**, including in a manifest that declares
  one ROM. Nothing about a stage is inferred, for the same reason nothing about a
  challenge is (ADR 0017).

**Co-location is conditional on telemetry identity.** A second ROM may share a
manifest only if `[runner]`, `[watch]`, `[[marker]]`, `[[rule]]` and `[attempt]`
hold against it unchanged. If a hack moves an address, changes the memory route,
or needs a different completion rule, it is a different game and earns its own
manifest. That condition is the same fact as the reason for co-locating — shared
telemetry — so the schema gains no per-ROM overrides of anything. Nothing checks
it; it is the author's to establish by playtesting, on the same trust boundary
`[watch]` addresses and patch-slot safety already sit on.

**Rebinding does not cross `[[rom]]` entries.** ADR 0019's rebind stays what it
was: a state may be rebound only onto an artifact the seeded patch engine derived
from the ROM the stage names. A state captured on the base is never rebound onto a
hack. 0019 says a structural patch would break a rebound state *silently*, and a
hand-authored hack is structural by definition; the loud refusal from the core is
the correct outcome and must be left intact.

**This does not reopen ADR 0015.** 0015 constrains what a generator may do with a
seed, and its reasons — per-game structural knowledge, uncompletable output — are
reasons about machines generating content unsupervised. A human authored the hack
in a level editor and playtested it before declaring it. Value substitution remains
the only thing `content-gen` does from a seed, on a base or a derived ROM alike.

**Cartridge ROMs only, for now.** A BPS patch against a DOS bundle is meaningless:
the bundle is a zip, and `docs/games/doom.md` already records that its hash depends
on how it was zipped. A DOS alternative is a member-level overlay — a PWAD added, a
`dosbox.conf` replaced — and that shape is blocked on the member hashing
`asset-store` already owes (ADR 0018). `patch` on a manifest whose console is `dos`
is refused with that reason, rather than silently accepted.

## Alternatives considered

**A separate manifest per hack**, as `docs/manifest-schema.md` currently requires.
Rejected: it duplicates the telemetry block, which is the expensive and drift-prone
half of a manifest, and two copies of DKC's rules will not stay in agreement. Still
the right answer for a hack that *changes* the telemetry — see the co-location
condition.

**A path to the hacked ROM** — the literal first sketch of this decision. Rejected:
it distributes a derivative of a copyrighted ROM, contradicting the stated reason
ADR 0013 generates client-side; and `asset-store` cannot meaningfully verify a
player owns something the platform hands them.

**IPS rather than BPS.** Rejected: IPS carries no source or target verification at
all, so a patch applied to the wrong revision corrupts the ROM silently, and its
three-byte offsets cannot address past 16 MB. BPS costs nothing extra and refuses
the wrong dump.

**Express a hack as ADR 0015 patch slots with one legal value each.** Rejected:
value substitution cannot insert an object or relocate a table, which is most of
what a hack is; and a hack is thousands of changed bytes, which is not a slot table.

**Exactly one alternative ROM slot**, as first sketched. Rejected: a stage must name
an id either way, so a cap of one buys no schema simplicity, and it is hit the
moment a contributor wants two hacks of one game.

**`rom` on `[[challenge]]` instead of `[[stage]]`.** Rejected: several challenges
share one stage by design, and they must not be able to disagree about which machine
they are on. ADR 0019 also established that an entry state is bound to one ROM's
bytes, so the ROM and the entry point belong to the same table.

## Consequences

**ADR 0013's derivation key is unchanged.** It is
`(base_rom_hash, generator_id, generator_version, seed)` — keyed on a hash, not on
"the manifest's ROM". A generated challenge on a hacked base widens the domain of
its first element and changes nothing else. The cross-player hash compare and the
abort-on-mismatch rule work as written.

**`schema_version` becomes 3.** `[rom]` becomes `[[rom]]` with an `id`, and
`[[stage]]` gains a required `rom`. Twelve stages across three manifests migrate,
and each gains one line.

**`StageSpec` grows the content it runs on**, beside the entry point it already
carries. `AssetKey` is already documented as naming "an entry state, a ROM, a
generated artifact", so this is not adapter vocabulary crossing the boundary
(invariant 3) — it is content identity, which the type was built for. It also
retires ADR 0013's awkward consequence that "`EntryPoint` must be able to name a
derived artifact": the stage names the content, the entry point names the position
in it. Two things, two fields, and generated content gets the same seam.

**A ROM switch only happens at a challenge boundary.** A time-trial death re-enters
the same stage on the same ROM, so a runner never changes content mid-run. Since
`challenge-core` does not exist yet, the runner's lifetime is still open, and this
settles it as per-(game, ROM) rather than per-game.

**`content-gen` gains BPS application**, next to the seeded patch engine and the
per-console checksum table it already owns. It is format-generic, has no per-game
knowledge and no emulator dependency, so invariant 5 survives on the same argument
ADR 0015 makes: the engine is written once, and adding a hack is a data change.

**`asset-store` gains apply-and-cache.** Verify the player's base dump, apply the
patch, check the result against the declared `sha1`, cache under
`(base_sha1, patch_sha1)`.

**Wrong labels are the accepted cost of having no per-ROM overrides.** A hack that
renames DKC's levels will log the base game's names. `[label.*]` changes no detector,
no event and no verdict, so this is cosmetic — and a per-ROM label table is the first
thing a future ADR should add if it becomes annoying. Nothing else in the telemetry
block is allowed to differ at all.

**A hack owes a full playtest, twice over.** Its stages need entry states captured on
it, and its telemetry needs confirming against it — the rules were established on the
base game and are only assumed to hold.

**Display drifts from `[game].title`.** A challenge on a hacked ROM still reports
`game = "smb3"`, and `[[rom]].name` is what a player should be shown. Whatever
presents a challenge picks the ROM's name over the game's.

**The harness's `--rom` becomes ambiguous** and needs to say which ROM it overrides.

**Distributing a patch rather than a ROM is standard practice, not a legal opinion.**
A BPS patch encodes differences, and it is materially better than shipping the bytes.
It is not a guarantee, and nothing here is advice.
