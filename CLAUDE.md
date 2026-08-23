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
| `session-libretro` | SMB3, DKC. Software cores only for now. |
| `session-external` | Doom. Process launch + stats-file harvest. |
| `session-fake` | Scripted telemetry. Used by all integration tests. |
| `asset-store` | ROM hash verification, content-addressed bundles. |
| `app-client` | Tauri shell, overlay, wiring. |
| `server` | Lobby, clock sync, telemetry aggregation. Never runs a game. |
| `content-gen` | Per-game procedural stage generators. Phase 3; does not exist yet. |

## Conventions

- Manifests are TOML, one per *base* ROM hash, versioned with a `schema_version`
  field. Generated artifacts get no manifest of their own; they are identified by
  their derivation key (ADR 0013).
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

Phase 2: Build the second single-game harness for DKC on the SNES core. Compare it against the SMB3 harness, create abstractions, then refactor to create the libretro harness. Create the manifest schema which is -- hopefully -- the same for all libretro games and not core-specific.

Phase 1 (complete): single-game libretro harness. Deliberately SMB3-specific and ugly.
Do not extract abstractions yet — that is Phase 2's job, after DKC exists.
No generated content, no endurance stages, no head-to-head in this phase.
