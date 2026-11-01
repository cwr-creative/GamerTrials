# GamerTrials

A multiplayer challenge platform for classic games. Players race simultaneously
through a sequence of stages drawn from several games at once, with live
telemetry and spectating.

## Challenges

A **challenge** is one stage, one kind, and the criteria that complete it —
reach the end, survive 45 seconds, defeat the boss, stomp twenty enemies. A
match runs in time trial mode, survival mode, or both, and deals a randomised
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
a seed. Everyone in a match gets the same seed, and the last two compose: a
human designs the level, and the generator rerolls the enemies inside it.

The platform never distributes game files. A player supplies their own; a hack
travels as the difference from it, and a generated stage is built on the machine
that plays it.

**Spectating is a core requirement.** A player who finishes a stage can watch
anyone still playing it, whatever game they are in.

Head-to-head play, where players share one running game, is deliberately the last
thing on the list: it needs synchronised netcode rather than one-way telemetry.

## Status

Vertical slice, in progress: a three-game library — Super Mario Bros. 3 (NES),
Donkey Kong Country (SNES) and Doom (DOS) — and a solo match that plays end to
end across all three. Players supply their own game files; the client verifies
them by hash.

Every game runs on an **emulated machine** (ADR 0018). DOS games go through the
same libretro adapter as the consoles, under dosbox-pure, so there is one
telemetry mechanism and one spectating mechanism rather than two. Modern source
ports and Windows-era PC games are out of scope.

Not done yet: a networked lobby, spectating, and head-to-head.

```sh
# the client: menu, lobby of one, a match
cargo run -p app-client

# the developer harness a manifest is authored with
cargo run --release -p session-libretro --bin harness -- --game smb3 --challenge 1-1-time-trial
```

It builds on Windows and on Linux. On Ubuntu, run `bash tools/setup-ubuntu.sh`
first: it installs the libraries the client needs, and Rust. The client runs
under X11, or XWayland on a Wayland desktop (ADR 0035). Every push builds both
platforms in GitHub Actions and leaves a download of each under the run's
Artifacts.

`CLAUDE.md` has the crate layout, the invariants and the current phase;
`docs/manifest-schema.md` is what a manifest contains; `docs/adr/` is why the
architecture is the way it is — start with `docs/adr/README.md`. Per-game notes
are in `docs/games/`.
