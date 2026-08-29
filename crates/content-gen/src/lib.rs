//! Content derivation: the only crate that touches ROM bytes.
//!
//! Two transforms live here, and a game may use either, both or neither:
//!
//! * **A hand-authored hack**, applied from a contributor's BPS patch
//!   ([`bps::apply`]). A human built the level in an editor and playtested it;
//!   the platform carries the difference, never the ROM (ADR 0020).
//! * **A seeded value substitution** ([`derive`]), which rerolls the values
//!   already sitting at declared offsets — the enemy at an existing spawn slot,
//!   the item at an existing pickup. Same seed, same bytes, every machine
//!   (ADR 0013, ADR 0015).
//!
//! They compose in one direction, and that direction is the interesting one: a
//! generated challenge sitting on a hack is the expected shape rather than an
//! exotic one. A contributor designs a level, and the engine rerolls the enemies
//! inside it. A derived ROM is a base ROM in every respect that matters here —
//! its patch-slot offsets are into it, and its own hash is what the derivation
//! key names.
//!
//! ## What this crate refuses to be
//!
//! **It has no per-game knowledge and must never acquire any.** A slot table is
//! data; a BPS patch is data; the only thing keyed on a console family is the
//! header size and the checksum rule ([`Console`]), which ADR 0015 sized as "a
//! small, closed set reviewed once per new console". If something in here starts
//! wanting to know what SMB3's object list looks like, the abstraction has
//! failed and the fix is in the abstraction (invariant 5).
//!
//! **It cannot generate structure.** No new objects, no new geometry, no
//! relocated or expanded tables. ADR 0015 rules that out permanently rather than
//! pending, and ADR 0019 turned it from a preference into a correctness
//! requirement: an entry state is rebound onto a derived ROM by rewriting the
//! ROM hash in its header, and that transplant is only safe because the derived
//! ROM has the same length, mapper and geometry as the one the state was
//! captured on. A structural patch would break a rebound state *silently*.
//!
//! **It has no dependencies worth the name, and no emulator.** That is what lets
//! the golden-hash tests run in CI as ordinary `cargo test`, and what lets
//! contributor tooling preview a patched ROM without loading a core.
//!
//! **It never decides anything about a run.** It produces bytes. Whether the
//! challenge those bytes carry was completed is `challenge-core`'s question, and
//! nothing in this crate has ever heard of it.
//!
//! ## The shape of a derivation
//!
//! ```no_run
//! # use content_gen::{bps, derive, Console, PatchTable};
//! # fn go(base: &[u8], patch: &[u8], table: &PatchTable) -> Result<(), String> {
//! // A hack: the player's own dump, plus the contributor's patch.
//! let hack = bps::apply(base, patch)?;
//! // A roll on it: same seed, same bytes, on every machine in the session.
//! let artifact = derive(&hack.bytes, Console::Nes, table, 0x5EED)?;
//! # let _ = artifact.sha1; Ok(())
//! # }
//! ```
//!
//! Nothing generated is written anywhere by this crate, and nothing generated
//! crosses a wire. Both are the caller's business, and ADR 0013 has strong
//! opinions about the second.

#![forbid(unsafe_code)]

pub mod bps;
pub mod console;
pub mod hash;
pub mod roll;
pub mod slots;

pub use console::{Console, Fixup};
pub use hash::{crc32, sha1_hex};
pub use roll::{derive, Artifact, DerivationKey, Roll, Rolled, Write};
pub use slots::{
    Endian, Group, GroupDoc, GroupMode, PatchSlot, PatchSlotDoc, PatchTable, RangeDoc, ValueSource,
    ValuesDoc, Width,
};
