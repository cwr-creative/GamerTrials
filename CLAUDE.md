# CLAUDE.md

Multiplayer retro-game challenge platform. Players race through stages drawn from
multiple classic games simultaneously; live telemetry and spectating.

Solo developer. Windows targets. Vertical slice = SMB3 (NES), DKC (SNES), Doom (DOS).

## Challenges

A **match** — what ADR 0009 calls a session; the `session-*` crates mean one
running stage — runs in one of three modes — time trial, survival, or both —
and deals a randomised series of challenges from the library. Three words,
used precisely (ADR 0017):

- **stage** — a place, and the entry point that reaches it.
- **challenge** — what a player is dealt: one stage, one kind, and the criteria
  that complete it. A stage may carry several.
- **attempt** — one entry-to-terminal-event span. A **run** is a player's whole
  go at one challenge.

The two kinds differ in what death costs, not in what completes them:

- **Time trial** — on death, re-enter at the stage's entry point with the clock
  still running. Scored on total frames across every attempt.
- **Survival** — on death the run is over and the player spectates. Scored on
  whether the criteria were met at all. "Without dying" is therefore not a
  criterion; it is what the kind means.

Whoever finishes first spectates the other until they finish too.

**Contributors define every challenge explicitly**, in the manifest. Nothing is
inferred from a stage: 100 levels as time trials is 100 `[[challenge]]` entries,
and 200 if they should also be survival runs. Completion criteria are a closed
predicate set over facts the same manifest emits — never code, never arithmetic.

Every run is bounded: a manifest may hint a `limit_seconds`, the lobby supplies
a default otherwise, and a run that does not complete costs the full limit in
any total (ADR 0026).

Either kind may run on **vanilla** content (a stage as it originally shipped), on
a **hack** (content a contributor authored in a level editor, carried as a BPS
patch against the base ROM and named by the stage — ADR 0020), or on **generated**
content (a ROM patched per attempt from a seed — ADR 0013/0015). All three are
built, run and playtested to their criteria, but the seed a generated
challenge needs still comes from a harness flag rather than from a session. All
three feed the same sequencing and scoring, and the last two compose — a
generated challenge sitting on a hack is the expected shape, not an exotic one: a
human designs the level, the generator rerolls enemies inside it.

Head-to-head, where players share one running game, is last by choice: it needs
synchronised netcode rather than one-way telemetry. Nothing in the slice may
assume it exists.

## Invariants

These are settled. Do not violate them without writing a superseding ADR.

1. **`challenge-core` is pure.** No I/O, no emulator, no network, no UI. It must
   compile with zero dependencies on any adapter crate. If you are adding
   `use libretro` to core, stop.
2. **Interfaces are owned by the consumer.** `StageRunner` and its types live in
   `session-api`; adapters depend on it, never the reverse.
3. **No adapter vocabulary crosses the boundary.** No `savestate_path`, no
   `retro_run`, no `wad_file` in shared types. If a name only makes sense for one
   adapter, it is wrong.
4. **Timing is measured in emulated frames, never wall-clock.** `SessionEvent`
   carries `frame: u64` by design.
5. **Adding a game is a data change, not a code change.** New game = new manifest.
   So is adding a *challenge* to a game (ADR 0017), and so is adding a *hack* — a
   patch, a `[[rom]]` entry, and the stages that name it (ADR 0020). If any of
   those requires touching Rust, the abstraction has failed — fix the abstraction.
   So is adding a *generated challenge*: a `patch_slot` table and a
   `generator_version`, rolled by one shared engine (ADR 0015 narrowing ADR
   0013). What is code is a new *console family* — a header size and a checksum
   rule in `content-gen`, reviewed once per family, never per game. Whatever a
   generator produces is still data, and `challenge-core` never learns
   generators exist.
6. **Every phase ends in something runnable.** No big-bang integration.
7. **Every game runs on an emulated machine** (ADR 0018). Not "runs under
   libretro" — that is the delivery mechanism, and native ports behind a
   libretro API satisfy it while satisfying nothing else. A core qualifies when
   it gives deterministic frame stepping, an addressable memory space stable
   enough to write `[watch]` against, and an entry point. No adapter launches a
   process or reads a stats file; there is no `session-external`. Doom is a DOS
   game under dosbox-pure, and "classic PC games" means "DOS games".

## Structure

| Crate | Role |
|---|---|
| `challenge-core` | Stage sequencing, scoring, rules. Pure. |
| `challenge-library` | The challenge half of a manifest: `[game]`, stage names, every `[[challenge]]`, and the emit vocabulary its criteria are checked against. What the server deals and judges from (ADR 0009/0024). No emulator, no `content-gen`, no I/O; `session-libretro` parses through it. |
| `session-api` | `StageRunner`, `Presentable`, shared types. |
| `session-libretro` | SMB3, DKC, Doom, and every later game. Software cores only for now. One crate, no per-game code: a game is a manifest (ADR 0016). DOS games run here too, under dosbox-pure (ADR 0018) — same crate, same schema, same events. |
| `session-fake` | Scripted telemetry. Used by all integration tests. |
| `asset-store` | ROM hash verification, content-addressed bundles, and the cache of artifacts derived from them. Today: hashes a player's dump against the manifest's `sha1` and says what is missing. |
| `protocol` | The wire between client and server (ADR 0024): versioned messages with types of their own, never `session-api`'s or `challenge-core`'s on the wire. Conversions live here. |
| `app-client` | Tauri shell, overlay, wiring (ADR 0027). Menu and lobby in an ordinary window; a native game window under a transparent overlay during a match. Prepares and runs stages and streams their events; judges only provisionally, to know whether to re-enter or spectate. Plays the server's half in-process for solo play (ADR 0009). |
| `server` | Rust (ADR 0024). Lobby, clock sync, telemetry aggregation, and the authority on a match: deals it, issues seeds, judges each run from its streamed events and ranks the players, by linking `challenge-core` (ADR 0009). A state machine over `protocol` messages; the transport is whoever delivers them. Never runs a game; never depends on `session-libretro`, `content-gen` or `asset-store`. |
| `content-gen` | Content derivation, and the only crate that touches ROM bytes: applying a contributor's BPS patch (ADR 0020) and the seeded value-substitution engine (ADR 0015). No per-game code, no emulator dependency. |

## Conventions

- Manifests are TOML, one per *base* ROM hash, versioned with a `schema_version`
  field, and live in `manifests/`. The schema is `docs/manifest-schema.md`;
  `challenge-library` is the authority on what parses in the challenge half
  (`schema_version`, `[game]`, `[[challenge]]`, stage names, what a rule emits)
  and `session-libretro` on the runner half. Generated artifacts get no
  manifest of their own; they are identified by their derivation key (ADR 0013).
  A hand-authored hack gets no manifest of its own either — it is a `[[rom]]`
  beside the base it patches, and every stage names which ROM it runs on (ADR
  0020). It may only share that manifest if the telemetry holds against it
  unchanged; a hack that moves an address earns a manifest of its own.
- **The platform never distributes ROM bytes.** A player supplies their own dump.
  A hack ships as a patch, and a generated ROM never leaves the machine that
  derived it (ADR 0013/0020). A field holding a path to a hacked ROM is that rule
  being broken.
- A manifest reports *facts* — cleared, exited, died, damaged, enemy defeated,
  item, count, objective — and declares *criteria* (`[[challenge]].complete_when`).
  It never produces a verdict: an adapter must never decide a run was won, and
  there is no `stage_failed`. `challenge-core` compares the facts against the
  criteria. Every criterion is a goal ("at least"); a fact that must *not* happen
  goes in `ends_attempt_on`, where it costs what a death costs (ADR 0025). A
  contributor's own counters and objectives are open-ended names, so a game
  unlike the slice still needs no Rust.
- Manifests may author durations in seconds. They convert to frames at the game's
  framerate on load — seconds never reach a `SessionEvent` or a wire message.
- Wire protocol messages carry a version field. Never remove a field; deprecate it.
- Integration tests use `session-fake`. A test that needs a ROM is a manual test.
- Prefer encoding a rule as a type or schema over describing it here.

## Where decisions live

`docs/adr/`. Read `docs/adr/README.md` first. Do not re-open a settled question
without reading its ADR — several rejected alternatives are seductive and were
rejected for non-obvious reasons.

## Current phase

Phase 2 (complete): `smb3-harness` and `dkc-harness` dissolved into
`session-libretro` + `session-api`, and both games became manifests. The schema
is `docs/manifest-schema.md` and the decision is ADR 0016. Not core-specific:
what varies between the two consoles is `[runner.libretro.memory]` and a pad
layout, both per console family rather than per core.

Then `schema_version = 2` (ADR 0017): challenges became contributor-authored
`[[challenge]]` entries with declared completion criteria, rather than a closed
Rust enum plus an eligibility list on the stage.

Then Doom (ADR 0018/0014): the DOS spike folded into `session-libretro` and
`manifests/doom.toml` loads and runs. All three slice games are now manifests
against one adapter. What DOS needed was confined to the runner half of the
schema — `controller = "keyboard"`, `boot_seconds`,
`[runner.libretro.options]`, `[runner.libretro.memory.scan]`, and `u32` as a
watch width — and nothing in `[watch]`, `[[rule]]`, `[[challenge]]` or
`SessionEvent` changed. The spike crate is deleted.

Five things learned there are worth not rediscovering:

- **A DOS bundle carries the game's configuration, not just its files.** DOOM
  reads its sound card out of a `DEFAULT.CFG` that `SETUP.EXE` normally writes;
  with no such file both devices default to `snd_none` and the game boots
  correctly, runs correctly and plays in total silence. A console ROM has no
  equivalent — there is nothing beside it to get wrong. Two of the four members
  of `doom-shareware-e1m1.zip` are ours rather than id's, and the state a stage
  enters at has to be recaptured whenever any of them changes, because a DOS
  state is the whole machine including whatever the config already
  initialised.

- **A core may publish no system RAM at all.** dosbox-pure answers
  `retro_get_memory_data` with nothing and publishes `SET_MEMORY_MAPS`
  descriptors instead. The runner requires *one* route, not that route.
- **`retro_serialize_size` is not free to call.** On dosbox-pure it opens a
  handshake that `retro_serialize` closes; calling it before
  `retro_unserialize` deadlocks. It also is not constant — 365 KB before the
  guest boots, 3.6 MB after — so it cannot be used to validate a state.
- **A core options table is part of a game's identity.** `dosbox_pure_cycles`
  changes how many emulated frames a piece of play takes, so two players on
  different values have incomparable scores (invariant 4).
- **Host headroom is not guest speed, and a PC has no 60 Hz.** The host spent
  under 1 ms a frame on Doom at 26800 cycles, which proved nothing about how
  fast the *emulated* CPU draws; and forcing 60 fps put Doom's 35 pictures a
  second on an uneven 1-2 cadence. Both made it feel slower than RetroArch at
  the same game speed. It now runs at 77000 and VGA's native 70.0866 Hz
  (`docs/games/doom.md`).

Then `schema_version = 3` and `content-gen` (ADR 0020/0015/0013): content
stopped being one file per manifest. `[rom]` became `[[rom]]` with an `id`,
`[[stage]].rom` is required everywhere, and `[[challenge]]` may carry the
`patch_slot` table the seeded engine rolls from. `content-gen` applies a
contributor's BPS patch and performs the value substitution; it has no emulator
dependency and no per-game code, so its golden-hash tests are ordinary
`cargo test`. `session-libretro::prep` is what ADR 0013 called "whatever
prepares a stage": dump → hack → roll → bytes, and the runner below it cannot
tell which of the three it was handed.

Three things learned there, in the same spirit:

- **A pool's duplicates are its weights.** `[0x72, 0x72, 0x72, 0x72, 0x81,
  0x82, 0x86, 0x87]` is a coin flip then an even choice among four. An engine
  that deduplicated it would make it a 20% roll and say nothing. Duplication is
  the only way this schema can express a non-uniform roll at all.
- **The generator is a wire contract.** Every player derives the artifact
  locally and the server aborts on a hash mismatch, so the RNG is written out
  by hand rather than taken from a crate, and changing it is a
  `generator_version` bump for the whole library.
- **A golden hash from another implementation is worth more than a round
  trip.** The three SMB3 hashes in `docs/games/smb3.md` were recorded against a
  Python script before the crate existed, and the crate reproducing them is
  evidence rather than a tautology.

Phase 1 (complete): single-game libretro harness, deliberately SMB3-specific.

Phase B (solo client, in progress): a solo match plays end to end.
`cargo run -p app-client` opens a menu; Host game opens a lobby of one on an
in-process `server` (ADR 0009); Start deals a match that crosses all three
consoles in one process, with the intro, countdown, death flash, garland or
skull, round standings and score screen drawn by a transparent overlay over
a native game window (ADR 0027). The rules are `challenge-core`'s: the
server deals from a seed, judges every run from the `Frame` messages the
client streams, and compares the client's provisional verdict with its own;
a disagreement or an artifact-hash mismatch ends the match loudly. The wire
is `protocol`, versioned from day one; the file check is `asset-store`.
`--host <name> --start --mode … --format kind:n --games a,b --limit <s>`
plays a match from the command line. Learned on the way:

- **A Tauri page needs a capability to hear anything.** Without
  `capabilities/default.json` granting `core:default`, `listen` is refused
  and nothing says so on either side. The overlay HUD now reports it.
- **The libretro frontend's statics are per core, not per process.** A match
  loads one core after another; `Core::load` now resets the directories,
  the memory map, the options table and the keyboard callback the previous
  core left behind.
- **The server replays the judgement from exactly the frames it is sent.**
  The client streams a `Frame` for every frame that carried an event and for
  the frame its own judge settled the run on, nothing else, and the server's
  `Run` reaches the same verdict from those alone because every criterion
  is monotone in the frame. A client that skips one disagrees with the
  server and the match fails, which is the check working.

What is *not* done, and the next things to settle:

- **Phase B is not finished.** Owed: a key or pad binding for give-up and
  pause beyond F9/F10 on the keyboard; fullscreen (both windows borderless,
  ADR 0027); the protocol has no word for "I cannot prepare this round" (a
  missing dump at deal time ends the match through `EndMatch`); the
  rolled-content seed now comes from the server, but ADR 0013 stays
  Proposed until a remote server issues it (Phase C). Hand-tested on all
  three games; nothing in `app-client` is covered by `cargo test`, and the
  overlay is not a test target — the server's tests play the client's loop on
  `session-fake` instead.
- **`asset-store` will need to hash a bundle's members, not the container.** A
  DOS game is a zip, and its hash depends on how it was zipped (ADR 0018). That
  work also gates DOS hacks: a BPS patch against a zip is meaningless, so a PWAD
  arrives as a member overlay and ADR 0020 defers it until members are hashable.
- **Generated content is playtested, but its seed has nowhere to come from.**
  The pipeline is end to end — dump, patch, roll, rebind, enter — and the two
  SMB3 custom challenges in `manifests/smb3.toml` have been played through to
  their criteria on rolled ROMs. ADR 0013 has the server issuing the seed and
  comparing the artifact hashes clients report; the in-process server does
  both for a lobby of one, and the harness still takes `--seed`. A remote
  server doing it for several players is Phase C.
- **A rolled quantity still cannot reach `complete_when`.** It is static TOML
  with no arithmetic and nothing that can name the seed, so a generator cannot
  roll a count a criterion then refers to. Both SMB3 challenges avoid it by
  rolling types rather than counts. It is the first real boundary a generator
  hits and it is still unsettled.
- **Patch slots hang off `[[challenge]]`, so two kinds on one stage duplicate the
  table.** `manifests/smb3.toml` carries each of its two tables twice, and
  nothing stops the copies drifting. The alternative is hanging them off
  `[[stage]]`, which is a schema decision nobody has made.
- No networked lobby, no spectating, no head-to-head. Join game and Settings
  are buttons that do nothing yet.
