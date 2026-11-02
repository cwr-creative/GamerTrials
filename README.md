# GamerTrials

A multiplayer challenge platform for classic games. Players race simultaneously
through a sequence of stages drawn from several games at once, with live
telemetry and spectating.

## Challenges

A **challenge** is one stage, one kind, and the criteria that complete it —
reach the end, survive 45 seconds, defeat the boss, stomp twenty enemies. A
match runs in time trial mode, survival mode, or both, and deals a randomised
series of challenges from the library.

The kinds differ in what a death costs:

- **Time trial** — you re-enter at the start and the clock keeps running. Scored
  on your total frames across every attempt.
- **Survival** — the run is over, and you spectate. "Without dying" is not a
  criterion; it is what survival means.
- **Head-to-head** — several players share one running game, a fight in Street
  Fighter II, and a death costs whatever the game says it does. The first to
  meet the criteria ends the bout for everyone; whoever stays after a give-up
  wins. When more players are in the match than the game seats, who fights is
  drawn by the seed and everyone else backs a side, picks or is given one, and
  is scored exactly as the player they backed.

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

Head-to-head keeps the players' machines in step without a server in the
middle: a console game by rollback on the players' own machines, a DOS game by
its own netcode over an emulated network, and the two players' inputs travel
directly between them (ADR 0036).

## Status

Vertical slice: a five-game library — Super Mario Bros. 3 (NES), Donkey Kong
Country (SNES), Doom and Doom II (DOS) and Street Fighter II Turbo (SNES) — and matches
that play end to end across them, solo on one machine or in a lobby of several
over the internet. Players supply their own game files; the client verifies
them by hash.

Every game runs on an **emulated machine** (ADR 0018). DOS games go through the
same libretro adapter as the consoles, under dosbox-pure, so there is one
telemetry mechanism and one spectating mechanism rather than two. Modern source
ports and Windows-era PC games are out of scope.

What has been played: networked matches between live players on separate
machines, with generated challenges dealt and every artifact hash agreeing;
spectating, picture and sound, relayed through the server; and head-to-head
bouts between two and three clients on one machine through the server, in
Street Fighter II by rollback and in Doom II deathmatch over Doom's own
netcode. Not yet: a bout between two machines over the internet.

## Running it

Everything is `cargo`. Game files go under `assets/ROMs/` at the paths the
manifests name; the lobby says what is missing and what its hash should be.

**The client**, which is also a lobby of one:

```sh
cargo run --release -p app-client
```

Host game opens a lobby on the server named in Settings › Profile, or on an
in-process server when none is, and plays alone. Join game takes an invite,
`host[:port]/CODE`. Start deals the match. The lobby's settings are the mode,
the duration format, the time limit, and whether custom and head-to-head
challenges are dealt. Settings binds keyboard, mouse and gamepad, sets video,
and holds the server, the spectator stream's bitrate and the bout's input
delay.

**The server**, one binary with no state, lobbies by invite code:

```sh
cargo run --release -p server-net -- --listen 0.0.0.0:7878 --manifests manifests
```

It listens for WebSockets on that port and for head-to-head probes on the
same port over UDP, so open both on the host's firewall. Players put the
host's address in Settings › Profile, or paste the invite the lobby shows. A
server behind a TLS proxy, which players reach as `wss://host`, and the
`--udp-public` flag it then needs, are in `docs/server.md`. The server reads
only the challenge half of the manifests and refuses a client holding a
different library.

**From the command line**, for playtesting without the menu:

```sh
# host a one-round Street Fighter bout, start once two are in, press buttons by script
cargo run --release -p app-client -- --host A --server 127.0.0.1 --start --players 2 \
    --mode combo --format challenges:1 --games sf2 --h2h --script 3

# join it
cargo run --release -p app-client -- --join 127.0.0.1/CODE --name B --script 5
```

`--script` presses random buttons instead of reading the pad, so two clients
can play a bout with nobody at either keyboard; leave it off to play.

**The developer harness** a manifest is authored with: runs a game in a
window, watches its RAM, judges a challenge, and proves a core rolls back
exactly before a manifest says `netplay = "rollback"`:

```sh
cargo run --release -p session-libretro --bin harness -- --game smb3 --challenge 1-1-time-trial
cargo run --release -p session-libretro --bin harness -- --game sf2 --synctest 1800 --slots 2
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
