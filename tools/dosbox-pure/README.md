# dosbox-pure, patched

On the `input-replication` branch only (ADR 0037).

dosbox-pure emulates on a thread of its own. Its `retro_run` reads the
frame's input, then waits for the frame in flight, then starts the next one
and returns, so input it reads while the last frame is still running is taken
by that frame, mid-way, at a point host timing decides. The runner on `main`
copes by waiting (`settle_ms`) before every frame. A replay that is catching
up a late watcher cannot afford that wait: it bounds Doom near 100 frames a
second against the run's 70.

`0001-finish-the-frame-in-flight-before-reading-input.patch` makes `retro_run`
finish the frame in flight before it reads input, once the core is running.
Input then lands at the frame boundary however soon `retro_run` follows the
last, and a manifest says so with `input_at_boundary = true`, which lets
`LibretroRunner::advance_unobserved` skip the settle. The runner still settles
before it reads memory or restores a state: the patch changes when input is
taken, not that the thread runs on after `retro_run` returns.

## Source

- <https://codeberg.org/schelling/dosbox-pure> (moved from GitHub)
- tag `1.0-preview7`, commit `cdaea10ce8cac231bc341ca05d4c5e4e5fe0fbd5` — the
  version of the nightly DLLs in `cores/dos`, so the entry states still load.
- GPL v2 or later, as dosbox-pure is.

## Building

Both platforms at once, or neither (ADR 0035).

Windows, with the Visual Studio build tools Tauri already needs:

```
git clone https://codeberg.org/schelling/dosbox-pure.git
cd dosbox-pure
git checkout cdaea10ce8cac231bc341ca05d4c5e4e5fe0fbd5
git apply ../GamerTrials/tools/dosbox-pure/0001-finish-the-frame-in-flight-before-reading-input.patch
"C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\MSBuild\Current\Bin\MSBuild.exe" dosbox_pure_libretro.vcxproj /p:Configuration=Release /p:Platform=x64 /m
```

Linux, from the same patched checkout:

```
make -j"$(nproc)"
```

Then `dosbox_pure_libretro.dll` and `dosbox_pure_libretro.so` go in
`cores/dos`, replacing the nightly's, and `manifests/doom.toml` (and
`doom2.toml`) take `input_at_boundary = true`.
