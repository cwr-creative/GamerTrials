# CLAUDE.md

Multiplayer retro-game challenge platform. Players race through stages drawn from
multiple classic games simultaneously; live telemetry and spectating.

Solo developer. Windows and Linux (X11) targets, from one tree (ADR 0035).
Vertical slice = SMB3 (NES), DKC (SNES), Doom (DOS).

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

The kinds differ in what death costs, not in what completes them:

- **Time trial** — on death, re-enter at the stage's entry point with the clock
  still running. Scored on total frames across every attempt.
- **Survival** — on death the run is over and the player spectates. Scored on
  whether the criteria were met at all. "Without dying" is therefore not a
  criterion; it is what the kind means.
- **Head-to-head** (ADR 0036) — several players share one running stage, one
  per **slot** (a controller port on the shared machine; the challenge says
  how many). A death costs what the game says — a round, a respawn — and is
  only a fact. The round, a **bout**, ends when one slot meets its criteria:
  that run is completed, every other live slot's is *beaten* on that frame.
  Whoever stays after a give-up wins; a dead link is lost by both. When a
  bout is dealt to more players than it has slots, who sits is drawn from
  the seed and everyone else is a **backer**: they pick a slot (or are given
  one after 15 s) and are recorded with a copy of that player's entry.

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

Head-to-head is built last, and in two mechanisms (ADR 0036): a console core
whose state is exact is kept in step by **rollback** in the frontend — the
machine saves itself every frame, guesses the remote slot's input, and
restores and replays when the guess was wrong (`SharedRunner`, driven by the
`head-to-head` crate on `ggrs`); a core that brings its own network
(dosbox-pure's emulated IPX) is **relayed** packet by packet while the game's
own netcode keeps the machines in step (`Networked`). The two players' inputs
and packets go directly between their machines over UDP; the server only
brokers the addresses. A manifest says which with `[runner.libretro].netplay`,
and only such a manifest may carry a `head_to_head` challenge.

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
| `session-libretro` | SMB3, DKC, Doom, and every later game. Software cores only for now. One crate, no per-game code: a game is a manifest (ADR 0016). DOS games run here too, under dosbox-pure (ADR 0018) — same crate, same schema, same events. Also holds `x11`, the Linux twin of the Win32 calls the game window needs (ADR 0035). |
| `session-fake` | Scripted telemetry. Used by all integration tests. |
| `asset-store` | ROM hash verification, content-addressed bundles, and the cache of artifacts derived from them. Today: hashes a player's dump against the manifest's `sha1` and says what is missing. |
| `protocol` | The wire between client and server (ADR 0024): versioned messages with types of their own, never `session-api`'s or `challenge-core`'s on the wire. Conversions live here. Since ADR 0032 it also carries a watched player's picture and sound (`Video`, `Audio`) for the server to relay, their bytes in a binary frame after the packet, and the run's `Progress` for a watcher (ADR 0034). Says which messages are stream and may be dropped (`Lane`). Pure: no sockets. |
| `transport` | One WebSocket's loop for both ends (ADR 0034): non-blocking, the match before the stream, a stream the link cannot deliver fresh dropped, the link timed by stamped pings, liveness kept. `app-client`'s link and each `server-net` connection run it. Held to the server's fence. |
| `app-client` | Tauri shell, overlay, wiring (ADR 0027). Menu, lobby and settings in an ordinary window; a native game window under a transparent overlay during a match. Prepares and runs stages and streams their events; judges only provisionally, to know whether to re-enter or spectate. Plays the server's half in-process for solo play (ADR 0009). Owns the player's bindings and video settings (ADR 0028). While another player watches, encodes its own picture (H.264) and sound (ADPCM) off the game thread; when done or out, shows theirs on the overlay (ADR 0032). Each Win32 call has an X11 twin beside it under `cfg(target_os = "linux")` (ADR 0035). |
| `server` | Rust (ADR 0024). Lobby, telemetry aggregation, and the authority on a match: deals it, issues seeds, judges each run from its streamed events and ranks the players, by linking `challenge-core` (ADR 0009). A state machine over `protocol` messages; the transport is whoever delivers them. Relays a watched player's stream to their watchers, and refuses a watch from a player still playing (ADR 0032). Never runs a game; never depends on `session-libretro`, `content-gen` or `asset-store`. |
| `server-net` | The server process (ADR 0030): `gamertrials-server`, lobbies by invite code over WebSockets (a thread per socket, each running `transport`'s loop, and per lobby), each lobby a `server::Server`. Also the UDP side a head-to-head player probes so the server can tell the other slot where they are (ADR 0036); it carries nothing else. Held to the same dependency fence as `server`. |
| `content-gen` | Content derivation, and the only crate that touches ROM bytes: applying a contributor's BPS patch (ADR 0020) and the seeded value-substitution engine (ADR 0015). No per-game code, no emulator dependency. |
| `head-to-head` | One bout on one shared machine (ADR 0036). `Rollback` drives a `SharedRunner` through `ggrs`, answering its save, restore and advance requests and holding each frame's facts until every slot's input for it is final; `Relay` carries a `Networked` machine's packets. Both over a `Link` between the slots: `Udp` between two players' machines (candidates tried until one answers), `Channels` in a test. No emulator, no protocol, no server. |

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

**Phase B — the client — is built; Phase C — spectating and a server beyond
the LAN — is built, and has played matches over the internet with live
players.** `cargo run -p
app-client` opens a menu. Host game opens a lobby on the `gamertrials-server`
named in Settings › Profile, or a lobby of one on an in-process `server` when
none is (ADR 0009). Join game takes an invite, `host[:port]/CODE` (ADR 0030).
`cargo run -p server-net` runs the server. Start deals a match across all
three consoles, drawn by a transparent overlay over a native game window (ADR
0027). The server deals
from a seed, issues the rolled-content seed, judges every run from the `Frame`
messages the client streams, and ends the match loudly if the client's
provisional verdict or an artifact hash disagrees with its own.
`--host <name> [--server <addr>] --start --mode … --format kind:n --games a,b
--limit <s>` plays a match from the command line, and `--join <invite> --name
<name>` joins one. A round a client cannot prepare is voided for everyone
(ADR 0029). A player whose run is over, or who is out of the match, watches
another's picture and hears their sound on the overlay, relayed by the
server over the same socket (ADR 0032); ◀ ▶ or the pad switches players.
The sender encodes only while watched. A link too slow for the stream
drops pictures rather than the match, and a watcher whose picture cannot
keep up sees the run's clock, attempt and events instead (ADR 0034, from
the first match over the internet). `wss://host` in Settings › Profile
or an invite reaches a server behind a TLS proxy (ADR 0033,
`docs/server.md`). Settings binds keyboard, mouse and gamepad to the RetroPad once, overridable per core and per game, and sets video the
same way; nothing in it reaches a core option or the frame clock (ADR 0028).
Hand-tested on all three games before Settings; Settings itself is not yet
hand-tested in a match. `app-client`'s `cargo test` covers only its pure
modules (`settings`, `video`, `link`'s parsing). The server's tests play the
client's loop on `session-fake` instead, and `server-net`'s do it over real
sockets with two players — `slow_link` with one of them held to a phone's
rates through a throttling relay.

**Phase D — head-to-head (ADR 0036, Proposed) — is under way, in five
steps, each runnable.** Done: the vocabulary and the judge. `session-api`
has the third kind, `slots`, and the shared-machine traits (`SharedRunner`,
`Networked`); `challenge-core` has `Bout` (a run per slot, settled in frame
order across every slot's socket), seating drawn from the seed, and backing;
`challenge-library` parses `kind = "head_to_head"` with `slots`;
`session-fake` has `FakeMachine`, a two-slot deterministic game; protocol
v5 carries `Back`, `Endpoint`, `PeerLost`, `Sides` and `Peers`; the server
deals bouts, brokers the two players' addresses from their UDP probes,
closes the backing window on a tick, judges the bout and records backers;
`server-net` opens the UDP side (`--udp`, `--udp-public`) and ticks each
lobby. Also done: the `head-to-head` crate — `Rollback` on `ggrs` over a
`SharedRunner`, holding each frame's facts until confirmed, with a
checksum over the machine's digest every few frames; `Relay` for a
`Networked` machine; `Udp` between the two machines, trying every candidate
address until one answers; `Channels` for tests. Tested with literal frames
in both arrival orders, a sync test on `FakeMachine`, two sessions over a
jittery channel agreeing frame for frame, a diverged machine caught, a cut
link lost, and a whole bout played over real UDP through the real
rendezvous in `server-net/tests/bout.rs`. And `session-libretro` is a
`SharedRunner`: one pad per port, a snapshot that carries the frame, the
watcher's edge state and the core's state (saved in the same-instance
context so a core may take its fast path), no picture or sound on a replayed
frame, and `netplay = "rollback"` in `[runner.libretro]` refused on a
threaded or keyboard core and on one whose serialization declares itself
incomplete; a `[[rule]]` says whose fact it is with `slot`. **The two
console cores are proved:** `harness --synctest` rolls a stage back on
every frame with no network and compares checksums, and Snes9x and Mesen
both pass 1800 frames at three and at seven replays per advance, with two
processes on one script writing the same digests on every frame
(`docs/games/dkc.md`, `smb3.md`, with the cost per frame). And the client
plays a bout: the lobby's "Allow head-to-head challenges" toggle; a seated
player's game thread binds a UDP socket, probes the server, reports its
`Endpoint`, takes the `Peers`, puts the runner into a `Rollback` session and
says `Prepared` only once the machines have found each other (ten seconds,
else `Unprepared`); on `Go` it pumps the session once a frame, paced on the
clock, judges every slot's confirmed facts with a provisional `Bout`,
reports its own slot's frames (and an empty one every fifteen), claims its
verdict, reports `PeerLost` when the link dies, and forfeits on a desync; a
pause cannot hold the shared machine but the menu still gives up; a player
not seated sees the backing sheet, picks a side or is given one, and
spectates as the eliminated do; Settings › Profile sets the input delay.
And the second mechanism: `session-libretro` answers the netpacket
interface (78) and is `Networked` — the core's `start`, `receive`, `poll`,
`connected`, `disconnected` and `stop` called from `join`, `deliver`,
`outgoing` and the peers — and the `head-to-head` crate's `Relay` carries
the packets with a frame header, a hello handshake before the bout goes,
and reliable packets resent until acknowledged and delivered in order; the
client takes that path when the manifest says `netplay = "packets"`, and
claims the server's verdict for the slot it cannot judge itself.
**The first bout is played**: `manifests/sf2.toml` (Street Fighter II
Turbo, `best-of-three`, its addresses found with the harness's scripted
sync test and RAM log — `docs/games/sf2.md`) on two clients on one machine
through `gamertrials-server`, with `--script` pressing the buttons: both
found each other through the rendezvous, judged the same verdict on the
same frame, and the server agreed; and with three, the third backing a
side by the deadline and taking that side's entry. **Not yet**: two
machines over the internet with ggrs's numbers recorded against ADR 0036
(which stays Proposed until then), and a bout with hands on the pads. Doom's deathmatch needs a bundle of its own whose `[autoexec]` runs
`IPXSETUP` (`docs/games/doom.md`); the shipped bundle warps to E1M1 alone,
so no `deathmatch` stage ships until that bundle exists.

Behind it: Phase 1 (an SMB3-only harness), Phase 2 (one libretro adapter,
games as manifests — ADR 0016), contributor-authored `[[challenge]]`
entries (`schema_version = 2`, ADR 0017), Doom under dosbox-pure (ADR 0018), and
`schema_version = 3` with `content-gen` (ADR 0020/0015/0013). What each phase
learned is written down where it applies — `docs/games/`, the crate READMEs,
and the comments in `session-libretro/src/libretro.rs` — so read those before
changing that code.

Open, roughly in order:

- **Hand-tested:** a networked match between two clients on one machine,
  with each player spectating the other in turn — picture and sound, and
  the sound starting without a click. Also borderless fullscreen (alt-tab,
  Win+Down, a monitor move), and Doom on a pad and on mouse and keyboard.
  **Not yet:** switching between several watched players, the eliminated
  watching from the deal, a voided round, the lobby's file re-check,
  Settings in a match, and `wss://` against a real proxy. The encoder and
  the ADPCM coder have unit tests; the relay is tested in-process and over
  the wire.
- **Phase C's two-machine match is played** (`docs/server.md`): live
  players on separate machines over the internet, generated challenges
  dealt and every artifact hash agreeing, so **ADR 0013 is Accepted**.
  Head-to-head is Phase D, above.
- **Linux (ADR 0035, Proposed) plays under WSLg, not yet on a desktop.**
  A round of each game runs with the overlay aligned over it; a real window
  manager, fullscreen, mouse capture, sound and a match against a Windows
  player are unseen. `cores/` holds each core for both platforms from one
  nightly — update both at once, since entry states are savestates. `bash
  tools/setup-ubuntu.sh` installs what a build needs; `bash
  tools/package.sh` is `package.ps1`'s twin. Closing the client mid-round
  may hang there: dosbox-pure's static destructors wait on its thread, as a
  harness that exited with the core loaded showed under WSL, and the end of
  Tauri's event loop exits the same way. Not checked.
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
