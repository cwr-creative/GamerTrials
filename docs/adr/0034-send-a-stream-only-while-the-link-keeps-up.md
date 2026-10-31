# 0034. Send a spectator's stream only while the link keeps up, and show the run when it cannot

Date: 2026-10-31
Status: Accepted

## Context

The first match between two machines (`docs/server.md`) put one player on a
phone's tethered link: 0.2 Mbit/s down, 0.7 up. ADR 0032's stream was about
1.9 Mbit/s once base64 was counted — a megabit of picture and 48 kHz stereo
ADPCM — and nothing in its path could shed load. The sender's channel, the
lobby's channel to each watcher and the socket buffers all queued without
bound, so the picture fell further behind the longer it ran: a 30-second run
took three minutes to watch. The match messages the server judges from rode
the same first-in-first-out socket behind it, the pings with them, and every
watched round ended in a timeout. Two clients on one machine had never shown
any of this: loopback has no limit to exceed.

A link that slow is ordinary — a phone, a hotel, a shared house in the
evening — and the stream is the one thing on the socket that can be lost
without losing the match.

## Decision

**ADR 0032's decision stands; its sending rules do not.** The stream is still
protocol messages relayed by the server and decoded by the overlay page.

**One socket loop, both ends.** The client's link and each of the server's
connections run `transport::run`, so a slow link is handled one way in both
directions. The socket is non-blocking with a 16 KiB send buffer, and reads
everything waiting before it writes, so a ping is never stuck behind this
side's own output.

**Two lanes.** Control — every message but a picture or a sound — is sent
first and never dropped. Stream waits in the loop's own queue, where it can
still be dropped, until everything written before it has left; anything older
than 400 ms goes unsent. Once a picture is dropped, every later one is too
until a key frame, since none decodes without it, and the sender is asked to
start one. The server cannot ask for one on a watcher's behalf, so the
sender also makes one every two seconds.

**The link is timed, not trusted.** A phone network holds seconds of data in
buffers no socket sees, so the send buffer filling is not enough to notice.
While a stream flows the loop pings every 100 ms, each ping carrying the
moment it left; a round trip longer than the shortest seen is time spent in a
queue, and while that exceeds 250 ms — or a ping has been unanswered that
long — the stream waits, and what is queued for it is cut to the newest
100 ms so the link does not reopen on a burst. Pings for liveness come every
second.

**A smaller stream.** Picture at 0.5 Mbit/s; sound mixed to mono near 16 kHz
(64 kbit/s); and protocol version 4 carries both payloads in a binary frame
after their packet, sparing base64's third. About 0.6 Mbit/s in all.

**A watcher sees the run when the picture cannot reach them.** The server
relays each `Frame` a watched player reports as `ToClient::Progress`: the
attempt and the clock as the server judged them, and the events in a few
words on the page. A watched player reports a frame twice a second besides
its events. Below 8 painted pictures a second over three seconds the overlay
dims the picture, says the connection is too slow for live video, and shows
the run instead; it hands back only above 15, so a link at the edge does not
flicker.

## Alternatives considered

**A bounded queue alone.** Dropping the oldest when a queue is full bounds
memory, not latency: the queue in the phone network is outside it, and fills
first.

**Adapting the encoder's bitrate to the slowest watcher.** One encoder serves
every watcher, so it would serve all of them at the worst one's rate. Dropping
per connection lets each watcher get what their own link carries.

**Input replay** (ADR 0003's rejected option). It costs bytes, not megabits,
and Doom is now emulated (ADR 0018), which removes one of 0003's two
objections. The other — two machines' emulators diverging silently — still
stands, and replay would replace this stream rather than rescue it on a slow
link. It is being explored separately; this decision does not depend on it.

**A second socket for the stream.** Control could never queue behind a
picture. But a second socket competes for the same slow link, doubles the
connection handling on both ends, and still needs everything above to keep
its own queue short.

## Consequences

**The match survives any link that carries its control.** `server-net`'s
`slow_link` tests hold a player to 0.2 down and 0.4 up through a throttling
relay: every progress report reaches the watcher within about 1.3 s, the
pictures that arrive are under about 1.6 s old, and the round completes.

**A slow watcher sees about 12 pictures a second** on a phone's downlink, or
the run panel; on a home link, the whole stream.

**The loop polls.** An idle connection sleeps 2 ms between looks, which is
the latency it adds and a little CPU per connection. Fine for lobbies of
friends.

**Protocol version 4.** A version-3 client is refused at hello, as any other.

**Two measured numbers are tuning, not law:** the 250 ms queue and the 16 KiB
send buffer. The buffer limits a single connection to about 0.9 Mbit/s at a
150 ms round trip, which the stream fits under. The stream's own log line —
`stream … kbit/s, pictures … sent … dropped, round trip … ms`, every five
seconds while it flows — is what to read when either needs changing.
