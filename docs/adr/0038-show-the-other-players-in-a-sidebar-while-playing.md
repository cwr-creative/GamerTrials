# 0038. Show the other players in a sidebar while playing, superseding 0026 on spectating

Date: 2026-11-03
Status: Proposed

0037 is taken on the `input-replication` branch, which spectates by replaying
inputs; this is `main`'s, and keeps ADR 0034's picture stream.

## Context

A player sees nobody else until their own run is over (ADR 0032). In a
race, that is the moment it stops mattering: while playing they cannot tell
whether they are ahead, level, or the last one still going.

What kept it so was ADR 0026's rule that spectating must not leak a
generated layout: only a finished or eliminated player watches, so nobody
sees a rolled stage before they have played it. That rule was written in
error. The game is competitive, but it is a party game among friends, and a
player who glances at a rival further into the same rolled stage has learned
about as much as one who heard them shout about it across the room.

The picture stream is already there for every core and every kind: openh264
on the sender, the server as the relay, `VideoDecoder` in the overlay. What
it costs is bandwidth, which on the links players have — a router, by wire
or wifi — is room to spare at a retro picture's size.

## Decision

**ADR 0026's spectating paragraph is withdrawn.** Seeing ahead is allowed.
`Watch` is still refused to a player still playing, now for another reason:
their own game is the main view, and the sidebar is how they see the rest.

**The game sits at the far left, and a sidebar on the right holds up to
three thumbnails**, stacked, each labelled with its player's name. The
sidebar is four ninths of the window's height wide — three 4:3 boxes on top
of one another — and never more than two fifths of its width; a framed
window grows by it. It is there for the whole of every round of a match with
more than one player, whoever is left to show, so the game never moves
mid-run. Not in solo play, and not in a head-to-head bout, whose seats share
one machine and one picture. A finished or eliminated player's spectate
view takes the left side, and the sidebar stays.

**Only players still playing are shown**, never this player, and never the
one this player is watching full-size. With more than three, which three is
drawn at random; one who finishes, gives up, dies out of a survival run or
is otherwise done leaves the sidebar, and a random one of the rest not shown
takes the box. A thumbnail is picture only.

**A thumbnail is a second stream from the same sender.** `ToServer::Thumbs`
names the players whose thumbnails a client wants this round, at most three,
replacing the last set. The server tells each player how many want theirs
(`Watched.thumbs`, beside `watchers`), and while that is above zero the
sender runs a second encoder at the thumbnail bitrate, at the full frame
rate, and sends `Video` with `thumb` set. The server relays it to whoever
asked, once in from the sender and once out per viewer, as ADR 0032 does the
full stream.

**The two streams break separately.** ADR 0034 drops a picture stream until
its next key frame; with several streams on one socket that has to be per
stream, or one dropped thumbnail makes every other stream wait for a key of
its own. `Lane::Picture` carries which stream a picture belongs to, the
transport tracks each, and a sender asked for a key knows which encoder to
ask.

**Settings › Profile has a second tunable**, the thumbnail bitrate: 250
kbit/s unless the player changes it, 50 to 2000. Each player sets what they
send, as with the stream.

**Protocol version 6**: `Thumbs`, `Video.thumb` both ways, `Watched.thumbs`.
The `input-replication` branch's version 6 is another protocol; whichever
lands second renumbers.

## Alternatives considered

**Halving the frame rate, or the picture, for a thumbnail.** Cheaper on
every link and on the encoder. Not yet: it is tried at the full rate first,
and the bitrate is the one tunable until that is seen to be too much.

**The full stream, scaled down on the page.** One encoder per sender, and
the thumbnail sharpens to a full view with no new key frame. Rejected for
bandwidth: three full streams down is 1.5 Mbit/s of pictures a player sees
at a fraction of their size.

**Replaying inputs on local cores** (the `input-replication` branch). Bytes
rather than megabits, but three cores more on the machine of a player who is
playing, beside their own, and the frontend runs one core per process
(`session-libretro`'s statics), so each replica would be a process of its
own. Kept on its branch.

**Progress only** (ADR 0034's run panel) in the sidebar. Free, and already
on the wire. A line of text does not show who is about to finish.

**The server choosing who is shown.** It knows who is playing. But the
choice is a matter of the page, and a client picking for itself needs no
rule on the server beyond who may be asked for.

## Consequences

**Every player uploads a second stream for their whole run** once anyone
wants their thumbnail, which in a match of three or more is always: about
the thumbnail bitrate, beside the full stream while someone watches them
full-size. And a second encode per frame, on a thread of its own.

**Every player downloads up to three thumbnails**: 0.75 Mbit/s at the
default, the full stream on top while spectating. The server sends each
player's thumbnail once per viewer — in a match of eight, 24 streams — which
a small machine carries for a lobby of friends.

**The overlay decodes up to four H.264 streams at once**, three of them at
the full frame rate. Not yet measured on a slow machine.

**The game's picture is no longer centred** in a match of several. A
player's video settings still decide its size within the space left to it.
