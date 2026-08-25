# CLAUDE.md

Multiplayer retro-game challenge platform. Players race through stages drawn from
multiple classic games simultaneously; live telemetry and spectating.

Solo developer. Windows targets. Vertical slice = SMB3 (NES), DKC (SNES), Doom (PC).

## Challenge types

- **Time trial** — reach a defined end point as fast as possible.
- **Endurance** — survive a defined span without dying.

Either type may run on **vanilla** content (a stage as it originally shipped) or
on **generated** content (layout produced per attempt from a seed — ADR 0013,
not before Phase 3). Both stage sources feed the same sequencing and scoring.

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
   If it requires touching Rust, the abstraction has failed — fix the abstraction.
   Adding a *generator* for procedural content is code (ADR 0013); its output is
   still data, and `challenge-core` still never learns generators exist.
6. **Every phase ends in something runnable.** No big-bang integration.

## Structure

| Crate | Role |
|---|---|
| `challenge-core` | Stage sequencing, scoring, rules. Pure. |
| `session-api` | `StageRunner`, `Presentable`, shared types. |
| `session-libretro` | SMB3, DKC, and every later libretro game. Software cores only for now. One crate, no per-game code: a game is a manifest (ADR 0016). |
| `session-external` | Doom. Process launch + stats-file harvest. |
| `session-fake` | Scripted telemetry. Used by all integration tests. |
| `asset-store` | ROM hash verification, content-addressed bundles. |
| `app-client` | Tauri shell, overlay, wiring. |
| `server` | Lobby, clock sync, telemetry aggregation. Never runs a game. |
| `content-gen` | Per-game procedural stage generators. Phase 3; does not exist yet. |

## Conventions

- Manifests are TOML, one per *base* ROM hash, versioned with a `schema_version`
  field, and live in `manifests/`. The schema is `docs/manifest-schema.md`;
  `session-libretro` is the authority on what parses. Generated artifacts get no
  manifest of their own; they are identified by their derivation key (ADR 0013).
- A manifest reports *facts* — cleared, exited, died, damaged, item. What a run
  was worth is `challenge-core`'s question. There is no `stage_failed`.
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

Phase 1 (complete): single-game libretro harness, deliberately SMB3-specific.

What Phase 2 did *not* do, and the next things to settle:

- **No playtest of DKC's addresses.** Almost all have two independent sources
  and none have been confirmed against the running game. `--frames` and
  `--addr` are the tools; `docs/games/dkc.md` lists what is unverified.
- **ADR 0014 is still owed** the external-process half: Doom has no addressable
  core RAM, and `[runner.external]` is where that answer goes.
- **`challenge-core` does not exist.** `SessionEvent`s are produced and nothing
  scores them yet.
- No generated content (`content-gen`, ADR 0013/0015 — the schema documents
  `[[challenge]]` patch slots and the loader refuses them), no endurance
  scoring, no head-to-head.
