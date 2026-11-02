# Street Fighter II Turbo (SNES)

The first head-to-head game (ADR 0036): two players on one Snes9x, kept in
step by rollback. `manifests/sf2.toml` is the manifest; this is where its
numbers came from and what has been seen to work.

## The entry state

`assets/states/sf2_vs.state` is the first frames of a two-player match, both
fighters at full health and no rounds won. It was captured in RetroArch and
arrived as an RZIP container — `#RZIPv\x01#`, zlib chunks of 128 KiB — around
a RASTATE v1 block whose `MEM ` section is a Snes9x snapshot
(`#!s9xsnp:0014`), the same format version this tree's core writes. The
snapshot is what the manifest names; the container is not a state the core
can take, which is what `retro_unserialize refused 129885 bytes` meant.
Unpacking is twenty lines of Python (chunks, then the `MEM ` block). It came up
again with U.N. Squadron, and is now `tools/rzip-unpack.py`.

## The addresses

All found with the harness's scripted sync test on that state, logging the
whole of system RAM every thirty frames:

```sh
harness --game sf2 --stage vs --synctest 7200 --slots 2 --check 1 \
    --script 3 --idle-slots 1 --ram-log p1.bin      # player 1 fights, 2 stands
harness --game sf2 --stage vs --synctest 7200 --slots 2 --check 1 \
    --script 5 --idle-slots 0 --ram-log p2.bin      # the mirror
harness --game sf2 --stage vs --synctest 7200 --slots 2 --check 1 \
    --script-slots 0 --ram-log idle.bin             # both stand
```

and then looking, in each log, for bytes that stepped 0 → 1 → 2 and never
down, bytes that started at a maximum and fell in steps, and bytes that
counted down in BCD. The two players' addresses mirror each other `0x200`
apart, and the mirror runs played out as mirrors — the same frames, the
other player.

| Address | Width | What | Seen |
|---|---|---|---|
| `$05D0` / `$07D0` | u8 | rounds won this match, player 1 / 2 | 0 → 1 at the first time-over with more health, → 2 at the second; untouched by a draw |
| `$0530` / `$0730` | u8 | health, 176 (`0xB0`) full | falls on each hit, back to 176 at a round's start |
| `$18F3` | u8, BCD | the round timer, `0x99` down | one tick about every 30 frames, so a round is about 50 s |
| `$18DF` | u8 | rounds played, whoever won | 0 → 1 → 2 in every run, draws included |

What the rules make of them: `rounds_won` per slot from the two counters,
`damaged` per slot from the two health bars. The challenge, `best-of-three`,
is `counts = { rounds_won = 2 }`, which each slot's run is judged against on
its own facts; the first to hold it ends the bout for both (ADR 0036). A
match where every round is a draw reaches the limit, 240 s, and is everyone
failing.

Not found and not needed yet: a flag for leaving the fight (the bout ends on
the second win), who the characters are, the stage.

## Rollback

`harness --game sf2 --challenge best-of-three --synctest 7200 --slots 2
--check 3` passes with the tallies the rules declare (`rounds_won [slot 0]=2`
with slot 1 idle), at about 430 frames a second of host time with three
replays per advance; the per-frame cost is Snes9x's, measured in
`docs/games/dkc.md`.

## The first bout

Two clients on one machine through `gamertrials-server` on loopback, with
`--script` pressing the buttons:

```sh
gamertrials-server --listen 127.0.0.1:7878 --manifests manifests
app-client --manifests manifests --host A --server 127.0.0.1 --start \
    --mode combo --format challenges:1 --games sf2 --h2h --script 3
app-client --manifests manifests --join 127.0.0.1/CODE --name B --script 5
```

Both probed the server's UDP side, were told each other's address, found
each other (round trip 0 ms, loopback), reported prepared, and played: the
provisional judges on both machines settled the same verdict on the same
frame — one completed at 5825, the other beaten on it — the server agreed,
and the round result and the standings reached both. The first attempt had
stalled on a rule the sync test could not show: the beaten slot reported no
frame for the one it was beaten on, and the server holds the winner's
completion until every slot has reported past it; the frame a verdict
settles on is now always reported.

Three players too, with `--start --players 3` holding the host until all
are in: the seed drew two, the third never picked and was given a side at
the fifteen-second mark (`sides: [("B", "A")]`), watched that player's
picture from the deal, and was recorded with that player's beaten entry —
the round card read C first, A and B beaten, and the standings C with two
points and the other two tied second.

Not yet: two machines over the internet (ggrs's round trip and frames-ahead
go in ADR 0036 when it is), and anyone actually holding a pad.
