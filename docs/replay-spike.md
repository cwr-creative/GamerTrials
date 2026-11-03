# Input replication: the spike

> Since 2026-11-02 this branch spectates a run by replaying it, as ADR 0037
> decides; what follows is the spike that measured whether it could.

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
- **`--warmup N`** runs the stage idle before the first recorded entry, so a
  recording and its replay can start from the same history (below).
- **`--settle MS`** waits after every frame on top of what the manifest says.
  It was how the dosbox-pure findings were made; since the runner settles a
  threaded core itself (below) it is only for experiments.

A recording is a text file, one line a frame, about 150 bytes a frame for Doom
(the digests; the input alone is a few bytes).

## Results so far

One Windows 11 machine, and the same recordings replayed under Ubuntu 22.04 in
WSL with the `.so` builds of the same core versions (ADR 0035) — a different
compiler and OS, not a different CPU. Scripted input, 6000 frames per console
run and 4200 per Doom run, with deaths and re-entries. Doom's rows are on the
runner as fixed on `server-relay` (commits `4aa4c0f`, `fbfc65c`, `b82eae9`,
merged here), with no `--settle`.

| Game (core) | Replayed as recorded | On a core with a history | Linux replaying Windows |
|---|---|---|---|
| SMB3 (Mesen) | every frame, every part, 5 attempts | the same | every frame, every part, both passes |
| DKC (Snes9x) | every frame, every part, 3 attempts | the same | every frame, every part, both passes |
| Doom (dosbox-pure), on the runner before the fix | diverges from frame 1; three runs of one script were three different games | diverges from frame 1 | — |
| Doom, fixed runner | every frame, every part, 3 runs, cold or warmed, a re-entry included | picture and events on every frame but the first after an entry; sound and sound-side memory differ throughout | picture, sound and events on every frame; memory differs on some frames (the game-variable window on 1–57 frames per run) |

To reproduce on one machine:

```sh
harness --game smb3 --challenge 1-1-time-trial --frames 6000 --script 7 --record smb3.txt
harness --replay smb3.txt
harness --game doom --challenge e1m1-time-trial --frames 4200 --script 9 --warmup 120 --record doom.txt
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
or memory is read, and two recordings came out byte-identical. Three things
had to be true together, and the runner on `server-relay` now does all three
for a core whose manifest declares `settle_ms` (Doom's is 10): it waits that
long after a frame before reading memory, restoring a state or running the
next frame; it reads a frame's facts before starting the next one; and it
hands the core its keyboard events at the frame boundary rather than the
moment a key changes, since dosbox-pure takes those into a queue its thread
reads mid-frame. A paced client is a whole frame past its last `retro_run` by
its next turn and pays nothing. One frame in E1M1 takes longer than 5 ms of
host time, which is why Doom's value is 10; under WSL even 10 was not always
enough for it, so this is a bound that holds in practice, not a guarantee.

**A restore during dosbox-pure's boot is a race too.** `LibretroRunner::start`
spends one frame before anything is loaded, and the core boots on after it
returns. A state restored while it is still in `DBPSTATE_BOOT` is restored
without pausing the thread, and the stage then starts one frame further on or
not, by timing: a replay that reached its first entry sooner than its
recording ran exactly one frame ahead from the start. Waiting 50 ms before the
first entry was enough; the runner now waits 250 after that frame for a core
that declares a settle.

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

## Catching up from a state (2026-11-02)

Tried as the way a late watcher of Doom could skip its slow replay of the
backlog (ADR 0037). `--save-state-at <frame>` writes a portable state beside
a recording — the frame's pad, mouse buttons and held keys, then the runner's
snapshot in the ordinary savestate context — and `--resume-state <file>`
restores it straight after the first entry and compares only the frames after
it. `--quiet-before-state <n>` makes the script hold nothing over the frames
before the save.

- **Snes9x** resumes exactly; **Mesen** exactly but for 120 frames of sound.
- **dosbox-pure** keeps which keys and buttons are down outside its state, and
  passes a release on only for one it has seen pressed, so a replica restored
  with nothing held loses the player's next release. Restoring after giving
  the replica the player's input and running one throwaway frame mends that.
- **It still diverges, at the first sound after the restore.** Captured with
  nothing held, the restored Doom matched until frame 2901, where an item was
  picked up; captured at 1500 or at 2200, with the mouse or without, at a 10
  or a 50 ms settle, it parts on that frame and looks elsewhere after it (half
  the 3D view's pixels differ; the status bar does not). Its mixer is partly
  reset on load and partly outside the state, and Doom's sound library drives
  its own timer from the sound card.

So the state catch-up was not built into the client: the cores it would be
exact for replay fast enough already, and the one it was for it is not exact
for. What would lift Doom's catch-up is dosbox-pure reading input only once
the frame in flight is finished, which would leave a replay nothing to settle
for.

## Consequences outside the spike

Found on the way. Fixed on `server-relay`, for the client as much as the
harness:

- **Doom's telemetry read memory while the core was running**, input landed
  mid-frame, and a restore after a death interrupted a frame in progress.
  Before the fix the runner, given one scripted input three times, played three
  different games of E1M1 (1, 4 and 2 items picked up, on different frames);
  after it, three identical ones, death and re-entry included. SMB3 and DKC
  print the same events, frame for frame, as before. ADR 0018's first
  criterion — deterministic frame stepping — holds for dosbox-pure only
  because the frontend now lets its thread park.
- **The first entry of a Doom round raced the core's boot**, as above.
- **`GET_THROTTLE_STATE` was answered by writing a `bool`** into what is a
  `retro_throttle_state { unsigned mode; float rate; }`. It now reports the
  core's own rate. dosbox-pure sizes each frame's sound from it, so recordings
  made before the fix differ from replays after it in how sound is chunked,
  though not in the game.

And one that is not fixed:

- **Exiting with dosbox-pure's thread alive hangs on Linux.** Every Doom
  replay under WSL hung after its report: the harness called
  `process::exit` with the core still loaded, and the core's static
  destructors wait on its thread. The harness now drops the runner first and
  exits cleanly. The client does not call `process::exit` with a core loaded,
  but closing the app mid-round ends Tauri's event loop, which on Linux exits
  the process the same way. Not checked.

## What it does not settle

The hand tests, in order:

A package (`tools\package.ps1`) carries `harness.exe`, this file, and three
launchers: `Record scripted.cmd` records one scripted run of each game,
`Record.cmd` records a played one (DOOM E1M1 by default; `Record.cmd smb3
1-1-time-trial` for another), and `Replay.cmd` replays whatever recordings are
dropped on it, writing each one's full report beside it as `.result.txt`.
Recordings go in `recordings\`. Both machines need the same ROMs; a replay
refuses otherwise.

1. **Two real machines.** Run `Record scripted.cmd` on one, send the three
   files to the other however is convenient, and drop them on `Replay.cmd`
   there. Drop them on this machine's `Replay.cmd` too, as the control.
2. **A played recording.** `Record.cmd` and play it in the window. The digest of each frame
   is taken at the next frame's turn, when the core has parked; the input is
   whatever the 70 Hz pacing delivered. Replay it on the same machine and on
   the other. This answers whether a paced client is settled often enough in
   practice, which no scripted run can.
3. **Sound.** Whether the sound a history-laden Doom replay makes is audibly
   different. Needs a replay that plays aloud, which the harness does not do.

If replay is pursued, it needs a new ADR, and the dosbox-pure findings decide
its shape: the console cores could replay as they are; dosbox-pure replays
the game but not the sound unless the core is patched to serialize its mixer
whole, and its desync detection has to look at the facts and the picture, not
at whole memory.
