//! What a console family costs the engine: where its ROM's data starts, and
//! what has to be recomputed after a byte of it changes.
//!
//! ADR 0015 predicted exactly this and no more — "per-console-family
//! checksum/header fixup where the ROM format requires one (SNES's internal
//! header checksum; NES has none) ... a small, closed set reviewed once per new
//! console, not something that grows per game or per contributor". That is the
//! whole of this module, and it is the only place in the crate where the word
//! for a machine appears at all. A slot table, a BPS patch and a derivation key
//! are the same for every console.
//!
//! The manifest's `[game].console` is what picks the variant, which is the field
//! ADR 0015 said would key it. Nothing else in the platform switches on that
//! field: a DOS game is a DOS game because of its `[runner.libretro]`, not
//! because of its name (`docs/manifest-schema.md`).

use crate::hash::sha1_hex;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Console {
    Nes,
    Snes,
    /// Named so it can be *refused* by name rather than by falling off the end
    /// of a match. ADR 0020: a BPS patch against a DOS bundle is meaningless
    /// because the bundle is a zip whose hash depends on how it was zipped, and
    /// a value substitution into a compressed member is worse. A DOS
    /// alternative is a member-level overlay and waits on the member hashing
    /// `asset-store` owes (ADR 0018).
    Dos,
}

impl Console {
    pub fn parse(s: &str) -> Result<Console, String> {
        match s {
            "nes" => Ok(Console::Nes),
            "snes" => Ok(Console::Snes),
            "dos" => Ok(Console::Dos),
            other => Err(format!(
                "console `{other}` is not one this engine has a header and checksum rule for; \
                 nes, snes and dos are what exist. Adding one is a reviewed change to \
                 content-gen, once per console family (ADR 0015)"
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Console::Nes => "nes",
            Console::Snes => "snes",
            Console::Dos => "dos",
        }
    }

    /// Whether this crate will touch the bytes of this console's content at all.
    pub fn is_derivable(self) -> Result<(), String> {
        match self {
            Console::Dos => Err("a DOS game is a zip bundle, not a cartridge ROM: a BPS patch \
                                 against it is meaningless and a value substitution into it \
                                 would be worse. A DOS alternative is a member-level overlay, \
                                 and it waits on asset-store hashing a bundle's members \
                                 (ADR 0018, ADR 0020)"
                .to_string()),
            _ => Ok(()),
        }
    }

    /// How many bytes of the file come before the offset a manifest's
    /// `patch_slot` counts from.
    ///
    /// Sniffed from the content rather than read from the manifest's `header`,
    /// deliberately: `header` belongs to the base ROM only (ADR 0020) and this
    /// has to answer for a derived one too. Anything it does not recognise is
    /// refused — guessing zero here would write every slot 16 bytes early, which
    /// is the quietest possible way to corrupt a ROM.
    pub fn header_bytes(self, rom: &[u8]) -> Result<usize, String> {
        match self {
            Console::Nes => {
                if rom.len() >= 16 && &rom[..4] == b"NES\x1a" {
                    Ok(16)
                } else {
                    Err("this does not start with an iNES header, so there is no way to know \
                         what a patch_slot offset is counted from"
                        .to_string())
                }
            }
            // The SNES has no magic number at all. All that can be told from the
            // file is whether a 512-byte copier header is stapled to the front,
            // which is what `header = "copier-optional"` says in a manifest and
            // what shifts every offset in a table if it is missed.
            Console::Snes => Ok(if rom.len() % 1024 == 512 { 512 } else { 0 }),
            Console::Dos => Err(self.is_derivable().unwrap_err()),
        }
    }

    /// Recomputes whatever the ROM format keeps a checksum of, in place.
    ///
    /// `original` is the content as it was before the substitution, and it is
    /// not a convenience: on the SNES it is how the internal header is *found*.
    /// A valid ROM's stored checksum equals the sum of its own bytes, so the
    /// candidate offset where that holds is the header — and if no candidate
    /// holds, this refuses rather than writing a checksum into a guessed
    /// address.
    pub fn fixup(self, original: &[u8], rom: &mut [u8]) -> Result<Option<Fixup>, String> {
        match self {
            // The NES has none. An iNES header carries no checksum, the mapper
            // does not care, and Mesen keys its database on the file's SHA-1 —
            // which ADR 0019 rewrites into the entry state instead.
            Console::Nes => Ok(None),
            Console::Snes => snes_fixup(original, rom).map(Some),
            Console::Dos => Err(self.is_derivable().unwrap_err()),
        }
    }
}

/// What a fixup changed, for the log. A generated ROM that silently differs from
/// the one a golden hash was taken on is the failure this exists to make visible.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Fixup {
    /// Where the console's internal header was found, as a file offset.
    pub header_at: usize,
    pub was: u16,
    pub now: u16,
}

/// The SNES header's checksum pair, as offsets from the header's start.
const SNES_COMPLEMENT: usize = 0x1C;
const SNES_CHECKSUM: usize = 0x1E;
/// LoROM, HiROM, ExHiROM. Every mapper puts its header at one of these three,
/// after any copier header.
const SNES_HEADERS: [usize; 3] = [0x7FC0, 0xFFC0, 0x40FFC0];

/// Sum the bytes, keep the low 16 bits. The checksum field and its complement
/// are summed as they stand, which works because the two always contribute
/// `0x1FE` between them however they are set — that self-consistency is the
/// whole trick, and it is why the sum does not have to be computed with the
/// field blanked out.
fn snes_sum(data: &[u8]) -> u16 {
    if data.len().is_power_of_two() {
        return data.iter().fold(0u32, |a, b| a + u32::from(*b)) as u16;
    }
    // A ROM whose size is not a power of two is mirrored up to one by the
    // cartridge, and its checksum counts the mirror. Split at the largest power
    // of two, sum that, then sum the remainder as many times as it repeats to
    // fill the rest.
    let half = data.len().next_power_of_two() / 2;
    let (low, rest) = data.split_at(half);
    let low_sum = u32::from(snes_sum(low));
    let rest_sum = u32::from(snes_sum(rest));
    let repeats = (half / rest.len().next_power_of_two()) as u32;
    (low_sum + rest_sum * repeats.max(1)) as u16
}

fn snes_fixup(original: &[u8], rom: &mut [u8]) -> Result<Fixup, String> {
    let skip = if original.len() % 1024 == 512 { 512 } else { 0 };
    let body = &original[skip..];
    let expected = snes_sum(body);

    let mut found: Option<usize> = None;
    for candidate in SNES_HEADERS {
        let at = skip + candidate;
        if at + 0x20 > original.len() {
            continue;
        }
        let stored = u16::from_le_bytes([original[at + SNES_CHECKSUM], original[at + SNES_CHECKSUM + 1]]);
        let complement =
            u16::from_le_bytes([original[at + SNES_COMPLEMENT], original[at + SNES_COMPLEMENT + 1]]);
        if stored ^ complement == 0xFFFF && stored == expected {
            if found.is_some() {
                return Err("two SNES header positions both check out, which cannot be told \
                            apart from here"
                    .to_string());
            }
            found = Some(at);
        }
    }

    let at = found.ok_or_else(|| {
        format!(
            "no SNES internal header at $7FC0, $FFC0 or $40FFC0 holds a checksum matching this \
             ROM (the sum of its bytes is {expected:04X}). Without one there is nowhere to \
             write a corrected checksum, and writing it into a guessed address would corrupt \
             the ROM in a way nothing downstream would notice"
        )
    })?;

    let now = snes_sum(&rom[skip..]);
    // The field is inside the region being summed, so setting it changes the
    // sum by exactly the delta this line removes and re-adds — which is why the
    // pair is written as (sum, !sum) and the two together stay constant.
    let was = u16::from_le_bytes([rom[at + SNES_CHECKSUM], rom[at + SNES_CHECKSUM + 1]]);
    rom[at + SNES_CHECKSUM..at + SNES_CHECKSUM + 2].copy_from_slice(&now.to_le_bytes());
    rom[at + SNES_COMPLEMENT..at + SNES_COMPLEMENT + 2].copy_from_slice(&(!now).to_le_bytes());
    Ok(Fixup { header_at: at, was, now })
}

/// The SHA-1 an artifact is identified by, in the spelling manifests and
/// `docs/games/*.md` use.
pub fn identity(rom: &[u8]) -> String {
    sha1_hex(rom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dos_bundle_is_refused_by_name_rather_than_by_accident() {
        let e = Console::Dos.header_bytes(b"PK\x03\x04").unwrap_err();
        assert!(e.contains("member-level overlay"), "{e}");
        assert!(Console::Dos.is_derivable().is_err());
    }

    #[test]
    fn an_ines_header_is_sixteen_bytes_and_anything_else_is_refused() {
        let mut rom = b"NES\x1a".to_vec();
        rom.extend_from_slice(&[0u8; 32]);
        assert_eq!(Console::Nes.header_bytes(&rom).unwrap(), 16);
        let e = Console::Nes.header_bytes(&[0u8; 64]).unwrap_err();
        assert!(e.contains("iNES header"), "{e}");
    }

    #[test]
    fn a_snes_copier_header_is_recognised_by_the_size_alone() {
        assert_eq!(Console::Snes.header_bytes(&vec![0u8; 32768]).unwrap(), 0);
        assert_eq!(Console::Snes.header_bytes(&vec![0u8; 32768 + 512]).unwrap(), 512);
    }

    #[test]
    fn the_nes_has_no_checksum_to_fix() {
        let mut rom = vec![0u8; 64];
        assert_eq!(Console::Nes.fixup(&rom.clone(), &mut rom).unwrap(), None);
    }

    /// A ROM whose header does not check out gets no checksum written into it,
    /// because there is nowhere to write one that is known to be right.
    #[test]
    fn a_snes_rom_with_no_consistent_header_is_refused() {
        let mut rom = vec![0u8; 0x10000];
        let e = Console::Snes.fixup(&rom.clone(), &mut rom).unwrap_err();
        assert!(e.contains("no SNES internal header"), "{e}");
    }

    /// A synthetic LoROM: build a ROM, make its header self-consistent, then
    /// change a byte and watch the checksum follow.
    #[test]
    fn a_snes_checksum_follows_a_substitution() {
        let mut rom = vec![0u8; 0x8000];
        rom[0x1000] = 0x40;
        // Solve for the pair: everything but the pair contributes `rest`, and
        // the pair itself always contributes 0x1FE.
        let rest: u32 = rom.iter().fold(0u32, |a, b| a + u32::from(*b));
        let sum = (rest + 0x1FE) as u16;
        rom[0x7FC0 + SNES_CHECKSUM..0x7FC0 + SNES_CHECKSUM + 2]
            .copy_from_slice(&sum.to_le_bytes());
        rom[0x7FC0 + SNES_COMPLEMENT..0x7FC0 + SNES_COMPLEMENT + 2]
            .copy_from_slice(&(!sum).to_le_bytes());
        assert_eq!(snes_sum(&rom), sum);

        let original = rom.clone();
        rom[0x1000] = 0x41;
        let fix = Console::Snes.fixup(&original, &mut rom).unwrap().unwrap();
        assert_eq!(fix.header_at, 0x7FC0);
        assert_eq!(fix.now, sum.wrapping_add(1));
        // And the result is self-consistent, which is the property a core checks.
        assert_eq!(snes_sum(&rom), fix.now);
    }
}
