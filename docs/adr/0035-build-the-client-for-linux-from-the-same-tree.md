# 0035. Build the client for Linux from the same tree, on X11

Date: 2026-10-31
Status: Proposed

## Context

The project has targeted Windows only. A second tester plays on Ubuntu and
wants the latest build, not a fork of it, and the project has one developer
who does not run Linux day to day.

Most of the tree was already portable: `challenge-core`, `challenge-library`,
`protocol`, `transport`, `server`, `server-net`, `content-gen` and
`asset-store` make no platform calls, and libretro cores, minifb, cpal, gilrs,
Tauri and openh264 all run on Linux. What was not portable is small and in
two crates: the client's input reading (`GetAsyncKeyState`), its window sizing
and borderless fullscreen, the overlay's placement over the game window (ADR
0027), the mouse capture Doom uses, and the `*_libretro.dll` suffix a core
directory is searched for.

The overlay is the hard part. It is a transparent window kept exactly over
another program's window. Wayland, Ubuntu's default session since 22.04, lets
no client place its window or learn where another's is, by design; X11 does
both.

## Decision

**One tree, one branch.** Linux code sits beside the Windows code under
`#[cfg(target_os = "linux")]`, in the module that already held the Win32 call
(`keys::host`, `video::window`, `overlay::native`, `input::cursor`). No
branch or fork carries the port; CI builds and tests both platforms on every
push (`.github/workflows/ci.yml`), so a change on one cannot quietly break
the other.

**Every window is an X11 window.** minifb is built with its X11 backend only,
and the client sets `GDK_BACKEND=x11` before Tauri starts, so the game window
and both webviews are X11 clients — under XWayland on a Wayland desktop,
which every Ubuntu desktop has. The Win32 calls map one for one:

| Windows | X11 |
|---|---|
| owned window (`GWLP_HWNDPARENT`) | `WM_TRANSIENT_FOR` |
| `GetAsyncKeyState` | `XQueryKeymap`, `XQueryPointer` |
| `GetForegroundWindow` | `_NET_ACTIVE_WINDOW` |
| `SetForegroundWindow` | an EWMH activation request |
| `IsIconic` | ICCCM `WM_STATE` |
| borderless at the monitor's size | `_NET_WM_STATE_FULLSCREEN` |
| `ClipCursor` + `SetCursorPos` | `XWarpPointer` every frame, no grab |
| minifb's `StretchDIBits` | MIT-SHM into a pixmap, XRender stretches it (`x11::Presenter`) |

They live in `session_libretro::x11`, beside the game window minifb opens,
loaded at run time through `x11-dl` as minifb loads it.

**The game window is not painted by minifb.** Its X11 backend stretches each
frame on the CPU to the window's size and sends every pixel of that with a
plain `XPutImage`, on the thread that steps the core: eight megabytes a frame
at 1920×1080, and a large enough window slowed the game itself, every core
alike. The client sends the canvas alone — through shared memory where the
server allows it, else as a small `XPutImage` — and the server's XRender
stretches it over the window, as the system's blit does on Windows. Measured
under WSLg (a server that scales on the CPU) with a 256×240 canvas, a frame's
paint on the game thread cost:

| Window | minifb | `Presenter` |
|---|---|---|
| 1280×720 | 1.4 ms | 0.4 ms |
| 1920×1080 | 3.3 ms | 0.9 ms |
| 2560×1440 | 6.6 ms | 2.3 ms |
| 3840×2160 | 19.5 ms | 7.6 ms |

with every sampled pixel the one nearest-neighbour sampling at pixel centres
picks, with shared memory and without. A server with glamor scales on the
GPU. minifb still paints a server with no XRender.

**A key is still a Windows virtual-key code.** It is what a settings file
stores (ADR 0028), so one file binds the same keys on both platforms; Linux
maps each code to the keysym it types.

**A core directory may hold both builds.** The runner looks for its own
platform's suffix (`runner::CORE_SUFFIX`) and ignores the other's.

## Alternatives considered

**A long-lived `linux` branch, or a fork.** Every Windows change would need
carrying across by hand for as long as both exist, and the platform its
developer does not use would be the one that falls behind unseen.

**Wayland-native.** There is no protocol for one client to stack a window
over another's or to follow it; a compositor-specific route would cover one
desktop at a time. The alternative that works everywhere is to draw the
overlay inside the game window, which is ADR 0027's rejected option and a
different client.

**Ship the webview as the game window** (one Tauri window, the picture drawn
in the page). Also 0027's rejected option; it moves every frame through the
webview.

## Consequences

**Played under WSLg, not yet on a Linux desktop.** The workspace builds and
every test passes on Ubuntu 22.04 under WSL. Under WSLg a round of each game
plays with the overlay transparent over it, its clock running, and `--spike`
finds it aligned with the game window through a move and a resize, and
borderless fills the monitor. Not yet seen: a real desktop's window manager,
fullscreen there, mouse capture, sound (WSL has no ALSA device), and a match
against a Windows player.

**Both platforms run one build of each core.** An entry state is a savestate
(ADR 0002), and a core of another version may refuse it or load it
differently, which in a race is a different game. The buildbot keeps only
the latest nightly, so the port moved the Windows DLLs to the same night's
build as the Linux ones: Mesen 0.9.9, Snes9x 1.63 `fae2fea` (from
`9e63727`), DOSBox-pure 1.0-preview7 (from preview6). Every raced stage,
run headless for 600 frames, printed the same events on the old DLLs, the
new DLLs and the new `.so` files, and Doom's eight pinned options are all
still offered. A core is updated on both platforms at once, or not at all.

**What is weaker on Linux:**

- A key tapped and released between two polls is missed: X11 keeps no
  "pressed since" bit, which Windows' `GetAsyncKeyState` does.
- The mouse's side buttons (X1, X2) cannot be bound: X11's pointer state has
  no bits for buttons 8 and 9.
- The cursor is re-centred every frame rather than confined, so a fast flick
  can reach past the window's edge for a frame.
- `--spike` measures placement and focus but cannot inject its key, which
  needs XTest.

**WebKitGTK's DMA-BUF renderer stays on.** The usual advice for a blank
WebKitGTK page under XWayland is `WEBKIT_DISABLE_DMABUF_RENDERER=1`; under
WSLg that left the transparent overlay unable to clear what it had drawn,
so each new line of text landed on the last. A player who needs it can still
set it.
