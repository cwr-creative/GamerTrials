# Running a server

`gamertrials-server` is one binary with no state: lobbies by invite code,
each a `server::Server` (ADR 0030). It reads the challenge half of the
manifests and nothing else, so a player can only join a server whose
`manifests/` hash the same as their own; the lobby refuses the rest and says
which hash it holds.

```sh
cargo build --release -p server-net
target/release/gamertrials-server --listen 0.0.0.0:7878 --manifests manifests
```

That is a LAN server. Players put `host` or `host:port` in Settings ›
Profile, or paste the lobby's invite, `host[:port]/CODE`.

## Beyond the LAN: behind TLS

The server never holds a certificate (ADR 0033). Put a proxy in front of
it that does, and let the server listen on loopback only:

```sh
gamertrials-server --listen 127.0.0.1:7878 --manifests /srv/gamertrials/manifests
```

With [Caddy](https://caddyserver.com), the whole `Caddyfile` is:

```
games.example.com {
    reverse_proxy localhost:7878
}
```

Caddy gets and renews the certificate itself and upgrades the WebSocket
through. Open 443 (and 80, for the certificate challenge) on the host's
firewall and nothing else. nginx wants `proxy_http_version 1.1` and the
`Upgrade`/`Connection` headers passed; its own documentation covers it.

Players then set `wss://games.example.com` as their server, and invites
read `wss://games.example.com/CODE`. A port other than 443 is written as
`wss://games.example.com:8443`.

Keep the server running with whatever the host offers — a `systemd` unit
with `Restart=always` is enough; it holds nothing between lobbies.

## What to expect

- A lobby lives as long as one connection does. Nothing is stored.
- A player leaving mid-match ends the match for everyone (ADR 0030).
- Each spectated run costs the server about a megabit per watcher in and
  out (ADR 0032). A small virtual machine serves a few lobbies of friends.
- The server prints each lobby's code and seed as it opens, and each
  connection's reason as it closes. There is no other log.

## Checking it from two machines

This is the test ADR 0013 waits on. On one machine host a match with the
server set to the public address; on another join by the invite. A
generated challenge must prepare on both with the same artifact hash — the
server ends the match loudly if it does not — and a finished player must
see the other's picture and hear its sound (ADR 0032).
