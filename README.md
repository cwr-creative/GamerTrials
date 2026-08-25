# GamerTrials

A multiplayer challenge platform for classic games. Players race simultaneously
through a sequence of stages drawn from several games at once, with live
telemetry and spectating.

## Challenges

A **challenge** is one stage, one kind, and the criteria that complete it —
reach the end, survive 45 seconds, defeat the boss, stomp twenty enemies. A
session runs in time trial mode, survival mode, or both, and deals a randomised
series of challenges from the library.

The two kinds differ in what a death costs:

- **Time trial** — you re-enter at the start and the clock keeps running. Scored
  on your total frames across every attempt.
- **Survival** — the run is over, and you spectate. "Without dying" is not a
  criterion; it is what survival means.

Whoever finishes first spectates the other until they finish too.

Challenges are **defined by whoever contributes the game**, in its TOML
manifest, alongside the RAM addresses that make them detectable. Adding a game
or a challenge is a data change, not a code change. Either kind can run on
**vanilla** content — a stage as it originally shipped — or on **generated**
content, where a ROM is patched fresh for each attempt from a seed. Everyone in
a session gets the same seed.

**Spectating is a core requirement.** A player who finishes a stage can watch
anyone still playing it, whatever game they are in.

Head-to-head play, where players share one running game, is deliberately the last
thing on the list: it needs synchronised netcode rather than one-way telemetry.

## Status

Vertical slice, in progress: a fully playable version with a three-game library —
Super Mario Bros. 3 (NES), Donkey Kong Country (SNES), and Doom (PC). Players
supply their own ROMs; the client verifies them by hash.

Phase 2 is done: the two single-game harnesses became one libretro adapter plus
two game manifests, and adding a libretro game is now a TOML file rather than a
crate. See `docs/manifest-schema.md` for what that file contains, `CLAUDE.md`
for the crate layout and the invariants, and `docs/adr/` for why the
architecture is the way it is — start with `docs/adr/README.md`.

```sh
cargo run --release -p session-libretro --bin harness -- --game dkc --stage jungle-hijinx
```
