# 0028. The client owns input binding and presentation, and neither reaches the machine

Date: 2026-10-29
Status: Accepted

## Context

Phase B's Settings page has to let a player bind a gamepad and/or keyboard
and mouse once and have it work in every game, and set video once and have it
hold across cores. Either can be overridden for one core, or for one game.

The premise that libretro controls are core-specific is mostly not true. Every
console core reads the same virtual pad, the RetroPad
(`RETRO_DEVICE_JOYPAD`), and the frontend decides what drives it. What
actually varies is which RetroPad buttons a console has, which a manifest
already says (`pad_layout`), and, for a keyboard game, which keys *the game*
uses. Doom's come from DOOM 1.9's own defaults, and the bundle pins them
(ADR 0018).

Two things are already settled and constrain this. Scores are counted in
emulated frames (invariant 4). And a core option is part of a game's
identity, pinned in the manifest, not a preference (ADR 0018).

## Decision

**The player binds host inputs to the RetroPad, in layers: game, then core,
then global, then the built-in default.** Each layer holds only what it
changes, and an empty list means deliberately unbound. The built-in default is
the mapping the client had before settings, drawn from `pad_layout`, so an NES
game keeps "South *or* East is A" until the player says otherwise.

**A binding's keyboard-and-mouse half and its gamepad half inherit
separately.** A player who moves a key has said nothing about their pad. Up to
three inputs per half per button, and the halves OR together on the same
RetroPad button. That is how one game is driven by both a keyboard and a pad.

**A keyboard game takes the keyboard and mouse as they are.** The keyboard
passes through, optionally through a per-game host-key → guest-key remap, so
WASD can drive DOOM's arrows. The mouse's motion can be scaled. The *gamepad*
half of the RetroPad binding drives the game through a RetroPad → guest-key
table (`pad_keys`), whose defaults the manifest suggests: a new, optional
`[runner.libretro.pad_keys]`. That is a data change, like every other game
change (invariant 5). It is a suggestion, not a remap, because the manifest
still never rebinds the keyboard.

**Video is presentation only.** Window scale, fit / integer / stretch, the
core's aspect or square pixels, edge crop and borderless fullscreen are global
settings, and a core can override any of them except fullscreen. The
client draws a canvas the shape of the window with the cropped picture placed
at 1:1, and minifb stretches it, so no CPU upscale is added to the frame path.

**Nothing in settings reaches the emulated machine or its clock.** No core
option, and no fast-forward, slow motion, frame limiter, vsync toggle,
run-ahead, frame skip, rewind or region. Slow motion buys reaction time at the
same frame count. Run-ahead changes which emulated frame a press lands on.
Settings never go on the wire either; they are this machine's
(`settings.toml` in the app's config directory).

The client's own two actions, pause and give up, are bindable chords (global
only), defaulting to F10 or Select+Start and F9. The pause menu reads the pad
itself, so a pad-only player can give up from it.

## Alternatives considered

**Per-core control schemes, as the request first assumed.** Rejected as the
*only* layer: it makes the player rebind every console for what is one
RetroPad. It is kept as the middle layer.

**One list per button, keys and pad together.** Built first and rejected on
its first test. Rebinding a key globally copied the four-button pad default
along with it, which silently replaced an NES game's two-button pad layout and
stripped a keyboard game's pad binds.

**A "fold" for two-button consoles instead of a layout-shaped default.**
"RetroPad Y also presses B", or similar. Every version either moved today's
NES mapping or made a global edit mean different buttons on different consoles.
A default that depends on the layout, under per-button layers, reproduces today
exactly.

**Expose core options the manifest marks as display-only** (NTSC filters,
palettes, overscan). Deferred, not rejected: each option needs vetting for
whether it changes what a frame means, and the frontend crop covers the
common case. A future manifest allowlist is the shape if it is wanted.

**Capture bindings in the webview** (`KeyboardEvent.code`, the Gamepad API).
Rejected: it is a second input vocabulary that would have to be translated into
the virtual keys and gilrs buttons play reads, and it would drift from them.
Capture runs in Rust on the same path as play. The pause menu's own
navigation is the one place the page reads a pad, because the game thread is
held while it is up.

**Per-device profiles, or RetroArch-style remap files.** Not needed for one
local player. Every connected pad is read as one pad, as before.

## Consequences

**A player who never opens Settings plays exactly as before**, and a test pins
the built-in default against the old mapping.

**A global pad edit applies to every console, including the two-button ones.**
Moving NES B's pad input globally gives up the dual-habit layout for that button.
That is what "global" means. The page says so, and a core override restores it.

**Video applies from the next round.** The window is resized when a round's
core wants a different size, and left alone otherwise, so a player's own resize
survives. Fullscreen is the two-window case ADR 0027 describes. Alt-tab,
minimise and a monitor move are its hand tests, and the taskbar's z-order
against the always-on-top overlay is one more. On the first build, alt-tab
and Win+Shift+arrow worked, and only Win+D minimised: a `WS_POPUP` with no
minimise box cannot be minimised by Win+Down or its taskbar button, so
borderless keeps `WS_SYSMENU | WS_MINIMIZEBOX`, which draw nothing without a
caption. With them, Win+Down minimises it.

**Integer scaling is exact in a window and may be off by a column in
fullscreen**, when the monitor's width is not a multiple of the scale. The
stretch is minifb's, and the canvas cannot be a fraction of a pixel.

**Spectating (ADR 0003) encodes the core's frame, not the canvas.** A viewer's
settings are theirs.
