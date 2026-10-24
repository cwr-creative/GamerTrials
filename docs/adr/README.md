# Architecture Decision Records

One file per decision. Format is Michael Nygard's, plus an explicit
**Alternatives considered** section — for this project that section is the most
valuable part, because several rejected options are attractive enough to be
proposed again.

## Rules

- ADRs are **append-only**. Never edit an accepted ADR to reflect a new decision.
  Write a new ADR, set the old one's status to `Superseded by NNNN`, and link both.
- Status is one of: `Proposed`, `Accepted`, `Rejected`, `Superseded by NNNN`,
  `Deprecated`. A `Rejected` one needs no file if it never got past this index —
  but the reason it was rejected is recorded below, because that is the part that
  stops it being proposed again.
- Write the ADR when the decision is made, not later. The rationale is the part
  you forget first.
- Keep them short. One page. If it needs more, the decision is probably two decisions.

## Template

```markdown
# NNNN. Title in imperative mood

Date: YYYY-MM-DD
Status: Accepted

## Context
What forces are in play? What makes this decision necessary?

## Decision
What we are doing. Present tense, active voice.

## Alternatives considered
What else was on the table, and the specific reason each was rejected.

## Consequences
What becomes easier. What becomes harder. What we have to live with.
```

## Index

| # | Title | Status |
|---|---|---|
| 0001 | Embed libretro cores directly rather than driving RetroArch | Accepted |
| 0002 | Use savestates for stage entry | Accepted |
| 0003 | Stream video for spectating rather than replicating inputs | Accepted |
| 0004 | Delegate head-to-head netcode to external per-game solutions | Accepted |
| 0005 | Measure timing in emulated frames rather than wall-clock | Accepted |
| 0006 | Adopt ports and adapters with a pure challenge core | Accepted |
| 0007 | Reject the nested "frankenstein ROM" approach | Accepted |
| 0008 | Treat decomp/recomp PC ports as external-process adapters | Superseded by 0018 |
| 0009 | Server owns challenge sequencing; run it in-process for solo play | Accepted |
| 0010 | Game manifests are TOML data in git, not code | Accepted |
| 0011 | Client in Rust + Tauri; server in Go or TypeScript; SQLite storage | Accepted |
| 0012 | Extract the StageRunner interface in Phase 2, not Phase 1 | Accepted |
| 0013 | Generate stage content on each client from a server-issued seed | Proposed |
| 0014 | Detect stage completion and death by watching emulator RAM | Accepted |
| 0015 | Scope custom-challenge ROM generation to declarative value-substitution patches | Accepted |
| 0016 | Describe game telemetry as declarative RAM rules in the manifest | Accepted |
| 0017 | Challenges are contributor-authored manifest entries, not core-defined kinds | Accepted |
| 0018 | Support PC games only through emulation, and retire the external-process adapter | Accepted |
| 0019 | Rebind an entry state to a generated ROM rather than regenerating it | Accepted |
| 0020 | Carry alternative ROMs as patches against the base, and let a stage name one | Accepted |
| 0021 | Savestates are generated based on instructions from manifests, superseding 0019 | Rejected |
| 0022 | Instead of 0021, savestates are rebound based on instructions from manifests | Rejected |
| 0023 | Judge a run per attempt, end an attempt at a death, and let the session bound it | Proposed |

0003, 0013, 0014, 0015, 0016, 0017, 0018, 0019, 0020 and 0023 are written up; 0003 is
the exemplar. Fill in the rest as you implement each area; an unwritten ADR is
fine, a wrong one is not.

**0013 is still `Proposed` although `content-gen` is built, and that is
accurate.** What exists is the client half: the derivation key, deterministic
local generation, and the artifact hash. The half that makes it a *decision about
a session* — a server issuing the seed, clients comparing hashes, and the abort
on mismatch — has nothing to be built into yet. 0015 and 0020, which are about
what a generator may do and how content is declared, are Accepted and implemented.

## Notes on a few unwritten ones

**0002** — The savestate decision is load-bearing for the whole content pipeline.

The original justification for `EntryPoint` being an enum rather than a file
path was that Doom has no savestates and uses `-warp` instead. That premise is
gone: under `dosbox-pure` (ADR 0018) Doom *does* serialize, and a state restores
into a fresh process. The enum survives on a better reason — both spellings are
genuinely used, and `named` is how a state gets captured in the first place. A
DOS game reaches `named` through a command line in its own bundled
`dosbox.conf`, not through a process argument.

That is now implemented and measured, and the numbers are the argument: a Doom
state restores in ~60 frames and a cold boot takes ~960. `manifests/doom.toml`
declares one stage of each kind for the same place — the `named` one to author
a state from, the `state` one to race on — which is the pipeline the enum
exists to express, not two competing options.

Scope it narrowly: it rejects ROM patching as the way to enter a *vanilla* stage.
Generated challenges patch ROMs for an unrelated reason and are not covered — see
0013.

**0007** — Rejected: emulating SNES/NES inside an N64 ROM. Reasons were RDRAM
latency, stacking the least reliable emulator at the bottom, and destroying the
ability to reuse RetroAchievements RAM maps. Worth writing down because "one
architecture instead of three" is a genuinely good instinct pointing at a bad
implementation, and it will come up again.

**0008** — *Superseded by 0018, and never written.* What it held: ports (Ship of
Harkinian, N64Recomp output, etc.) buy cheap instrumentation via named symbols
but cost savestates and rcheevos map reuse; selective use only, for mature
titles, with a framerate-locking requirement. 0018 rejects the whole category —
the framerate lock treats a symptom, and a native process also has no
framebuffer to hand a spectator (0003). Recorded here rather than lost, because
"just use the good modern port" is a recurring and reasonable-sounding
instinct.

**0018's spike is gone, and that was the plan.** `crates/dos-spike` is
referenced by 0018 and no longer exists: what it proved moved into
`session-libretro` and the crate was deleted, which is what its own README said
would happen. 0018 is left unedited because ADRs are append-only and it is an
accurate record of what was done at the time.

Two of its tools were *not* carried over, deliberately, and are worth knowing
about if either is ever wanted again: PNG screenshots on a headless run
(`--shot`, `--shot-every`), which is how "the right value for the wrong reason"
was caught, and a scripted-input language (`--script "right:44 shift+up:400"`)
for driving a game without a human. Neither is needed to author a manifest and
both would have to be rebuilt.

**The crate split 0017 owes is done.** `challenge-library` reads the
challenge half of a manifest — `[game]`, stage names, `[[challenge]]`, and what
each rule emits — with no emulator and no `content-gen`, and
`session-libretro`'s loader parses through it before reading the runner half.
0017, 0009 and 0024 still describe the split as owed; they are left as written
because ADRs are append-only.

**0012** — Records *when* to abstract, not *what*. Included because the reasoning
generalises: build the second implementation before extracting the interface.