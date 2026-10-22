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
manifest, alongside how to detect them. Adding a game or a challenge is a data
change, not a code change. Either kind can run on **vanilla** content — a stage
as it originally appeared in the published game — on a **hack**, a level someone
built in an editor and shipped as a patch against the original, or on
**generated** content, where the content is patched fresh for each attempt from
a seed. Everyone in a session gets the same seed, and the last two compose: a
human designs the level, and the generator rerolls the enemies inside it.

The platform never distributes game files. A player supplies their own; a hack
travels as the difference from it, and a generated stage is built on the machine
that plays it.

**Spectating is a core requirement.** A player who finishes a stage can watch
anyone still playing it, whatever game they are in.

Head-to-head play, where players share one running game, is deliberately the last
thing on the list: it needs synchronised netcode rather than one-way telemetry.

## Status

Vertical slice, in progress: a fully playable version with a three-game library —
Super Mario Bros. 3 (NES), Donkey Kong Country (SNES), and Doom (DOS). Players
supply their own game files; the client verifies them by hash.

Every game runs on an **emulated machine** — that is the line, not "runs under
libretro" (ADR 0018). DOS games go through the same adapter as the consoles,
under dosbox-pure, so there is one telemetry mechanism and one spectating
mechanism rather than two. Modern source ports and Windows-era PC games are out
of scope.

Phase 2 is done: the two single-game harnesses became one libretro adapter plus
game manifests, and adding a game is now a TOML file rather than a crate. All
three slice games load — including Doom, which needed four fields in the
runner half of the schema and nothing at all in the telemetry half.

Custom content followed, and it is data too. A manifest declares the ROMs its
stages run on: one base, plus any number of hacks carried as BPS patches against
it. A challenge may declare a table of *patch slots* — an offset, a width and the
values that are legal there — and `content-gen` rolls one artifact per attempt
from the session's seed. It has no per-game code and no emulator dependency, so
its tests are golden hashes rather than a running game. See
`docs/manifest-schema.md` for what a manifest contains, `CLAUDE.md` for the
crate layout and the invariants, and `docs/adr/` for why the architecture is
the way it is — start with `docs/adr/README.md`.

`challenge-core` compares the facts a manifest reports against the criteria it
declares: it judges a run, re-enters a time trial on death, deals a session's
challenges, and places the players of one challenge. The harness drives it with
`--challenge`.

What is not done: there is no session yet — nothing deals a real one, scores
across challenges, or decides how long a run may take — and no head-to-head.
The seed a generated challenge runs on still comes from a developer flag rather
than from a session.

```sh
cargo run --release -p session-libretro --bin harness -- --game dkc  --stage jungle-hijinx
cargo run --release -p session-libretro --bin harness -- --game doom --stage e1m1-hangar
cargo run --release -p session-libretro --bin harness -- --game smb3 --challenge 1-1-time-trial --limit 60
```
