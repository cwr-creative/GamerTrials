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

They live in `session_libretro::x11`, beside the game window minifb opens,
loaded at run time through `x11-dl` as minifb loads it.

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

**Not yet hand-tested on a Linux desktop.** The workspace builds and every
test passes on Ubuntu 22.04 (under WSL), and the menu draws in an X11 window
under WSLg; the overlay's placement, focus and pass-through,
fullscreen, and mouse capture under XWayland are what the first match on
Linux has to check.

**Linux cores are owed.** `cores/` holds Windows DLLs only. The Linux builds
must be the same versions as the DLLs beside them: an entry state is a
savestate (ADR 0002), and a core of another version may refuse it or load it
differently, which in a race is a different game.

**What is weaker on Linux:**

- A key tapped and released between two polls is missed: X11 keeps no
  "pressed since" bit, which Windows' `GetAsyncKeyState` does.
- The mouse's side buttons (X1, X2) cannot be bound: X11's pointer state has
  no bits for buttons 8 and 9.
- The cursor is re-centred every frame rather than confined, so a fast flick
  can reach past the window's edge for a frame.
- `--spike` measures placement and focus but cannot inject its key, which
  needs XTest.

**The DMA-BUF renderer is off.** WebKitGTK's DMA-BUF renderer draws a blank
page under several XWayland drivers, so the client sets
`WEBKIT_DISABLE_DMABUF_RENDERER=1` unless the player has set it.
