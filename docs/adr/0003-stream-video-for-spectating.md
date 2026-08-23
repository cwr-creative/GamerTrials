# 0003. Stream video for spectating rather than replicating inputs

Date: 2026-08-20
Status: Accepted

## Context

After finishing a stage, a player must be able to watch others who are still
playing. This is a core requirement, not a nice-to-have.

The library spans three adapter types: libretro software cores (SMB3, DKC), an
external process (Doom), and potentially native decomp/recomp ports later. Any
spectating mechanism has to work across all of them, or we maintain two systems.

Expected scale is 4–8 friends. Retro content is low resolution — 256x240 for NES,
256x224 for DKC.

## Decision

Spectating is **video streamed over WebRTC**. Each client encodes its own
framebuffer and uploads a single stream; spectators receive and decode it in the
Tauri WebView2 panel, which handles jitter buffering, decode, and loss adaptation
for free.

Start with a full mesh for the vertical slice. Introduce an SFU (LiveKit) at
roughly four concurrent players.

Encode with ffmpeg/libavcodec. Software x264 on `ultrafast` is sufficient at
retro resolutions (~2% of one core); hardware encode is deferred until Doom at
1080p makes it worthwhile.

## Alternatives considered

**Input replication.** Send only controller inputs plus an initial savestate; the
spectator's local core re-simulates. Costs bytes per frame instead of megabits.

Rejected because it requires bit-exact determinism across machines. Cores with
threaded rendering or audio-driven timing desync silently — the spectator sees a
plausible but wrong game, with no error surfaced. It also cannot work for Doom or
any external-process adapter, so it would mean building and maintaining video
streaming anyway, for a subset of games.

This option is genuinely elegant and will be proposed again. The reason it loses
is not bandwidth; it is that "silently wrong" is the worst possible failure mode
for a competitive application, and that it solves only part of the problem.

**Screen capture of the whole window.** Simpler to implement, but captures the
overlay and menus too, and gives no control over resolution or framerate.
Rejected as strictly worse than capturing the framebuffer we already own.

## Consequences

**Easier.** One mechanism for every adapter type. Failures are visible rather than
silent — you can see the stream is wrong. Receiving is free via the webview, so
native code only ever sends. Debugging is tractable.

**Harder.** Bandwidth is megabits rather than kilobits. Requires an encoder
dependency and, past four players, an SFU deployment.

**Live with.** A fast finisher watching a slower player gains information. Harmless
in time trial (their run is already banked); add a configurable delay buffer if a
future stage type makes it matter.

**Open door.** Input replication remains viable later as a bandwidth optimisation
for libretro adapters specifically, with periodic savestate keyframes for resync.
It would be an addition, not a replacement. Any such work needs a new ADR.
