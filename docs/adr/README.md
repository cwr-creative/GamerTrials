# Architecture Decision Records

One file per decision. Format is Michael Nygard's, plus an explicit
**Alternatives considered** section — for this project that section is the most
valuable part, because several rejected options are attractive enough to be
proposed again.

## Rules

- ADRs are **append-only**. Never edit an accepted ADR to reflect a new decision.
  Write a new ADR, set the old one's status to `Superseded by NNNN`, and link both.
- Status is one of: `Proposed`, `Accepted`, `Superseded by NNNN`, `Deprecated`.
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
| 0008 | Treat decomp/recomp PC ports as external-process adapters | Accepted |
| 0009 | Server owns challenge sequencing; run it in-process for solo play | Accepted |
| 0010 | Game manifests are TOML data in git, not code | Accepted |
| 0011 | Client in Rust + Tauri; server in Go or TypeScript; SQLite storage | Accepted |
| 0012 | Extract the StageRunner interface in Phase 2, not Phase 1 | Accepted |
| 0013 | Generate stage content on each client from a server-issued seed | Proposed |
| 0014 | Detect stage completion and death by watching emulator RAM | Proposed |
| 0015 | Scope custom-challenge ROM generation to declarative value-substitution patches | Accepted |
| 0016 | Describe game telemetry as declarative RAM rules in the manifest | Accepted |

0003, 0013, 0015 and 0016 are written up; 0003 is the exemplar. Fill in the rest as
you implement each area; an unwritten ADR is fine, a wrong one is not.

## Notes on a few unwritten ones

**0002** — The savestate decision is load-bearing for the whole content pipeline.
Note in it that Doom does not have savestates and uses `-warp` instead, which is
why `EntryPoint` is an enum rather than a file path.

Scope it narrowly: it rejects ROM patching as the way to enter a *vanilla* stage.
Generated challenges patch ROMs for an unrelated reason and are not covered — see
0013.

**0007** — Rejected: emulating SNES/NES inside an N64 ROM. Reasons were RDRAM
latency, stacking the least reliable emulator at the bottom, and destroying the
ability to reuse RetroAchievements RAM maps. Worth writing down because "one
architecture instead of three" is a genuinely good instinct pointing at a bad
implementation, and it will come up again.

**0008** — Ports (Ship of Harkinian, N64Recomp output, etc.) buy cheap
instrumentation via named symbols but cost savestates and rcheevos map reuse.
Selective use only, for mature titles. Record the framerate-locking requirement.

**0012** — Records *when* to abstract, not *what*. Included because the reasoning
generalises: build the second implementation before extracting the interface.

**0014** — Half of it is now built and written down elsewhere: 0016 settles how
RAM watching is *expressed*, and `docs/manifest-schema.md` is the schema. What
0014 still owes is the part 0016 does not touch — what an external-process
adapter does instead.

Endurance challenges ("survive without dying") and time trials
("reach the end point") both need the runner to recognise events the game never
announces. The presumed answer is watching emulator RAM with RetroAchievements-
style memory maps, which is also the reuse argument 0007 leans on — but it has to
answer what an external-process adapter does instead, since Doom has no
addressable core RAM and reports through a stats file. Write it before endurance
ships, because a mis-detected death is a lost run.
