# 0009. Server owns challenge sequencing and judging; run it in-process for solo play

Date: 2026-10-24
Status: Accepted

## Context

This was decided early and left unwritten, with the index title "Server owns
challenge sequencing". Building `challenge-core` made the gap visible: it deals
a session, judges a run and ranks the players of one challenge, and as a
library it says nothing about *where* any of that happens. Read from the
harness — the only thing that links it today — it looks like client-side code,
and the obvious next step would be one player hosting: dealing for everyone,
collecting everyone's results and judging them, including their own.

Three things are true of every multiplayer session:

* Every player must be dealt the same challenges, with the same seeds, or the
  scores are not comparable.
* Every player's run must be judged by the same rules, against the same
  `ChallengeSpec`, or the places are not comparable.
* The facts a run is judged on — `SessionEvent`s — can only come from the
  machine running the game. The server never runs one.

## Decision

**The server is the authority on what is played and what it was worth.** It
deals the session (`challenge_core::deal`), issues each generated challenge's
seed (ADR 0013), compares the artifact hashes clients report and aborts on a
mismatch, applies the session's bound and forfeits (ADR 0023), judges each run
from the `SessionEvent`s its client streams (`challenge_core::Run`), places the
players of each challenge (`challenge_core::places`), and totals the session.

**The client prepares, runs and reports.** It derives the stage's bytes
(`session-libretro::prep`, `content-gen`), drives the `StageRunner`, and streams
every `SessionEvent` with its frame. It decides nothing that reaches a
scoreboard.

**The client also judges, provisionally.** It feeds the same events through its
own `Run`, because what happens next — re-enter a time trial, or end a survival
run and spectate — is needed on the frame the attempt ends, not a round trip
later. Frames are emulated (ADR 0005), so the wait would not change a score, but
it would stall the player at every death. The client's verdict is a prediction
of the server's and is never reported as a result.

**The two cannot disagree without a bug or version skew.** `challenge-core` is
pure, and both sides give it the same events and the same `ChallengeSpec`. A
disagreement is treated like an artifact hash mismatch: it fails loudly rather
than being resolved in either side's favour. The challenge library a session
runs on is therefore something the server and every client must agree on before
it starts, in the same way they agree on a seed.

**Solo play runs the server's half in-process.** There is no remote server and
no special path: the client links the same crate and plays both roles. A rule
that only holds when a server is present is a rule that solo play breaks.

**No player hosts.** A player's machine is never the authority for anyone else's
run.

## Alternatives considered

**One player hosts.** Their client deals, collects and judges for everyone.
Rejected because the host is a party to the result it judges, the session dies
when the host leaves, and the host's version of the challenge library silently
becomes everyone's. It also gives the server nothing to do but relay, which is
the next alternative.

**The server relays and every client judges.** Each client computes every
player's result from the relayed events. Rejected because N independent
verdicts need reconciling, and reconciling them is judging — there must be one
party whose answer is the answer.

**The server judges and the client waits for it.** The simplest reading of
"server-authoritative". Rejected for the stall at every death described above,
and because solo play would then need a server round trip that does not exist.

## Consequences

**The server needs the challenge library and nothing else from a manifest.** It
deals and judges from `[[challenge]]` and the kinds and ids of the stages they
name; it never reads `[watch]`, `[[rule]]`, `[runner]` or a ROM. Today
`[[challenge]]` is parsed by `session-libretro`, which carries an emulator, and
the server must not depend on it. The crate split ADR 0017 owes is therefore a
prerequisite of the server, not a tidy-up.

**The server has to be able to link `challenge-core`.** Two implementations of
the deal or the judge would drift, and the deal's generator becomes a wire
contract the moment two machines deal and compare. That decides the server's
language; see ADR 0024.

**Server authority is not anti-cheat.** The client produces the facts from its
own RAM, and a client that lies about them is believed either way. Authority
here buys agreement, not honesty. Spectating (ADR 0003) is the only outside
view of a run.

**ADR 0013 rests on this unchanged.** The seed is part of the assignment the
server already owns.
