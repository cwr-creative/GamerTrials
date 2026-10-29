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
content (a ROM patched per attempt from a seed — ADR 0013/0015). All three feed
the same sequencing and scoring, and the last two compose — a generated
challenge sitting on a hack is the expected shape, not an exotic one: a human
designs the level, the generator rerolls enemies inside it.

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
| `app-client` | Tauri shell, overlay, wiring (ADR 0027). Menu, lobby and settings in an ordinary window; a native game window under a transparent overlay during a match. Prepares and runs stages and streams their events; judges only provisionally, to know whether to re-enter or spectate. Plays the server's half in-process for solo play (ADR 0009). Owns the player's bindings and video settings (ADR 0028). |
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

**Phase B — solo client, in progress.** `cargo run -p app-client` opens a
menu; Host game opens a lobby of one on an in-process `server` (ADR 0009);
Start deals a match across all three consoles in one process, drawn by a
transparent overlay over a native game window (ADR 0027). The server deals
from a seed, issues the rolled-content seed, judges every run from the `Frame`
messages the client streams, and ends the match loudly if the client's
provisional verdict or an artifact hash disagrees with its own.
`--host <name> --start --mode … --format kind:n --games a,b --limit <s>` plays
a match from the command line. Settings binds keyboard, mouse and gamepad
to the RetroPad once, overridable per core and per game, and sets video the
same way; nothing in it reaches a core option or the frame clock (ADR 0028).
Hand-tested on all three games before Settings; Settings itself is not yet
hand-tested in a match. `app-client`'s `cargo test` covers only its pure
modules (`settings`, `video`) — the server's tests play the client's loop on
`session-fake` instead.

Behind it: Phase 1 (an SMB3-only harness), Phase 2 (one libretro adapter,
games as manifests — ADR 0016), contributor-authored `[[challenge]]`
entries (`schema_version = 2`, ADR 0017), Doom under dosbox-pure (ADR 0018), and
`schema_version = 3` with `content-gen` (ADR 0020/0015/0013). What each phase
learned is written down where it applies — `docs/games/`, the crate READMEs,
and the comments in `session-libretro/src/libretro.rs` — so read those before
changing that code.

Open, roughly in order:

- **Phase B owes:** a protocol word for "I cannot prepare this round" (a
  missing dump at deal time ends the match through `EndMatch`. Borderless
  fullscreen is hand-tested (alt-tab, Win+Down, a monitor move), as is Doom on
  a pad and on mouse and keyboard. Join game does nothing yet.
- **ADR 0013 stays Proposed** until a remote server issues seeds and compares
  hashes for several players (Phase C), along with the networked lobby and
  spectating. Head-to-head comes last.
- **`asset-store` must hash a bundle's members, not the container.** A DOS
  zip's hash depends on how it was zipped (ADR 0018), and DOS hacks — a PWAD as
  a member overlay — wait on it (ADR 0020).
- **A rolled quantity cannot reach `complete_when`.** Criteria are static TOML
  that cannot name the seed, so a generator cannot roll a count a criterion
  refers to. Both SMB3 generated challenges roll types instead. Unsettled.
- **Patch slots hang off `[[challenge]]`**, so two kinds on one stage duplicate
  the table (`manifests/smb3.toml` carries each of its two twice) and nothing
  stops the copies drifting. Hanging them off `[[stage]]` is the undecided
  alternative.
