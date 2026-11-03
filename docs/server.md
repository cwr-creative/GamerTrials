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

The same port is also opened for UDP, for head-to-head (ADR 0036): a
seated player sends a few probes to it so the server learns the address
their machine reaches the world from, and tells the other slot. Nothing
else crosses it — the two players then talk to each other directly. `--udp
<port>` moves it, `--udp off` deals no bouts, and on a host whose firewall
is open for the listen port, open it for UDP too.

The players' own machines talk on their **bout port**, UDP 7879 unless
Settings › Profile says otherwise. Two players behind ordinary home routers
usually find each other without help. A player behind a VPN or a carrier's
NAT usually cannot be reached, so the other side must be: forward
UDP 7879 on that player's router to their machine, and allow the client
through its firewall. A player who hosts the server and plays on the same
machine forwards both, 7878 and 7879. Two players who both cannot be reached
cannot play a bout; the round is voided (ADR 0029).

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

UDP does not pass through the proxy, so a server behind one that should
deal bouts listens for probes on its own address and says so: `--udp 7878
--udp-public games.example.com:7878`, with 7878/udp open on the host.
Without `--udp-public` a player would probe the proxy's host at the UDP
port, which answers nothing.

Keep the server running with whatever the host offers — a `systemd` unit
with `Restart=always` is enough; it holds nothing between lobbies.

## What to expect

- A lobby lives as long as one connection does. Nothing is stored.
- A player leaving mid-match ends the match for everyone (ADR 0030). So
  does one whose link drops without closing: each end pings every second
  and gives up on the other after 10 s with nothing heard
  (`protocol::Liveness`). A blip shorter than that is ridden out.
- Each spectated run costs the server about 0.6 Mbit/s per watcher in and
  out (ADR 0034). A small virtual machine serves a few lobbies of friends.
- A link too slow for that loses pictures, not the match: each connection
  sends the match first and drops a stream it cannot deliver fresh, and a
  watcher whose picture cannot keep up is shown the run instead (ADR 0034).
- The server prints each lobby's code and seed as it opens, and each
  connection's reason as it closes. While a stream flows, each connection
  carrying it logs a line every five seconds: kbit/s sent, pictures and
  sound sent and dropped, and the link's round trip. A client's log has the
  same line for its own uplink, as `[link]`.

## Checking it from two machines

`tools\package.ps1` builds what the other machine needs: a folder and zip
under `dist\` holding the client, the server, the manifests, the cores and
the states, but no ROMs — `assets\ROMs\README.txt` inside it lists each
dump, where it goes and its sha1. Package from a clean commit, so its name
says which build every player is on.

This is the test ADR 0013 was accepted on. On one machine host a match with the
server set to the public address; on another join by the invite. A
generated challenge must prepare on both with the same artifact hash — the
server ends the match loudly if it does not — and a finished player must
see the other's picture and hear its sound (ADR 0032).
