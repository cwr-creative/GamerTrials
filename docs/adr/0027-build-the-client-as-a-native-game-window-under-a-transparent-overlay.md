# 0027. Build the client as a native game window under a transparent overlay

Date: 2026-10-28
Status: Accepted

## Context

The client is Rust and Tauri (ADR 0024). `docs/client-sketch.md` wants
full-screen animated graphics with transparency over the emulation — intros
that drop in, a countdown, a death flash, a garland — built as crude
placeholders now and replaced with final animated assets later. Spectated
video is already decoded in the webview (ADR 0003). The harness draws the game
into a native window it owns and paces by the audio clock, and input latency
in that path has been worked on and measured.

What was not decided is what draws the game inside the client, and the
answer shapes everything built on top of it.

## Decision

**Rust owns a native game window; the Tauri webview is a transparent,
undecorated, always-on-top window over it, and holds every menu and overlay.**

The game window is the harness's present path, unchanged: the core draws into
a buffer and the window blits it. The overlay is an *owned* top-level window
(owner set to the game window), so Windows keeps it above its owner and
minimises it with it; a thread tracks the game window's client area and moves
the overlay with it. While nothing interactive is up, the overlay ignores the
mouse, so clicks reach the game window. Keys are read with
`GetAsyncKeyState`, independent of which window has focus, gated on one of
the two windows being in front.

Measured on the first build (`cargo run -p app-client -- --spike`, SMB3 1-1):
the overlay stayed aligned to the pixel through a programmatic move, a
resize and a restore; the extended style carried `WS_EX_TRANSPARENT |
WS_EX_LAYERED` while passthrough was on; with the overlay as the foreground
window, an injected Right arrow reached the core as joypad bit 7. The game's
frame path is the harness's, so the overlay adds nothing between a keypress
and a frame.

The webview is a static HTML page shipped with the crate and built by
`cargo`, with no Node toolchain. A bundler and a component framework can be
added when the lobby's tree outgrows plain script; nothing here depends on
one.

The page reaches Rust only through Tauri's event bridge, and that needs a
capability: `capabilities/default.json` grants the overlay window
`core:default`. Without it `listen` is refused, no event ever arrives, and
nothing is logged on either side; the page now reports the refusal on its
HUD rather than claiming to listen. The first build also showed a white sheet
under the page, which looked like a failed composite and was not one: the
overlay was clear, and the white was the game window, held through the intro
and countdown before the core had drawn a frame. The game thread paints it
black before the hold.

## Alternatives considered

**The webview draws the game.** Rust pushes each frame's pixels to a WebGL
canvas; overlays are ordinary page content; spectating becomes
`canvas.captureStream()` into a browser WebRTC mesh with no native encoder.
Rejected: it puts a compositor hop and an IPC copy between the core and the
screen on the local player's path, which is the one latency this project has
already paid to reduce. The simplification it buys for spectating is real and
is recorded here in case the overlay approach fails on a platform.

**Native everything.** An animation runtime in Rust for the overlays, and
native decode for spectating. Rejected: ADR 0003 chose the webview to get
decode, jitter buffering and loss adaptation for free, and an animation
runtime is a second project.

**A child WebView2 inside the game window.** One window, no tracking.
Rejected without trying: a transparent WebView2 composes over what is behind
its top-level window, not over a sibling child's pixels in the same window,
and the owned-window shape was aligned and click-through on the first run.

## Consequences

**Spectating is ADR 0003 as written.** Native encode and send; the webview
receives. The rejected first alternative would have changed that, and this
decision does not.

**Two windows to keep honest.** Fullscreen is both windows borderless at the
monitor's size. Alt-tab, minimise and a moved monitor are the cases to test
by hand on every change to the tracking thread; the spike logs the two rects
so a regression is a diff.

**Input is global while a GamerTrials window is in front.** A key held when
the player alt-tabs away is released on the next frame, because the gate
reads the foreground window every frame.

**`app-client` depends on `session-libretro`** for the window, the runner and
stage prep, as the harness does. The server never does (ADR 0024).

**The harness stays.** It is the authoring tool — RAM search, probe, state
capture — and the client's game thread is a port of its windowed loop, not a
replacement for it.

**The overlay page owns the choreography.** Rust emits facts — a round was
dealt, the server said go, the player died, the round's result — and the
page decides what waits for what: the round card holds, the intro drops in,
the countdown runs, and only then does the page call `begin`, which is when
the game thread starts the run. A final asset with another duration changes
the page and nothing in Rust, and the game never starts before the animation
says so.

**Two pages, one shell.** The menu and lobby are an ordinary decorated
window; the overlay is created when the game window reports in and closes
with it. Between them the match is played; after it the score screen is on
the overlay, where the game's last frame is still behind it.
