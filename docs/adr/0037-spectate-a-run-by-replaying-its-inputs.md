# 0037. Spectate a run by replaying its inputs, superseding the picture stream of 0034 for runs

Date: 2026-11-02
Status: Proposed

On the `input-replication` branch only; `main` keeps ADR 0034's stream.

## Context

ADR 0003 rejected replicating inputs because two machines' emulators could
diverge silently, and because Doom was not emulated. ADR 0032 restated it and
ADR 0034 kept the relay after a match over the internet, noting that the
replay spike (`docs/replay-spike.md`) had measured the question since.

What the spike measured: Mesen and Snes9x replay a run exactly, frame for
frame, from a fresh core and from one that has just played, across deaths,
re-entries and from Windows to Linux. dosbox-pure did not, as the client drove
it; the fixes the spike found — let its thread finish a frame before reading,
restoring or running the next, and hand it keys at the frame boundary — are on
`main` for every player, and since them Doom replays its picture and its facts
exactly too. Its sound does not, from a core with a history, because a restore
leaves part of its mixer as it was.

The picture stream costs a watched player half a megabit of uplink, an
encoder on a thread, a picture the watcher's settings never touch, and on a
slow link the run panel instead of the game. A run's inputs are a few bytes a
frame.

## Decision

**A run is spectated by replaying it.** The watcher's client runs the same
stage on its own core, in its own game window, from the watched player's
inputs, and the overlay shows only the bar, the clock and the run panel over
it. A bout keeps ADR 0034's picture stream until a shared machine can be
replayed, which needs every slot's input.

**Every run sends its inputs, watched or not.** The client sends
`ToServer::Inputs` in batches of a few frames on the control lane — never
dropped, since a lost frame is a different game: the frames' input as the
adapter encodes it (opaque everywhere else, invariant 3), the attempt they
belong to, and a checksum of the picture every 30 frames. A sender no longer
needs to know it is watched.

**The server keeps the current attempt.** It holds each seat's inputs since
its attempt began, so a watcher who arrives late is sent that backlog and then
every batch as it comes (`ToClient::Inputs`), without the player being asked
for anything. An attempt starts from the stage's entry state, so the backlog
never needs to reach further back.

**The watcher prepares the round itself.** A player whose run is over replays
on the core they played on; one eliminated before the round derives its
content from the deal, as a player would, and compares its artifact with the
one the server holds for the watched seat. Content it cannot prepare means the
run panel alone.

**It replays a little behind and catches up as fast as the core goes.** A
watcher arriving mid-attempt runs the backlog without pacing, picture or sound,
then plays live a few frames behind the newest it has, and holds rather than
guesses when the input runs dry.

**A desync is shown, not hidden.** The replica checks its picture against each
checksum; on a mismatch it says so and gives the view to the run panel until
the next attempt, whose entry restores the same bytes on both sides.

## Alternatives considered

**Keep the picture stream** (ADR 0034, and `main`). It works for every core
and every kind, today. It is what this decision is measured against, not
rejected for good; `main` keeps it.

**Send inputs only while watched**, and the whole attempt when a watcher
arrives. Keeps a player's uplink idle, but makes every arrival a request to
the player and a burst on their link, for a stream that costs a few kilobits.

**Catch up from a savestate** the sender captures on request. Tried, for
the one core that needs it, and measured in the harness (`--save-state-at`,
`--resume-state`): a state taken mid-attempt at a frame boundary and restored
on another process carries Snes9x on exactly and Mesen exactly but for 120
frames of sound, but not dosbox-pure. Its saved state leaves out what it keeps
on input — keys and buttons down, which it needs to pass a release on — and
part of its mixer. The first is mended by giving the replica the player's
held input and running one throwaway frame before the restore; the second is
not: the restored Doom plays the same game until the first sound after the
restore — an item picked up — and from that frame looks elsewhere, on the
same frame whether the state was taken at 1500 or 2200, with or without the
mouse, at any settle. Doom's sound library times itself off the sound card,
which is this ADR's warning come true. The 1.4 MB a Doom state compresses to
would also be 16 s of a phone's uplink. Replaying the attempt from its entry
state uses what both sides already hold, and is exact.

**Check memory, not the picture.** The spike's whole-memory digests showed
dosbox-pure's sound-side memory differing while the game did not; the picture
is what a watcher sees, and its checksum is a few bytes.

## Consequences

**A watched run costs its player nothing more than an unwatched one**: a few
kilobits up, always, and no encoder. The watcher sees the game at their own
settings and hears it from their own core.

**A watcher needs the game's content**, as a player does. A late watcher waits
while the attempt replays: seconds on a console, longer on Doom, whose 10 ms
settle bounds it near 100 frames a second against the run's 70 — measured, a
15 s backlog took 27 s, and one of two minutes would not be caught before the
round ends. Only dosbox-pure itself can lift that, and on this branch it does:
`cores/dos` is dosbox-pure 1.0-preview7 patched to finish the frame in flight
before it reads input (`tools/dosbox-pure/`), the manifests say
`input_at_boundary`, and a catch-up runs unsettled. Measured in the harness,
4200 frames of Doom replay exactly at ~800 frames a second on Windows and
~720 on the Linux build of the same patch, against ~97; in the client, a
watcher who gave up 15 s in caught up the attempt's 1040 frames in about
1.3 s and stayed live, every picture check passing. The cost is a core
built here rather than taken from the buildbot, on both platforms at once.

**Doom's sound may differ** for a watcher, and every core this project adds
must replay its picture exactly to be spectated, which the spike's harness
(`--record`, `--replay`) is how to check.

**Protocol version 6**: `Inputs` both ways. `Video`, `Audio` and `Watched`
stay, for bouts.
