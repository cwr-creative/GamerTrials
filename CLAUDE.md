# CLAUDE.md

Multiplayer retro-game challenge platform. Players race through stages drawn from
multiple classic games simultaneously; live telemetry and spectating.

Solo developer. Windows targets. Vertical slice = SMB3 (NES), DKC (SNES), Doom (PC).

## Challenges

A session runs in one of three modes — time trial, survival, or both — and deals
a randomised series of challenges from the library. Three words, used precisely
(ADR 0017):

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

Neither kind has a failure bound yet, and one is owed before either ships; see
ADR 0017.

Either kind may run on **vanilla** content (a stage as it originally shipped) or
on **generated** content (a ROM patched per attempt from a seed — ADR 0013/0015,
not before Phase 3). Both feed the same sequencing and scoring.

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
   So is adding a *challenge* to a game (ADR 0017). If either requires touching
   Rust, the abstraction has failed — fix the abstraction. Adding a *generator*
   for procedural content is code (ADR 0013); its output is still data, and
   `challenge-core` still never learns generators exist.
6. **Every phase ends in something runnable.** No big-bang integration.

## Structure

| Crate | Role |
|---|---|
| `challenge-core` | Stage sequencing, scoring, rules. Pure. |
| `session-api` | `StageRunner`, `Presentable`, shared types. |
| `session-libretro` | SMB3, DKC, and every later libretro game. Software cores only for now. One crate, no per-game code: a game is a manifest (ADR 0016). |
| `session-external` | Doom, and other classic PC games in the future. Process launch + stats-file harvest. |
| `session-fake` | Scripted telemetry. Used by all integration tests. |
| `asset-store` | ROM hash verification, content-addressed bundles. |
| `app-client` | Tauri shell, overlay, wiring. |
| `server` | Lobby, clock sync, telemetry aggregation. Never runs a game. |
| `content-gen` | Per-game procedural stage generators. |

## Conventions

- Manifests are TOML, one per *base* ROM hash, versioned with a `schema_version`
  field, and live in `manifests/`. The schema is `docs/manifest-schema.md`;
  `session-libretro` is the authority on what parses. Generated artifacts get no
  manifest of their own; they are identified by their derivation key (ADR 0013).
- A manifest reports *facts* — cleared, exited, died, damaged, item, objective —
  and declares *criteria* (`[[challenge]].complete_when`). It never produces a
  verdict: an adapter must never decide a run was won, and there is no
  `stage_failed`. `challenge-core` compares the facts against the criteria.
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

Phase 1 (complete): single-game libretro harness, deliberately SMB3-specific.

What is *not* done, and the next things to settle:

- **ADR 0014 is still owed** the external-process half: Doom has no addressable
  core RAM, and `[runner.external]` is where that answer goes.
- **`challenge-core` does not exist.** `SessionEvent`s are produced, challenges
  declare what completes them, and nothing yet compares the two. It also owes
  the failure bound neither kind has (ADR 0017) and the crate split that moves
  `[[challenge]]` parsing out of an adapter.
- No generated content (`content-gen`, ADR 0013/0015 — the schema documents
  patch slots and the loader refuses them), no scoring, no head-to-head.
