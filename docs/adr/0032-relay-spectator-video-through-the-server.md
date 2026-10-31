# 0032. Relay spectator video through the server, superseding the media path of ADR 0003

Date: 2026-10-31
Status: Superseded by 0034

## Context

ADR 0003 decided that spectating is streamed video rather than replicated
inputs, and chose the mechanism of its day: each client encodes its own
framebuffer with ffmpeg's x264, sends it over a WebRTC mesh to every other
client, and the webview decodes. It planned an SFU at four players.

That was written before the server existed. Since then every player holds
one outbound WebSocket to `gamertrials-server` (ADR 0030), the lobby thread
owns the match, and ADR 0030 left the media path open: "whatever signalling
it needs can ride these sockets."

Retro pictures are small — 256x240, 256x224, 320x200 — and H.264 carries
one at its own rate in about a megabit. The scale is 4–8 friends. The
toolchain on the one development machine is MSVC, with neither cmake nor
nasm.

## Decision

**The stream is protocol messages, and the server relays them.** A client
whose run is being watched encodes its own picture and sound and sends
them as `ToServer::Video` and `ToServer::Audio`, like any other message, on
the socket it already holds. The server hands each to every player watching
that one (`ToClient::Video`, `ToClient::Audio`). The overlay page decodes
and plays them.

**Who may watch is a rule, so the server holds it.** `ToServer::Watch` names
a player, or nobody. A player still playing the round is refused — only a
finished or eliminated player spectates, so nobody sees a generated layout
they have yet to play (ADR 0026). Watching ends with the round.

**A sender encodes only while watched.** The server tells it how many are
watching (`ToClient::Watched`), and a rise starts a new group on a key
frame. A player's uplink is idle during their own run unless somebody
finished first.

**The picture is H.264 from openh264**, built from source by `cc` with the
compiler Tauri already needs, in Annex B form with the parameter sets on
every key frame, which is what the page's `VideoDecoder` takes with no
description. Encoding runs on its own thread; the game thread's whole cost
is one copy of the core's buffer, and a picture the encoder has no time for
is not offered. The sound is **IMA ADPCM**, written here, four bits a
sample at the core's own rate, with the coder's state at the head of every
chunk so a lost chunk is a gap and the next one still decodes. Sound is never
dropped by the sender; a picture may be.

**The page aligns them.** Picture and sound left the sender together and
travelled the same socket, so the page shows both a fixed lead after they
arrive, drops sound that falls more than twice the lead behind, and restarts
the lead when it runs dry. No clock crosses the wire for this; both carry
the emulated frame they belong to, which is there if a finer alignment is
ever wanted.

**ADR 0003's decision stands; its mechanism does not.** Video, not inputs;
the webview decodes; native code only sends. 0003 is marked superseded for
the record, and its rejection of input replication is restated here: it
needs bit-exact determinism across machines and fails silently.

## Alternatives considered

**WebRTC mesh with ffmpeg, as 0003 wrote.** Jitter buffering, loss
concealment and bandwidth estimation for free, and no media through the
server. Rejected because a mesh between friends behind home routers needs
ICE with STUN at least and TURN for the pairs that cannot connect, which is
a second service — and a TURN relay is this decision with more parts. A
Rust WebRTC stack is either async, which ADR 0030 kept out of a synchronous
codebase, or sans-IO with the socket loop and keying written by hand. And
building ffmpeg for MSVC would have been the hardest dependency this
project ever took, for an encoder openh264 replaces at these sizes. Each
sender would also upload once per watcher until the SFU arrived; here the
lobby is the SFU from the first build.

**Raw or losslessly compressed frames, no codec.** Exact pixels and no
dependency. A zlib'd NES frame is 5–15 KB, so sixty a second is 2–7 Mbit/s
per stream: fine on a LAN, too much for a home uplink. Kept as the fallback
if openh264 ever fails to build somewhere.

**The webview encodes.** `canvas.captureStream` and a browser peer
connection for free. Rejected because every frame would cross Tauri's IPC
into the page, which ADR 0027 rejected for the display path and which is
still a copy here, and it keeps the mesh's NAT problem.

**Opus for the sound.** Better at a quarter of ADPCM's bitrate. Every Rust
binding builds libopus with cmake, which is not here. Deferred, not
rejected: the `Audio` message names its codec by its shape, and a second
codec is a new field, not a new message.

**No sound.** 0003 never mentioned it. A game you cannot hear is a worse
thing to watch, and the cost was a tap on the audio callback and a short
coder.

## Consequences

**One transport, one version check, one state machine.** Watching is
tested on `session-fake` in-process like everything else, and over the wire
in `server-net`. Solo play carries the same code and never uses it.

**TCP, so a lost packet stalls what follows it** until it is resent. At
these bitrates that is a hitch, not a freeze; the sender's dropped pictures
and the page's lead absorb most of it. WebRTC would have traded quality
instead.

**The server carries the media.** Each stream enters once and leaves once
per watcher. Eight friends with one player still running is seven copies of
about a megabit, which a small machine carries; a server hosting many
lobbies would notice.

**The lobby thread is in the media path.** Sixty small messages a second
per sender pass through the thread that judges. Trivial here; shared.

**A player's picture leaves their machine.** Only to the lobby they joined,
only while they are in a round somebody else has finished, and never the
ROM (ADR 0013/0020). It is also the only outside view of a run (ADR 0009).

**Open door.** If a public server ever hosts many lobbies, `Watch`, `Video`
and `Watched` are the signalling a media path would need; moving the bytes
off the protocol socket is an addition behind these messages, and a new ADR.
