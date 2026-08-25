//! Turning an address a manifest wrote down into a byte.
//!
//! The two harnesses this crate was extracted from disagreed about how to do
//! that, and the disagreement is the whole reason this module is configurable
//! rather than constant.
//!
//! On the NES, Mesen publishes exactly one region through
//! `retro_get_memory_data` — the 2 KB of internal RAM at `$0000-$07FF` — and
//! SMB3 keeps its end-of-level card count at `$7D9C`, in cartridge WRAM. That
//! byte is reachable only through the memory map the core publishes via
//! `SET_MEMORY_MAPS`, so on the NES the map is the primary route and the
//! system-RAM slice is the fallback.
//!
//! On the SNES, snes9x publishes all 128 KB of work RAM (`$7E:0000-$7F:FFFF`)
//! as system RAM, every address DKC cares about is inside it, and the core
//! publishes no descriptors at all. There the slice is primary and the map is
//! the fallback for anything outside work RAM.
//!
//! So a manifest states which way round its console is, and both games get
//! exactly the behaviour they had before. Neither ordering is a default that
//! quietly works for the other game: getting it backwards on the NES loses the
//! card detector, and there is no way to notice that from the output.

use std::fmt;

use crate::libretro;

/// How wide a watched value is. Both consoles in the slice are little-endian,
/// but a manifest states its endianness anyway, because the next console
/// (Genesis) is not.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    U8,
    U16,
}

impl Width {
    pub fn parse(s: &str) -> Result<Width, String> {
        match s {
            "u8" | "8" => Ok(Width::U8),
            "u16" | "16" => Ok(Width::U16),
            other => Err(format!("width must be u8 or u16, got `{other}`")),
        }
    }

    pub fn bytes(self) -> usize {
        match self {
            Width::U8 => 1,
            Width::U16 => 2,
        }
    }
}

impl fmt::Display for Width {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Width::U8 => write!(f, "u8"),
            Width::U16 => write!(f, "u16"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Endian {
    Little,
    Big,
}

/// Which route to a byte is tried first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Prefer {
    /// The `RETRO_MEMORY_SYSTEM_RAM` slice. Right for a console whose whole
    /// interesting address range arrives that way (SNES).
    SystemRam,
    /// The descriptors the core published through `SET_MEMORY_MAPS`. Right for
    /// a console where the interesting bytes are outside system RAM (NES).
    MemoryMap,
}

/// The address space a manifest's addresses are written in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct AddressSpace {
    /// The address the system-RAM slice starts at, in the console's own
    /// numbering. `$7E0000` on the SNES, `$0000` on the NES.
    pub base: usize,
    /// How many bytes of it the slice is expected to cover.
    pub size: usize,
    pub prefer: Prefer,
    pub endian: Endian,
}

impl AddressSpace {
    /// Folds an address onto an offset into the system-RAM slice. `None` for
    /// anything outside it, which then has to go through the core's map.
    ///
    /// Both spellings of an SNES address mean the same byte: `0x0577` is a
    /// WRAM offset and `0x7E0577` is the same byte named in full. Accepting
    /// both matters because published RAM maps use whichever their author
    /// preferred, and a manifest should be able to quote its source verbatim.
    pub fn offset(&self, addr: usize) -> Option<usize> {
        if self.base == 0 {
            // Nothing to fold: the address *is* the offset. What bounds a read
            // is the slice the core handed over, not `size` — which is how the
            // NES ends up correctly reporting `$7D9C` as unreadable through
            // this route while the memory map answers for it.
            return Some(addr);
        }
        if addr < self.size {
            return Some(addr);
        }
        if addr >= self.base && addr - self.base < self.size {
            return Some(addr - self.base);
        }
        None
    }

    /// Reads one byte, trying the preferred route first and the other second.
    pub fn read_u8(&self, addr: usize, ram: &[u8]) -> Option<u8> {
        let from_slice = || self.offset(addr).and_then(|off| ram.get(off).copied());
        match self.prefer {
            Prefer::SystemRam => from_slice().or_else(|| libretro::read_addr(addr)),
            Prefer::MemoryMap => libretro::read_addr(addr).or_else(from_slice),
        }
    }

    /// Reads a value of `width` at `addr`, in this space's byte order.
    pub fn read(&self, addr: usize, width: Width, ram: &[u8]) -> Option<u32> {
        match width {
            Width::U8 => self.read_u8(addr, ram).map(u32::from),
            Width::U16 => {
                let a = self.read_u8(addr, ram)?;
                let b = self.read_u8(addr + 1, ram)?;
                let (lo, hi) = match self.endian {
                    Endian::Little => (a, b),
                    Endian::Big => (b, a),
                };
                Some(u32::from(lo) | (u32::from(hi) << 8))
            }
        }
    }

    /// One canonical spelling for every address this crate prints, so anything
    /// in the log pastes straight back into `--addr` or `--watch`.
    pub fn show(&self, addr: usize) -> String {
        match self.offset(addr) {
            Some(off) => {
                let full = off + self.base;
                if self.base == 0 && self.size <= 0x1_0000 {
                    format!("${full:04X}")
                } else {
                    format!("${full:06X}")
                }
            }
            None if self.base == 0 && addr <= 0xFFFF => format!("${addr:04X}"),
            None => format!("${addr:06X}"),
        }
    }
}

/// One watched quantity: where it lives and how wide it is.
///
/// `at: None` is a first-class state, not a missing value. It means "nobody has
/// found this address yet", and every detector built on it stays quiet rather
/// than guessing — which is how DKC's damage detector spent a week honestly
/// reporting nothing instead of dishonestly reporting noise.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Field {
    pub at: Option<usize>,
    pub width: Width,
}

impl Field {
    pub fn read(&self, space: &AddressSpace, ram: &[u8]) -> Option<u32> {
        space.read(self.at?, self.width, ram)
    }

    pub fn show(&self, space: &AddressSpace) -> String {
        match self.at {
            Some(a) => format!("{} {}", space.show(a), self.width),
            None => "unset".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snes() -> AddressSpace {
        AddressSpace {
            base: 0x7E_0000,
            size: 0x2_0000,
            prefer: Prefer::SystemRam,
            endian: Endian::Little,
        }
    }

    fn nes() -> AddressSpace {
        AddressSpace {
            base: 0x0000,
            size: 0x0800,
            prefer: Prefer::MemoryMap,
            endian: Endian::Little,
        }
    }

    #[test]
    fn full_snes_addresses_fold_onto_wram_offsets() {
        assert_eq!(snes().offset(0x7E_0577), Some(0x0577));
        assert_eq!(snes().offset(0x0577), Some(0x0577));
        // The upper half of work RAM is $7F:0000 and up.
        assert_eq!(snes().offset(0x7F_0000), Some(0x1_0000));
        // Cartridge space is not work RAM; the core's map answers for it.
        assert_eq!(snes().offset(0x70_0000), None);
    }

    /// On the NES there is no second spelling, and the slice the core handed
    /// over is what bounds a read: $7D9C is past internal RAM, so only the
    /// memory map can reach it. Under `cargo test` no core has published one,
    /// so it reads as unmapped rather than as some other byte.
    #[test]
    fn nes_addresses_past_internal_ram_read_as_unmapped() {
        assert_eq!(nes().offset(0x0014), Some(0x0014));
        assert_eq!(nes().read_u8(0x0014, &vec![0u8; 0x0800]), Some(0));
        assert_eq!(nes().read_u8(0x7D9C, &vec![0u8; 0x0800]), None);
    }

    #[test]
    fn sixteen_bit_fields_read_in_the_declared_byte_order() {
        let mut ram = vec![0u8; 0x2_0000];
        ram[0x0577] = 0x34;
        ram[0x0578] = 0x12;
        assert_eq!(snes().read(0x0577, Width::U16, &ram), Some(0x1234));
        assert_eq!(snes().read(0x0577, Width::U8, &ram), Some(0x34));

        let big = AddressSpace { endian: Endian::Big, ..snes() };
        assert_eq!(big.read(0x0577, Width::U16, &ram), Some(0x3412));
    }

    #[test]
    fn an_unset_field_reads_as_none() {
        let f = Field { at: None, width: Width::U16 };
        assert_eq!(f.read(&snes(), &vec![0u8; 0x2_0000]), None);
    }

    /// Output has to paste back into a flag, so an offset and its full address
    /// print identically however the manifest spelled it.
    #[test]
    fn addresses_print_in_one_canonical_form() {
        assert_eq!(snes().show(0x0577), "$7E0577");
        assert_eq!(snes().show(0x7E_0577), "$7E0577");
        assert_eq!(snes().show(0x1_0000), "$7F0000");
        assert_eq!(snes().show(0x70_0000), "$700000");
        assert_eq!(nes().show(0x0014), "$0014");
        assert_eq!(nes().show(0x7D9C), "$7D9C");
    }
}
