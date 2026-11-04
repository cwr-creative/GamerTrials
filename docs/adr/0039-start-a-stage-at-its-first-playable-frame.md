# 0039. Start a stage at its first playable frame

Date: 2026-11-04
Status: Proposed

## Context

An entry point is where a stage can be reached from, and that is not always
where it can be played. SMB3's generated challenges have to enter from the
World 1 map: the level loads fresh from whatever bytes the roll wrote, and a
state captured inside it would carry an index built from the unrolled list
(`docs/games/smb3.md`). A Doom bout cannot restore a state at all, because the
netpacket contract forbids it while connected, so it boots into IPXSETUP and
the game starts only once both machines have found each other.

So the frames a player was given did not start at the game. Measured on Bro
Roulette: frame 1 is the map, frames 30 to 40 are black, the palette fades in
to 56, input first does anything at 61, Mario is on screen at 62 and the four
enemies at 64. All of those frames were on the clock, as were IPX and the level
load against a bout's limit. And a restore draws nothing, so what a countdown
showed was whatever the core had drawn at power-on: flat green on the NES,
black on the SNES, a DOS prompt under dosbox-pure, on every state entry.

## Decision

**Frame 0 is the first frame of the stage a player can act on.** That is
`StageRunner::enter`'s contract now: on return the machine stands there, with
that frame's picture. It generalises what `boot_frames` already did, which
ADR 0005 is the reason for: how the machine got here is before the stage, and
is not in any count.

**A stage declares where that is, in its manifest.** `[[stage]].ready` is a
number of frames, or an edge on a watched field with a bound on the wait and an
optional number of frames after it. Nothing in `session-api`, `challenge-core`,
`protocol` or the server learned the word. An edge, not a level, because a byte
already at the value says nothing: DOS memory reads 0 before the game is loaded,
and 0 is also what `gamestate` reads in a level.

**A machine that can get there alone does, inside `enter`:** with nobody
touching it, sound dropped and the picture kept, its facts unobserved. A rollback
bout is one of these, since its machines run the same frames on the same input,
which is none. The result is kept in memory, so a death in a time trial restores
it rather than running the frames again.

**A state entry that declares nothing still has its picture drawn.** One silent
frame, and the same restore takes it back, so no count moves. A threaded core
shows the frame before the one it runs, so one frame shows it nothing, and such
a stage says `ready` instead.

**A relayed machine gets there on the network, and is then not held.** The
relay goes live when the hellos are answered rather than at `Go`, and the game
thread pumps it on the clock, behind the intro, with input released, until
`ready` fires; only then does it say `Prepared`, so the existing wait for every
seat is what holds the round. The manifest must name an edge for such a stage,
since no count can say when the network let the game start, and missing the
bound voids the round (ADR 0029). The machine then runs on through the
countdown, silent and with nobody's hands on it, and `Begin` makes the frame it
has reached frame 0.

## Alternatives considered

**Capture the state at the first playable frame.** The obvious fix, and what
every other stage does. It carries the unrolled index into a rolled level, which
is why these states are on the map; and a bout cannot restore one at all.

**Freeze a relayed machine at `ready`.** It would match the console path exactly
and is what the countdown seems to want. Doom's netcode is lockstep: a machine
that stops stops sending, and the other can stall short of the same frame with
nobody to answer its retransmit, until something times out. Letting it run costs
nothing while nobody can touch it, on a level with `-nomonsters`. A game whose
level moves while nobody plays would be seen moving through the countdown, and
would need thinking about then.

**Hide the frames behind the overlay and let the clock run.** The overlay could
cover them, but they would still be on the clock and against the limit, and
enemies would move behind a curtain for a time that depends on nothing the
player did.

**Count the frames and subtract them when scoring.** This pushes an adapter's
detail into the judge, across a boundary that is otherwise clean, on both client
and server, for a number nobody wants to see.

**A rule emitting `ready`.** `[[rule]]` emits facts about a run, and this is a
fact about the machine before there is one. `challenge-core` would have to be
taught to ignore it, and every rule would have to be taught not to fire first.

**Scripted input to walk in.** Not needed: the SMB3 states were captured with
the walk already under way. If a game ever needs a button pressed to reach its
first playable frame, this is where that would go.

## Consequences

**A death in a time trial is cheaper on these stages.** Bro Roulette's lead-in
was 64 frames an attempt and is now none, which is the same for everyone. Any
frame count recorded on such a stage before this is not comparable with one
after it. Doom's E1M1 moves by two.

**A contributor has a number to find.** Enter with `harness --no-ready`, compare
`--synctest --ram-log --ram-every 1` with `--idle-slots 0` against scripted runs
for where input first does anything, and look at the pictures for where there is
something to see. They are not always the same frame, and the second is the one
to choose. The numbers hold while the state and the hack do, and either changing
already means a new state.

**A bout takes as long to prepare as its game takes to start**, and the intro
stays up through it. The bound is the manifest's.

**A relayed bout's first frame is each machine's own `Begin`.** `Go` reaches two
players a round trip apart, and the same fact can still sit a frame apart across
machines, as it could before. Nothing here makes that worse, and no number here
comes from two machines over the internet.

**The relay holds a reliable packet for a machine not yet live**, because the
machines now go live a round trip apart while running, rather than a round trip
apart at `Go`. It was dropped before, and acknowledged.
