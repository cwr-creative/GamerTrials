# 0036. Run head-to-head on the players' own machines, over rollback, superseding 0004

Date: 2026-11-01
Status: Proposed

## Context

Head-to-head — two players in one running game: a fight in Street Fighter
II, Super Mario Kart's battle mode, a Doom deathmatch — has been deferred from
the start. ADR 0004 delegated its netcode to "external per-game solutions" and
was never written up; CLAUDE.md says nothing in the slice may assume it
exists. Phase C is played and ADR 0013 is Accepted, so it is next.

Everything built so far is one-way telemetry: each player runs their own
emulated machine, streams facts to the server, and the server judges (ADR
0009). A shared game needs the two machines to agree frame for frame, which
is a different problem, and three things were learned on the way here that
decide its shape:

* **The console cores are exact.** The replay spike on branch
  `input-replication` (`docs/replay-spike.md` there; ADR 0034) replayed
  recorded input into Mesen and Snes9x and got the same RAM, picture, sound
  and events on every frame — from a fresh core, from a core that had just
  played, and from Windows to Linux. That is the determinism rollback needs.
* **dosbox-pure is not.** It emulates on its own thread, takes input
  mid-frame unless the frontend lets it park, leaves part of its mixer out of
  its state, and its state is 3.6 MB with a handshake that deadlocks if
  `retro_serialize_size` is asked and not followed by `retro_serialize`
  (`session-libretro/src/libretro.rs`). Saving and restoring it every frame is
  out.
* **dosbox-pure brings its own network.** RetroArch 1.16 added the libretro
  *netpacket* interface (`RETRO_ENVIRONMENT_SET_NETPACKET_INTERFACE`, 78),
  under which a core sends and receives its own packets and the frontend only
  carries them; dosbox-pure uses it to emulate an IPX driver and a modem.
  Doom's own lockstep netcode runs over that IPX driver, exactly as it did on
  two DOS machines in 1994. libretro's header names this the alternative to
  "state serialization-based multiplayer", which is what rollback is.

Latency is the other force. The server runs on a cheap box of the host's own.
Rollback does not hide latency; it moves it. Each machine applies its own
player's input at once and guesses the opponent's, then restores and
re-simulates when the real input arrives. The one-way latency is therefore the
number of frames the opponent's actions are late by, and the number of frames
re-simulated on a wrong guess — a few is invisible, five or six is a visible
warp. A relay through the server adds a leg to both.

## Decision

**A third kind of challenge, `head_to_head`.** It seats `slots` players
(two, for the slice) on one shared stage; what a death costs is what the game
says — a round in Street Fighter, a respawn in Doom — so a death is a fact
and never an ending. The round (a *bout*) ends when one slot meets its
criteria: that run is completed, every other live slot's is failed, *beaten*,
on the same frame. Two completing on one frame is a draw. Both at the limit
is everyone timing out, which ADR 0026 already reads as everyone failing. A
give-up hands the win to whoever stays; a dead link between the players is
lost by both — nobody wins a bout the network ended. This narrows ADR 0023's
first rule and ADR 0017's two-kind table to the two kinds that re-enter or
spectate; neither is reopened for them.

**Slots are drawn at random, and the rest back a slot.** When a bout is dealt
to more active players than it has slots, the players who sit are drawn from
the active ones with a seed derived from the match seed and the round index.
Every other active player backs a slot — throws in their lot — and is
recorded with a copy of the backed player's round entry: completed, place,
frames, success, failure, points. Every format and standing then reads a
backer exactly as it reads a competitor (ADR 0026), and a Stock or Strikes
match can put a backer out. A player who has not picked within fifteen
seconds of the deal is assigned a slot at random, so every active player has
a result every round; a sat-out round would make time-trial totals a reward
for sitting out. This assignment is not seed-reproducible — it depends on
which picks arrived — and nothing downstream needs it to be.

**Dealing is gated by data, never by code that knows a core.** A manifest's
`[[challenge]]` may say `kind = "head_to_head"` only with `slots >= 2`
(`challenge-library`), and `session-libretro` loads such a manifest only when
`[runner.libretro].netplay` says how the machine is shared: `"rollback"` for
a core whose state is exact and cheap to save every frame, `"packets"` for a
core that brings its own network. A lobby toggle, "allow head-to-head",
admits the kind in every mode, and a bout is dealt only while the active
players number at least its slots. `challenge-core` learns that a challenge
seats slots; it never learns what a core is.

**Rollback runs in the frontend over the machine's own save and restore.**
`session-api` gains a `SharedRunner`: a stage with slots, per-slot input, a
`Snapshot` of the machine and its observer, `restore`, and a `digest` of what
two machines fed the same inputs must agree on. The new `head-to-head` crate
drives it through the `ggrs` crate (GGPO reimplemented in Rust, MIT/Apache):
ggrs asks for a save, a restore or an advance, and the crate answers with the
trait. Facts a predicted frame produced are held until ggrs confirms the
frame and discarded on a restore, so the judge — the client's provisional one
and the server's — only ever sees facts from frames that happened. A
checksum over the digest, every few frames, is compared between the two
machines; a mismatch ends the bout loudly, as an artifact mismatch does.

**A core that brings its own network is relayed, not rolled back.**
`session-api` also gains `Networked`: join a session as a slot, deliver a
packet, drain what the core wants sent. `session-libretro` implements it over
the netpacket interface; the `head-to-head` crate carries the packets over
the same link rollback uses. Each player still runs their own emulated
machine (invariant 7); the game's own netcode keeps them in step. Doom's entry
is a boot with a command line, because the netpacket contract forbids
restoring a state while connected.

**The players' inputs travel directly between their machines over UDP.** The
server brokers the connection and nothing more: each seated client binds the
UDP socket it will play on, probes the server's UDP port so the server learns
its public address, and tells the server its LAN address; once both are
known the server tells each the other's candidates, and the clients send to
all of them until one answers. That is what lets two clients on one machine,
two on a LAN and two across the internet connect without a STUN service.
Two refinements came from the first try across the internet, a player on the
server's home network against a laptop on a phone. A player who reaches the
server from its own network is seen only at private addresses, so the other
client also tries the server's public address, the one it dialled, on that
player's port. And a carrier's NAT gives each destination its own port, so a
packet from a candidate's IP on an unlisted port is taken as that slot's
when only one slot still waiting has that IP. The bout socket binds a fixed
port, 7879 unless Settings says otherwise, so a player at home can forward
it; one forwarded side is enough for any pair, and two sides that both
cannot be reached cannot play.
There is no relay: a pair that cannot reach each other voids the round for
everyone (ADR 0029), since a client reports itself prepared only once ggrs
has synchronised with every peer. This narrows ADR 0030's "a client never
listens for other players" and ADR 0032's rejection of peer connections to
the match path and the spectator stream, which stay server-relayed. A
competitor's WebSocket dropping still ends the match.

**Every scored number stays an emulated frame** (ADR 0005). The fifteen
seconds and the connection timeouts are lobby timing; the server takes a
clock tick from whoever drives it and compares nothing a player did against
it.

## Alternatives considered

**Relay the inputs through the server over the existing WebSocket.** No NAT
traversal, no new socket, works behind the TLS proxy today; rollback tolerates
the hop. Rejected by the host of the project: the server is a cheap box at
home, and one-way latency through it is the sum of both legs, which is frames
of warp on every exchange. Kept as the obvious fallback should the direct
path prove unworkable for some pairs; it would be a new ADR, not a quiet
addition.

**A hand-written rollback engine**, fully specified like the deal's SplitMix64.
Time synchronisation, input prediction, disconnect handling and desync
detection are the bulk of GGPO, and nothing here needs to be reproducible
across upgrades the way a seed does. ggrs is sans-IO — it owns no socket, we
hand it ours — so it fits the fence.

**Delay-based lockstep only.** Simpler: no saves, no restores, no checksums.
Every frame waits for the opponent's input, so every millisecond of latency is
felt on one's own buttons, which is what fighting games cannot stand. ggrs
does it anyway with a prediction window of zero, so it costs nothing to keep
as a setting.

**Rollback on dosbox-pure.** Rejected by measurement, above.

**Native Doom ports with real netcode.** ADR 0018 rejected the category.

**The server runs the shared game.** ADR 0009: the server never runs a game.

**One player hosts the shared game and streams it.** ADR 0009's rejected
"one player hosts", with a stream's latency on the guest's every button.

## Consequences

**ADR 0004 is superseded.** Head-to-head is the platform's, in one crate,
on the same manifests.

**New vocabulary.** A *slot* is a controller port on the shared machine; a
*bout* is one head-to-head round; a *backer* is an active player not seated
in it. The server's `Seat` keeps meaning a connection.

**A UDP port beside the WebSocket one**, and a flag to say where it is when a
proxy fronts the WebSocket (ADR 0033). `docs/server.md` says which to open.
And a player's own bout port, UDP 7879 by default, which a player at home
forwards to be reachable from a phone or a VPN.

**Protocol version 5.** The new messages are all control lane and the stream
is untouched; only ggrs traffic leaves the WebSocket.

**Two judges see the same facts in a different order.** Each slot's facts
reach the server on its own socket, so a bout settles in frame order and not
arrival order: a completion at frame F waits until every other slot has
reported a frame at or past F. That is a rule in `challenge-core`, with the
arrival orders as tests.

**What is measured next.** The first bout over the internet records ggrs's
own numbers — round trip, frames ahead, rollbacks — against this ADR, which
stays Proposed until then, as 0013 did.
