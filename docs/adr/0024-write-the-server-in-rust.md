# 0024. Write the server in Rust, superseding ADR 0011

Date: 2026-10-24
Status: Accepted

## Context

ADR 0011 chose "Client in Rust + Tauri; server in Go or TypeScript; SQLite
storage". It was never written up, so the reasoning behind the server's
language is not on record.

Since then ADR 0009 has been written down with the server as the authority on
dealing, judging and ranking, and `challenge-core` exists as the pure Rust
crate that does all three. The client links it too, for solo play and to judge
provisionally. Whatever the server is written in has to run *that* code, not a
description of it.

## Decision

**The server is Rust, a crate in this workspace, and links `challenge-core`
directly.** The rest of ADR 0011 stands and is restated here so that 0011 can be
superseded whole: the client is Rust and Tauri, and storage is SQLite.

Its dependencies are held to its role the way `challenge-core`'s are held to
invariant 1 — by its `Cargo.toml`. It may depend on `challenge-core`,
`session-api` and the crate that parses the challenge half of a manifest. It
never depends on `session-libretro`, `content-gen` or `asset-store`: it never
runs a game, never touches ROM bytes and never holds a ROM. A dependency on any
of them is the server learning a game, and is wrong.

## Alternatives considered

**Go or TypeScript, reimplementing the rules.** What 0011 implied. Rejected
because there would be two judges and two dealers. The deal's generator is
written out by hand so that a seed means the same challenges everywhere; a
second implementation in another language is exactly the drift that rule
exists to prevent, and a judge that disagrees with the client's provisional
verdict fails the session (ADR 0009).

**Go or TypeScript, embedding `challenge-core` compiled to WebAssembly.**
Keeps one implementation. Rejected for now, not forever: it adds a second
toolchain and a serialisation boundary around every `SessionEvent` and
`ChallengeSpec` for a solo developer whose other crates are all Rust, and it
buys nothing the Rust server does not. If a reason to leave Rust appears, this
is the route, and the purity of `challenge-core` is what keeps it open.

**Server-side judging dropped, so the language stops mattering.** Rejected by
ADR 0009.

## Consequences

**One workspace, one toolchain, one set of types.** Solo play links the same
crates in-process; the server is that half running somewhere else.

**Shared Rust types make the wire protocol easy to forget.** Wire messages still
carry a version field and still never remove one (CLAUDE.md). A Rust type
derived straight onto the wire is a protocol that changes whenever the struct
does; the messages want types of their own rather than borrowing `session-api`'s.

**The crate split becomes a prerequisite.** The server needs `[[challenge]]`
without `session-libretro`'s emulator, so the challenge-library parser has to
move out first (ADR 0009, ADR 0017).

**Not decided here.** The async runtime, the transport, and the SQLite binding.
Each is the server's first real decision and gets made when it is built.
