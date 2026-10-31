# 0033. Terminate TLS in front of the server, and let the client dial wss://

Date: 2026-10-31
Status: Accepted

## Context

ADR 0030 carries the protocol over `ws://` and leaves undecided where a
public server runs and how it is reached safely. A server somewhere other
than a LAN needs TLS: the socket carries players' names, and now their
pictures and sound (ADR 0032). The scale is one small machine serving a few
lobbies.

## Decision

**`gamertrials-server` speaks plain `ws://` and never holds a certificate.**
Whatever stands in front of it ends TLS and proxies the socket: Caddy,
nginx, a host's own load balancer. The server listens on `127.0.0.1:7878`
there and is not reachable from outside except through the proxy.

**The client dials `wss://`.** A server is named `[ws://|wss://]host[:port]`
in Settings › Profile and in an invite; `wss://` means TLS, verified by the
platform's own roots (SChannel on Windows, through `tungstenite`'s
`native-tls`), and its default port is 443. An invite from such a server
reads `wss://games.example.com/CODE`; the scheme is written only when it is
TLS, the port only when it is not the default.

`docs/server.md` is the runbook.

## Alternatives considered

**TLS in `gamertrials-server` itself.** One process, no proxy. Rejected:
it means certificate files and their renewal on a process whose job is a
state machine, and `rustls` or `native-tls` in a crate held to ADR 0024's
dependency fence. A proxy renews certificates for a living.

**`ws://` over a VPN or SSH tunnel.** Works for friends and needs no change.
Rejected as the answer, kept as a way: the address is still
`host:port`, and nothing here stops it.

**Authentication.** Not decided here. A lobby code is the only secret, and
it is five letters. A public server that strangers can reach wants more,
and that is its own ADR when there is such a server.

## Consequences

**The two-machine test ADR 0013 waits on can now run across the internet**,
not only a LAN. It has not run yet; `docs/server.md` says how.

**`app-client` links `native-tls`.** No new build dependency on Windows.
`server-net` is unchanged and has no TLS feature, deliberately.

**An invite carries a scheme.** `parse_invite` splits on the last `/`, so
`wss://host/CODE` reads as before; a bare code joins on the profile's
server, which may itself be `wss://`.
