# 0030. Carry the protocol over WebSockets to a separate server process

Date: 2026-10-29
Status: Accepted

## Context

`server::Server` is a state machine over `protocol` messages with no transport
(ADR 0024). Solo play drives it over an in-process channel. A lobby of several
needs a socket in front of it, and ADR 0024 left three things for the server's
first real build: the async runtime, the transport and the SQLite binding.
Join game in the client sketch takes an invite code, so a lobby also needs a
name that a player can paste.

The scale is 4–8 friends per lobby. The state machine is synchronous: it takes
one message and returns what to send.

## Decision

**WebSockets, through synchronous `tungstenite`, with threads.** The crate is
`server-net`, and its binary is `gamertrials-server`.

* One thread accepts connections.
* One thread per connection reads text frames and writes replies. Each frame
  is one `protocol` packet, so the version check (CLAUDE.md) runs on every
  message.
* One thread per lobby owns its `Server`, takes what its connections read and
  routes each reply in the `Outbox` to the connection it names.

No async runtime: nothing here waits on more than one socket, and the state
machine would sit behind a lock in any runtime.

**The server is always its own process.** Host game on a client with a server
set in its profile opens a lobby there. Host game with none set runs the
server's half in-process for a lobby of one, as before. A client never listens
for other players. That would be ADR 0009's rejected "one player hosts" by
another route.

**`Hello.lobby` routes a connection.** `None` opens a lobby; a code joins one.
A code is five letters with no I or O, read without regard to case. An
unknown code is refused and the connection closed. The library hash is still
checked by the lobby, as before. A lobby ends when its last connection does,
and nothing is stored.

**An invite is the server's address and the code**: `host[:port]/CODE`, with
the port left off when it is 7878. A bare code joins on the server in the
player's profile.

**`LobbyState.in_match`** tells a guest when the host has gone back to the
lobby, so every table closes together.

## Alternatives considered

**tokio with axum or tokio-tungstenite.** It is the conventional choice and
has more headroom. Rejected for now: it brings a second concurrency model into
a codebase that is synchronous everywhere else, for a load threads carry
easily. The lobby actor is the seam if it is ever needed. Moving the
connection threads to tasks changes nothing behind `LobbyMsg`.

**The host's client listens and runs the server in-process for everyone.**
This needs no deployment and no NAT story beyond the host's. Rejected because
it is ADR 0009's rejected alternative. The host's machine becomes the authority
on everyone's run, and the lobby dies when the host leaves.

**Codes without the address**, resolved through a directory server. It gives
a shorter invite. Rejected while there is one server: a directory is a second
service to run, for a string that can be pasted anyway.

**Clock synchronisation.** CLAUDE.md lists it among the server's jobs. It is
deliberately not built. Scores are emulated frames (ADR 0005), so a `Go` that
reaches two players one round trip apart changes only who sees the countdown
first, never a score.

## Consequences

**A player leaving mid-match ends it for everyone.** This is the state
machine's existing rule ("a run with nobody to report it cannot be judged"),
and now it can happen. A guest's Leave match, or a guest closing the game
window, leaves the lobby. The host's End match ends it instead.

**ADR 0013 now holds for several players over a socket.** The lobby issues one
seed to every player and compares their artifact hashes. It stays `Proposed`
until that has run on two machines rather than two clients on one.

**Not decided here:** where a public server runs, TLS (`ws://` only today),
and the SQLite binding. Spectating (ADR 0003) needs a media path this does not
provide. Whatever signalling it needs can ride these sockets.
