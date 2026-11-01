# Input replication: the spike

Branch `input-replication`, cut from `server-relay` on 2026-10-31.

ADR 0003 rejected spectating by replicated input because two machines'
emulators can diverge silently. ADR 0034 noted that Doom being emulated (ADR
0018) removes one of 0003's two objections and left the other — divergence —
to be measured. This is that measurement. Nothing here changes how a match is
spectated; it is a harness feature and a set of results, and any decision
waits on the hand tests below and a new ADR.

## What was built

The developer harness (`cargo run --release -p session-libretro --bin
harness`) records and replays. `crates/session-libretro/src/replay.rs` holds
it.

- **`--record <file>`** writes, for every frame, what the core could read —
  the pad mask, the key edges in order, the latched mouse — and a digest of the
  machine after it: each writable memory region the core publishes, the
  manifest's scan window (where a game's own variables were found: Doom's
  player struct and globals), the picture, the sound, and the telemetry events
  the manifest's rules emitted. Every entry and re-entry is marked. In the
  window you play; with `--frames N --script SEED` the harness presses buttons
  at random instead, so nobody has to.
- **`--replay <file>`** plays the inputs into a fresh core, headless, and
  reports per attempt the first frame each part stopped matching. The game,
  stage, challenge, seed, settle and warm-up come from the recording. It
  refuses a recording made on other content or from another entry state, and
  exits 2 if anything diverged.
- **`--passes N`** replays it N times in one process. Pass 1 starts like the
  recording did; later passes start on a core that has just played, as a
  spectator's has.
- **`--settle MS`** and **`--warmup N`** exist because of what dosbox-pure
  turned out to do (below).

A recording is a text file, one line a frame, about 150 bytes a frame for Doom
(the digests; the input alone is a few bytes).

## Results so far

One Windows 11 machine, and the same recordings replayed under Ubuntu 22.04 in
WSL with the `.so` builds of the same core versions (ADR 0035) — a different
compiler and OS, not a different CPU. Scripted input, 6000 frames per console
run and 3500–4200 per Doom run, with deaths and re-entries.

| Game (core) | Replayed as recorded | On a core with a history | Linux replaying Windows |
|---|---|---|---|
| SMB3 (Mesen) | every frame, every part, 5 attempts | the same | every frame, every part, both passes |
| DKC (Snes9x) | every frame, every part, 3 attempts | the same | every frame, every part, both passes |
| Doom (dosbox-pure), driven as the client drives it | diverges from frame 1 | diverges from frame 1 | — |
| Doom, settled and warmed | every frame, every part (one run had one frame of game-variable mismatch at frame 1240, then matched again) | picture and events on every frame but the first; sound and sound-side memory differ throughout | picture, sound and events on every frame; memory differs on 14 frames of one run and most frames of the other (game-variable window: 1 and 53 frames) |

To reproduce on one machine:

```sh
harness --game smb3 --challenge 1-1-time-trial --frames 6000 --script 7 --record smb3.txt
harness --replay smb3.txt
harness --game doom --challenge e1m1-survival --frames 4200 --script 9 --settle 5 --warmup 120 --record doom.txt
harness --replay doom.txt --passes 3
```

## What it found

**The console cores are exact.** Mesen and Snes9x replay frame for frame —
RAM, picture, sound and events — from a fresh core and from one that has just
played the same stage, across every death and re-entry. Altering one input in
a recording is reported at exactly that frame, and the next attempt is back in
step: an entry restores the same bytes on both sides, so a divergence cannot
outlive the attempt it happened in.

**dosbox-pure is not deterministic the way the client drives it.** Its
`retro_run` (1.0-preview6 source; preview7 behaves the same) reads the input,
then waits for the frame in flight, then sets the next frame running on its own
thread *before returning*. The thread takes queued input whenever it next
looks, which is mid-frame, so where a key lands in emulated time depends on how
far the thread got before the key arrived. Memory read after `retro_run` is
read while it changes: hashing it twice with no frame between gave two
answers, every frame. Two identical scripted recordings differed from the first
frame. Its `rewind` save-state mode changes none of this, and preview7 has no
latency option.

**Waiting a frame's work out restores it, given the same history.** With the
harness sleeping 5 ms after each frame (`--settle`; Doom's frame is ~1.3 ms of
host time) the thread is parked at the frame boundary whenever input arrives
or memory is read, and two recordings came out byte-identical. A paced client
gets most of this for free — at 70 Hz a frame's turn is ~14 ms — but not by
guarantee.

**A restore during dosbox-pure's boot is a race too.** `LibretroRunner::start`
spends one frame before anything is loaded, and the core boots on after it
returns. A state restored while it is still in `DBPSTATE_BOOT` is restored
without pausing the thread, and the stage then starts one frame further on or
not, by timing: a replay that reached its first entry sooner than its
recording ran exactly one frame ahead from the start. Waiting 50 ms before the
first entry was enough; the harness waits 250.

**A restore does not reset the sound pipeline.** dosbox-pure's state leaves out
part of the mixer (buffer position, channel state, the remainder it carries
between frames). Replaying with one frame more warm-up than the recording had
was enough for the sound — and the memory Doom's sound code writes, inside the
scan window too — to differ on nearly every frame, while the picture and every
telemetry event matched on every frame but the first after the entry. A
spectator's core always has a different history from the player's, so this is
the case that matters: for Doom, the watcher would see the same game and hear
slightly different sound. Whether that is audible is not measured. A DOS game
whose logic times itself off sound interrupts would not be so lucky.

**Across platforms it holds as well as on one machine.** The Linux builds of
the same core versions are different binaries from a different compiler, and
replayed the Windows recordings with the same results — the console runs
exactly, Doom's picture, sound and events on every frame. Doom's memory
differed in places that never reached the screen or the facts: on one run for
14 frames, on the other for nearly all of them. What those bytes are is not
known; host-side state such as file timestamps in DOS's buffers is a guess.

## Consequences outside the spike

Found on the way, true on `server-relay` today, and not fixed here:

- **Doom's telemetry reads memory while the core is running.** The watcher
  reads RAM straight after `retro_run`, which for dosbox-pure is while the next
  frame is being emulated. An event's frame number can therefore move by a frame
  with host timing, and a fact that holds for a single frame could in principle
  be missed. ADR 0018's first criterion — deterministic frame stepping — holds
  for dosbox-pure only when the frontend lets the thread park.
- **The first entry of a Doom round races the core's boot**, as above. The
  client restores the entry state right after `LibretroRunner::start`.
- **`GET_THROTTLE_STATE` is answered by writing a `bool`** into what is a
  `retro_throttle_state { unsigned mode; float rate; }`. dosbox-pure reads the
  rate; it happens to stay zero, which it treats as "the core's own rate".
  Harmless by luck.
- **dosbox-pure hangs at teardown on Linux.** Every Doom replay under WSL
  finished and printed its report, then never exited: both threads waiting on
  futexes. Not seen on Windows. Whether the client hits it when a round
  unloads the core is not checked.

## What it does not settle

The hand tests, in order:

1. **Two real machines.** Package a build (`tools\package.ps1` now includes
   `harness.exe` and this file), make a recording on one machine with the
   commands above, copy the `.txt` to the other, and run `harness --replay
   <file> --passes 2` from the package folder. Both need the same ROMs; the
   replay refuses otherwise.
2. **A played recording.** `harness --game doom --challenge e1m1-time-trial
   --record doom-play.txt` and play it in the window. The digest of each frame
   is taken at the next frame's turn, when the core has parked; the input is
   whatever the 70 Hz pacing delivered. Replay it on the same machine and on
   the other. This answers whether a paced client is settled often enough in
   practice, which no scripted run can.
3. **Sound.** Whether the sound a history-laden Doom replay makes is audibly
   different. Needs a replay that plays aloud, which the harness does not do.

If replay is pursued, it needs a new ADR, and the dosbox-pure findings decide
its shape: the console cores could replay as they are, but dosbox-pure needs
either a patched core (poll input after the frame in flight finishes, and
serialize the mixer whole) or a frontend that settles every frame on both
ends — and desync detection on the facts and the picture, not on whole memory.
