//! Reconciling an entry state with the ROM it is about to be restored onto.
//!
//! A generated challenge derives its ROM per seed (ADR 0013) but ships its entry
//! state as an ordinary asset, so the state names one ROM and the machine is
//! running another. Whether that matters is a property of the **state format**,
//! not of the game and not of the content: a format that records the ROM's
//! identity will refuse the pairing, and a format that records nothing will not
//! notice it.
//!
//! Both answers exist among this project's three cores, which is why this module
//! dispatches on the state's own magic rather than assuming one shape:
//!
//! - **Mesen** (NES) records an uppercase SHA-1 of the ROM file in a plaintext
//!   header and compares it on load. ADR 0019 satisfies the check rather than
//!   avoiding it: recompute the hash of the ROM actually in hand and write it
//!   into a *copy* of the state. The file on disk is never touched and stays
//!   bound to the base ROM it was captured on.
//! - **snes9x** (SNES) records no ROM identity at all. Its `NAM` chunk is the
//!   literal string `Removed` — the libretro port blanks it — and no chunk
//!   carries a hash. Measured 2026-08-30: `dkc_reptile-rumble.state` restores
//!   unmodified onto a rolled `reptile-roulette` ROM and runs. There is nothing
//!   to reconcile, and saying so is this module's job as much as rewriting a
//!   hash is.
//!
//! "I do not recognise this format" and "I recognise this format and it needs no
//! repair" are therefore different answers. Conflating them is what made a sound
//! SNES pairing look like a broken one.
//!
//! This is the most core-specific code in the crate, and it stops here. No
//! header offset, no hash, and no word from this module reaches `session-api`
//! (invariant 3). It is one arm per state format, a closed set reviewed once per
//! console family — the same shape, and the same clause of invariant 5, as
//! `content_gen::console`.
//!
//! The Mesen arm is only sound because ADR 0015 restricts a patch to
//! substituting values in place: same file length, same mapper, same geometry,
//! so the machine the state describes is the machine the ROM builds. A
//! structural patch would break a rebound state silently, which is the one
//! failure mode worse than the refusal it replaces. That restriction is also
//! what makes the snes9x arm safe, and there it is the *only* thing that does —
//! a format that checks nothing will happily restore onto a ROM it should not.

/// What reconciling found, and what the caller should restore.
#[derive(Debug)]
pub enum Pairing {
    /// This state format records no ROM identity, so any ROM may be restored
    /// onto and the state as it stands is the right bytes. Carries the format's
    /// name for the log: a deliberate no-op here and a no-op from a bug look the
    /// same from outside.
    NoIdentity(&'static str),
    /// The state already names this exact ROM. Every vanilla stage.
    Bound,
    /// The state named a different ROM and has been rewritten to name this one.
    Rebound(Rebound),
}

/// A state that was bound to one ROM and is now bound to another.
#[derive(Debug)]
pub struct Rebound {
    /// The rewritten state. Restore this; leave the file on disk alone.
    pub state: Vec<u8>,
    /// The ROM the state was captured on.
    pub from: String,
    /// The ROM it will now restore onto.
    pub to: String,
}

/// The Mesen savestate header, as measured on Mesen 0.9.9, format 13:
///
/// ```text
/// 00: "MST"                 magic
/// 03: u32                   emulator version, 0x000909 for 0.9.9
/// 07: u32                   state format version, 13
/// 0b: 3 bytes               04 00 00 in every state measured; meaning unknown
/// 0e: 40 bytes              uppercase hex ASCII SHA-1 of the ROM *file*,
///                           iNES header included
/// 36: u32 + that many bytes the ROM's filename when the state was captured
/// ```
///
/// The three bytes at `0x0B` are named here rather than explained because they
/// were never identified — the offsets either side of them were measured
/// against real files, and the two checks in [`mesen_reconcile`] confirm the
/// hash field lands where it is expected before anything is written. If Mesen
/// ever moves it, they are what fails.
///
/// Only the hash field is rewritten. The filename is left alone deliberately:
/// once the hash matches, Mesen never consults it, and it is the only record in
/// the file of which base ROM the state was authored against.
mod mesen {
    pub const MAGIC: &[u8; 3] = b"MST";
    pub const FORMAT_VERSION_OFFSET: usize = 7;
    pub const SUPPORTED_FORMAT_VERSION: u32 = 13;
    pub const HASH_OFFSET: usize = 0x0E;
    pub const HASH_LEN: usize = 40;
    /// The length-prefixed ROM filename, immediately after the hash. Not
    /// rewritten — read only as a second, independent confirmation that the hash
    /// field is where this module thinks it is.
    pub const NAME_LEN_OFFSET: usize = HASH_OFFSET + HASH_LEN;
    /// Longer than any path Windows will hand back, and far shorter than a state.
    pub const NAME_LEN_MAX: u32 = 4096;
}

/// snes9x. The header is ASCII — `#!s9xsnp:0014\n`, then length-prefixed named
/// chunks, the first of which is `NAM:000008:Removed\0`. That chunk is where a
/// ROM filename would go if the libretro port kept one, and nothing anywhere in
/// the file is hash-shaped.
const SNES9X_MAGIC: &[u8; 9] = b"#!s9xsnp:";

/// dosbox-pure. Named so it is refused *by name* rather than by falling off the
/// end of the dispatch, because the reason is specific and worth reading: a DOS
/// game is a zip bundle and `content_gen::Console::Dos` will not derive one at
/// all (ADR 0018/0020), so a DOS state should never reach here needing
/// reconciliation. If one does, something upstream is wrong, and the interesting
/// question is what — not whether this header carries a hash.
const DOSBOX_MAGIC: &[u8; 3] = b"GW[";

/// Reconciles `state` with `rom`, rewriting the pairing only if this format
/// records one and it does not already match.
///
/// Errors are deliberately loud and specific. A state this does not recognise is
/// not a state to guess at: a format check exists to catch a mismatched pairing,
/// and shrugging at an unknown header would hand the core a state from a
/// different machine.
pub fn reconcile(state: &[u8], rom: &[u8]) -> Result<Pairing, String> {
    if state.starts_with(SNES9X_MAGIC) {
        return Ok(Pairing::NoIdentity("snes9x"));
    }
    if state.starts_with(DOSBOX_MAGIC) {
        return Err(
            "this is a dosbox-pure state, and a DOS bundle is not content this platform \
             derives — a BPS patch or a value substitution against a zip is meaningless \
             (ADR 0018, ADR 0020). Nothing should have asked for this pairing to be \
             reconciled, so the mistake is upstream of here"
                .into(),
        );
    }
    if state.starts_with(mesen::MAGIC) {
        return mesen_reconcile(state, rom);
    }
    Err(format!(
        "this state's header starts {:02X?} and matches no format this crate has measured. \
         Reconciling a state with a ROM is per-core knowledge and every core owes its own \
         answer (ADR 0019): measure whether this one records a ROM identity before generated \
         content ships for a game that runs on it",
        &state[..state.len().min(4)]
    ))
}

fn mesen_reconcile(state: &[u8], rom: &[u8]) -> Result<Pairing, String> {
    use mesen::*;

    if state.len() < NAME_LEN_OFFSET + 4 {
        return Err(format!(
            "this state is {} bytes, too short to carry a savestate header",
            state.len()
        ));
    }
    let version = u32::from_le_bytes([
        state[FORMAT_VERSION_OFFSET],
        state[FORMAT_VERSION_OFFSET + 1],
        state[FORMAT_VERSION_OFFSET + 2],
        state[FORMAT_VERSION_OFFSET + 3],
    ]);
    if version != SUPPORTED_FORMAT_VERSION {
        return Err(format!(
            "this state is savestate format {version}; rebinding was measured against format \
             {SUPPORTED_FORMAT_VERSION} and the hash field may have moved. Re-measure the \
             header before trusting it"
        ));
    }
    let field = &state[HASH_OFFSET..HASH_OFFSET + HASH_LEN];
    if !field.iter().all(|b| b.is_ascii_hexdigit()) {
        return Err(
            "the 40 bytes where this state should carry a ROM hash are not hex, so this is \
             not the header shape rebinding was measured against"
                .into(),
        );
    }
    let name_len = u32::from_le_bytes([
        state[NAME_LEN_OFFSET],
        state[NAME_LEN_OFFSET + 1],
        state[NAME_LEN_OFFSET + 2],
        state[NAME_LEN_OFFSET + 3],
    ]);
    if name_len > NAME_LEN_MAX || NAME_LEN_OFFSET + 4 + name_len as usize > state.len() {
        return Err(format!(
            "the {name_len} bytes of ROM name this header claims to carry after the hash do \
             not fit the file, so the hash field is not at 0x{HASH_OFFSET:02X} in this state"
        ));
    }
    let from = String::from_utf8_lossy(field).into_owned();
    let to = sha1_hex(rom).to_uppercase();
    if from.eq_ignore_ascii_case(&to) {
        return Ok(Pairing::Bound);
    }

    let mut out = state.to_vec();
    out[HASH_OFFSET..HASH_OFFSET + HASH_LEN].copy_from_slice(to.as_bytes());
    Ok(Pairing::Rebound(Rebound { state: out, from, to }))
}

/// SHA-1, lowercase hex — `content-gen`'s, not a second one.
///
/// This module and that crate both answer the question "what does this ROM hash
/// to", and they have to answer it identically: `content-gen` derives an
/// artifact and reports the hash a session compares across players (ADR 0013),
/// and this module writes that same hash into a copy of the entry state so the
/// core will accept the pairing (ADR 0019). Two implementations of one number is
/// two chances to disagree about which ROM is in hand.
///
/// Re-exported rather than imported at each use site, because the tests below
/// were written against this name and it is what this module means by "the hash
/// of a ROM".
pub use content_gen::sha1_hex;

#[cfg(test)]
mod tests {
    use super::mesen::*;
    use super::*;

    #[test]
    fn sha1_matches_published_vectors() {
        assert_eq!(sha1_hex(b""), "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert_eq!(sha1_hex(b"abc"), "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            sha1_hex(b"The quick brown fox jumps over the lazy dog"),
            "2fd4e1c67a2d28fced849ee1bb76e7391b93eb12"
        );
    }

    /// 55 bytes is the last length whose padding fits in one block, 56 the first
    /// that needs two, and 64 an exact block. A padding bug shows up here or
    /// nowhere.
    #[test]
    fn sha1_handles_block_boundaries() {
        assert_eq!(
            sha1_hex(&[b'a'; 55]),
            "c1c8bbdc22796e28c0e15163d20899b65621d65a"
        );
        assert_eq!(
            sha1_hex(&[b'a'; 56]),
            "c2db330f6083854c99d4b5bfb6e8f29f201be699"
        );
        assert_eq!(
            sha1_hex(&[b'a'; 64]),
            "0098ba824b5c16427bd7a1122a5a442a25ec644d"
        );
        assert_eq!(
            sha1_hex(&[b'a'; 1000]),
            "291e9a6c66994949b57ba5e650361e98fc36b1ba"
        );
    }

    /// A Mesen header shaped like the real one: `MST`, version 0.9.9, format 13,
    /// the three unidentified bytes, the ROM's hash, then a length-prefixed name.
    fn state_bound_to(rom: &[u8]) -> Vec<u8> {
        let mut s = Vec::new();
        s.extend_from_slice(b"MST");
        s.extend_from_slice(&0x0000_0909u32.to_le_bytes());
        s.extend_from_slice(&13u32.to_le_bytes());
        s.extend_from_slice(&[0x04, 0x00, 0x00]);
        assert_eq!(s.len(), HASH_OFFSET);
        s.extend_from_slice(sha1_hex(rom).to_uppercase().as_bytes());
        s.extend_from_slice(&5u32.to_le_bytes());
        s.extend_from_slice(b"a.nes");
        s.extend_from_slice(&[0xAB; 32]); // stand-in for the machine itself
        s
    }

    /// The header the Mesen arm was measured against, so a fixture that drifts
    /// from the real thing fails here rather than in a run.
    #[test]
    fn the_fixture_matches_a_real_state() {
        let real = match std::fs::read("../../assets/states/smb3_1-1.state") {
            Ok(b) => b,
            Err(_) => return, // no assets in this checkout; the ROM tests are manual anyway
        };
        assert_eq!(&real[..3], MAGIC);
        assert_eq!(&real[3..HASH_OFFSET], &state_bound_to(b"")[3..HASH_OFFSET]);
        assert!(real[HASH_OFFSET..NAME_LEN_OFFSET]
            .iter()
            .all(|b| b.is_ascii_hexdigit()));
        assert!(matches!(
            reconcile(&real, b"some other rom").unwrap(),
            Pairing::Rebound(_)
        ));
    }

    #[test]
    fn rebinding_rewrites_only_the_hash() {
        let base = b"the base rom".as_slice();
        let generated = b"the patched rom".as_slice();
        let state = state_bound_to(base);

        let Pairing::Rebound(rebound) = reconcile(&state, generated).unwrap() else {
            panic!("the hashes differ, so the state should have been rewritten");
        };
        assert_eq!(rebound.from, sha1_hex(base).to_uppercase());
        assert_eq!(rebound.to, sha1_hex(generated).to_uppercase());
        assert_eq!(rebound.state.len(), state.len());
        assert_eq!(rebound.state[..HASH_OFFSET], state[..HASH_OFFSET]);
        assert_eq!(
            rebound.state[HASH_OFFSET + HASH_LEN..],
            state[HASH_OFFSET + HASH_LEN..]
        );
    }

    #[test]
    fn a_state_already_bound_to_this_rom_is_left_alone() {
        let rom = b"the base rom".as_slice();
        assert!(matches!(
            reconcile(&state_bound_to(rom), rom).unwrap(),
            Pairing::Bound
        ));
    }

    /// The bug this dispatch exists to fix: a SNES state on a rolled SNES ROM is
    /// a sound pairing that the Mesen-only version reported as a broken one.
    #[test]
    fn a_snes9x_state_records_no_rom_identity_and_needs_no_rebinding() {
        let mut state = Vec::from(*b"#!s9xsnp:0014\n");
        state.extend_from_slice(b"NAM:000008:Removed\0");
        state.extend_from_slice(b"CPU:000048:");
        state.extend_from_slice(&[0xAB; 72]);
        assert!(matches!(
            reconcile(&state, b"any rom at all").unwrap(),
            Pairing::NoIdentity("snes9x")
        ));
    }

    /// And against the real thing, so the magic above cannot drift from what
    /// snes9x writes.
    #[test]
    fn the_snes9x_magic_matches_a_real_state() {
        let Ok(real) = std::fs::read("../../assets/states/dkc_reptile-rumble.state") else {
            return; // no assets in this checkout
        };
        assert!(matches!(
            reconcile(&real, b"a rolled dkc rom").unwrap(),
            Pairing::NoIdentity("snes9x")
        ));
    }

    /// A DOS bundle is not derivable, so this pairing is a symptom rather than a
    /// question — and the error should say which.
    #[test]
    fn a_dosbox_state_is_refused_with_the_reason_it_cannot_be_derived() {
        let mut state = Vec::from(*b"GW[\xd0");
        state.extend_from_slice(&[0u8; 64]);
        let err = reconcile(&state, b"rom").unwrap_err();
        assert!(err.contains("dosbox-pure"), "{err}");
    }

    #[test]
    fn a_state_from_an_unmeasured_core_is_refused_rather_than_guessed_at() {
        let err = reconcile(&[0u8; 4096], b"rom").unwrap_err();
        assert!(err.contains("no format this crate has measured"), "{err}");
    }

    #[test]
    fn an_unmeasured_header_version_is_refused() {
        let mut state = state_bound_to(b"rom");
        state[FORMAT_VERSION_OFFSET] = 14;
        let err = reconcile(&state, b"other rom").unwrap_err();
        assert!(err.contains("format 14"), "{err}");
    }

    #[test]
    fn a_truncated_mesen_state_is_refused() {
        assert!(reconcile(b"MST", b"rom").is_err());
    }
}
