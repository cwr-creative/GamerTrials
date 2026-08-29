//! The patch-slot table: what a contributor declares, and what the engine reads.
//!
//! ADR 0015 scopes a generated challenge to **declarative value substitution**,
//! and this module is that scope written as types. A slot is an offset, a width
//! and a source of legal values. There is no way to spell "insert an object",
//! "move this table" or "make the level longer", and that is not an omission:
//! structural generation needs per-game structural knowledge, which is exactly
//! what invariant 5 exists to keep out of this crate.
//!
//! The schema lives here rather than in the manifest loader because the engine
//! that reads it is here. `session-libretro` deserializes
//! `[[challenge.patch_slot]]` straight into [`PatchSlotDoc`] and hands the
//! result to [`PatchTable::build`]; if the shape ever changes, one crate changes
//! and the loader follows for free.
//!
//! Two rules in here are load-bearing and easy to "clean up" by accident:
//!
//! * **A pool's duplicates are weights.** `[0x72, 0x72, 0x72, 0x72, 0x81, 0x82,
//!   0x86, 0x87]` is SMB3's Bro Roulette: a coin flip on whether a goomba is
//!   replaced, then an even choice among four bros. Deduplicating that pool
//!   turns a 50% roll into a 20% one without saying so, and duplication is the
//!   only way this schema can express a non-uniform roll at all
//!   (`docs/games/smb3.md`).
//! * **A group is a permutation, not a correlated roll.** Its pool is consumed
//!   without replacement, which is how "shuffle which enemy stands in which
//!   existing spawn slot" is spelled. It is the wrong tool for "three slots that
//!   should agree", and there is deliberately no tool for that.

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;

// ------------------------------------------------------------- raw documents

/// One `[[challenge.patch_slot]]`, as written in a manifest.
#[derive(Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct PatchSlotDoc {
    /// Into the ROM the challenge's stage runs on, **after any header**. A
    /// derived ROM is a base ROM in this respect too: offsets into a hack are
    /// into the hack (ADR 0020).
    pub offset: i64,
    pub width: String,
    #[serde(default)]
    pub endian: Option<String>,
    pub values: ValuesDoc,
    #[serde(default)]
    pub note: Option<String>,
}

/// Exactly one of the three, which is checked rather than assumed.
#[derive(Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct ValuesDoc {
    #[serde(default)]
    pub pool: Option<Vec<i64>>,
    #[serde(default)]
    pub range: Option<RangeDoc>,
    #[serde(default)]
    pub group: Option<String>,
}

/// An inclusive numeric range. Inclusive because a manifest author is writing
/// down the legal values of a game field, and "1 to 8" means eight of them.
#[derive(Clone, Copy, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct RangeDoc {
    pub min: i64,
    pub max: i64,
}

/// One `[challenge.group.<name>]`.
#[derive(Clone, Deserialize, Debug)]
#[serde(deny_unknown_fields)]
pub struct GroupDoc {
    pub pool: Vec<i64>,
    pub mode: String,
}

// ----------------------------------------------------------- validated shape

/// How wide the value written at a slot is. Not the same type as
/// `session-libretro`'s watch width and deliberately not shared: that one
/// addresses an emulated machine's RAM through a memory route, this one is a
/// byte offset into a file. They agree on three names today and nothing forces
/// them to keep agreeing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    U8,
    U16,
    U32,
}

impl Width {
    pub fn bytes(self) -> usize {
        match self {
            Width::U8 => 1,
            Width::U16 => 2,
            Width::U32 => 4,
        }
    }

    /// The largest value that fits. `u32::MAX` for `u32`, which is why this
    /// returns a `u64`.
    fn max(self) -> u64 {
        match self {
            Width::U8 => u64::from(u8::MAX),
            Width::U16 => u64::from(u16::MAX),
            Width::U32 => u64::from(u32::MAX),
        }
    }

    fn parse(s: &str) -> Result<Width, String> {
        match s {
            "u8" => Ok(Width::U8),
            "u16" => Ok(Width::U16),
            "u32" => Ok(Width::U32),
            other => Err(format!("width `{other}`; try u8, u16 or u32")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Width::U8 => "u8",
            Width::U16 => "u16",
            Width::U32 => "u32",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Endian {
    Little,
    Big,
}

impl Endian {
    fn parse(s: &str) -> Result<Endian, String> {
        match s {
            "little" => Ok(Endian::Little),
            "big" => Ok(Endian::Big),
            other => Err(format!("endian `{other}`; try little or big")),
        }
    }

    pub(crate) fn write(self, value: u32, width: Width, into: &mut [u8]) {
        let bytes = match self {
            Endian::Little => value.to_le_bytes(),
            Endian::Big => value.to_be_bytes(),
        };
        let n = width.bytes();
        match self {
            // The low `n` bytes either way; big-endian takes them from the
            // right-hand end of the 4-byte encoding.
            Endian::Little => into[..n].copy_from_slice(&bytes[..n]),
            Endian::Big => into[..n].copy_from_slice(&bytes[4 - n..]),
        }
    }

    pub(crate) fn read(self, width: Width, from: &[u8]) -> u32 {
        let n = width.bytes();
        let mut acc = 0u32;
        match self {
            Endian::Little => {
                for (i, b) in from[..n].iter().enumerate() {
                    acc |= u32::from(*b) << (8 * i);
                }
            }
            Endian::Big => {
                for b in &from[..n] {
                    acc = (acc << 8) | u32::from(*b);
                }
            }
        }
        acc
    }
}

/// Where a slot's value comes from. A closed set, and the closure is the
/// decision: ADR 0015 refused an opt-in path to anything richer.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ValueSource {
    /// An explicit list of legal values. **Duplicates are weights** and are
    /// never deduplicated.
    Pool(Vec<u32>),
    /// Every value from `min` to `max`, inclusive, equally likely.
    Range { min: u32, max: u32 },
    /// One member of a named group, whose pool is consumed by permutation.
    Group(String),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PatchSlot {
    pub offset: usize,
    pub width: Width,
    pub endian: Endian,
    pub values: ValueSource,
    pub note: Option<String>,
}

impl PatchSlot {
    /// The bytes this slot occupies, for the overlap check and the bounds check.
    fn span(&self) -> std::ops::Range<usize> {
        self.offset..self.offset + self.width.bytes()
    }
}

/// How a group's pool is consumed. One variant, named rather than implied,
/// because a second one is a schema decision and should have to be written
/// down.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GroupMode {
    /// Drawn without replacement: each slot in the group gets a different
    /// member of the pool.
    Permutation,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Group {
    pub pool: Vec<u32>,
    pub mode: GroupMode,
    /// How many slots draw from it. Recorded at build time because it is what
    /// makes the pool large enough or not.
    pub members: usize,
}

/// One challenge's whole declaration of how its ROM is derived.
///
/// `generator_id` and `generator_version` are two thirds of ADR 0013's
/// derivation key. The *id* is the challenge's, because under ADR 0015 the
/// generator is not a piece of code — the engine is generic — it is this table.
/// Two challenges with different slots are different generators even though
/// they share every line of Rust, and keying on the challenge id is what makes
/// their artifacts distinct.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PatchTable {
    pub generator_id: String,
    pub generator_version: u32,
    pub slots: Vec<PatchSlot>,
    pub groups: BTreeMap<String, Group>,
}

impl PatchTable {
    /// Validates a manifest's declaration into something the engine can run.
    ///
    /// Everything checkable without the ROM is checked here, because a generated
    /// challenge that fails at deal time is a session aborting on a player.
    /// What is *not* checkable is whether a declared value is safe — whether it
    /// crashes the level or makes it uncompletable — and that is the author's,
    /// established by playtesting, on the same trust boundary a `[watch]`
    /// address already sits on (ADR 0015).
    pub fn build(
        generator_id: &str,
        generator_version: u32,
        slots: &[PatchSlotDoc],
        groups: &BTreeMap<String, GroupDoc>,
    ) -> Result<PatchTable, String> {
        if slots.is_empty() {
            return Err("declares a generator but no patch_slot; a roll with no slots \
                        derives the ROM it started from"
                .to_string());
        }

        let mut built: Vec<PatchSlot> = Vec::with_capacity(slots.len());
        let mut group_members: BTreeMap<String, usize> = BTreeMap::new();

        for (i, doc) in slots.iter().enumerate() {
            let what = format!("patch_slot {} (offset {:#X})", i + 1, doc.offset.max(0));
            let offset = usize::try_from(doc.offset)
                .map_err(|_| format!("{what}: offset cannot be negative"))?;
            let width = Width::parse(&doc.width).map_err(|e| format!("{what}: {e}"))?;
            let endian = match (&doc.endian, width) {
                (None, _) => Endian::Little,
                // Refused rather than ignored, for the reason every table in
                // this project denies unknown keys: a key that parses and does
                // nothing reads as a decision that was made.
                (Some(_), Width::U8) => {
                    return Err(format!(
                        "{what}: `endian` on a u8 slot decides nothing; remove it"
                    ))
                }
                (Some(e), _) => Endian::parse(e).map_err(|x| format!("{what}: {x}"))?,
            };

            let v = &doc.values;
            let values = match (&v.pool, &v.range, &v.group) {
                (Some(pool), None, None) => {
                    if pool.is_empty() {
                        return Err(format!("{what}: an empty pool has nothing to roll"));
                    }
                    ValueSource::Pool(fit_all(pool, width, &what)?)
                }
                (None, Some(r), None) => {
                    let min = fit(r.min, width, &what)?;
                    let max = fit(r.max, width, &what)?;
                    if min > max {
                        return Err(format!("{what}: range min {min} is above max {max}"));
                    }
                    ValueSource::Range { min, max }
                }
                (None, None, Some(name)) => {
                    *group_members.entry(name.clone()).or_insert(0) += 1;
                    ValueSource::Group(name.clone())
                }
                (None, None, None) => {
                    return Err(format!(
                        "{what}: `values` needs one of pool, range or group"
                    ))
                }
                _ => {
                    return Err(format!(
                        "{what}: `values` takes exactly one of pool, range or group"
                    ))
                }
            };

            let slot = PatchSlot { offset, width, endian, values, note: doc.note.clone() };
            // Two slots writing the same bytes means one of them decides
            // nothing, and which one depends on iteration order. That is a
            // manifest bug worth naming.
            if let Some(clash) = built.iter().find(|s| overlaps(&s.span(), &slot.span())) {
                return Err(format!(
                    "{what}: overlaps the slot at offset {:#X}; two slots cannot roll the \
                     same bytes",
                    clash.offset
                ));
            }
            built.push(slot);
        }

        let mut out_groups: BTreeMap<String, Group> = BTreeMap::new();
        for (name, doc) in groups {
            let what = format!("group `{name}`");
            let mode = match doc.mode.as_str() {
                "permutation" => GroupMode::Permutation,
                other => {
                    return Err(format!(
                        "{what}: mode `{other}`; permutation is the only one there is"
                    ))
                }
            };
            let members = *group_members.get(name).unwrap_or(&0);
            if members == 0 {
                return Err(format!("{what}: no patch_slot draws from it"));
            }
            // Every member slot's width has to hold every pool value, because a
            // permutation can put any value in any slot.
            let mut pool: Option<Vec<u32>> = None;
            for slot in built
                .iter()
                .filter(|s| matches!(&s.values, ValueSource::Group(g) if g == name))
            {
                pool = Some(fit_all(&doc.pool, slot.width, &what)?);
            }
            let pool = pool.expect("a group with no members was refused above");
            if pool.is_empty() {
                return Err(format!("{what}: an empty pool has nothing to roll"));
            }
            if pool.len() < members {
                return Err(format!(
                    "{what}: {members} slots draw from a pool of {}; a permutation is drawn \
                     without replacement, so the pool cannot be smaller than the group",
                    pool.len()
                ));
            }
            out_groups.insert(name.clone(), Group { pool, mode, members });
        }

        let declared: BTreeSet<&String> = out_groups.keys().collect();
        for name in group_members.keys() {
            if !declared.contains(name) {
                return Err(format!(
                    "patch_slot draws from group `{name}`, which no [challenge.group.{name}] \
                     declares"
                ));
            }
        }

        Ok(PatchTable {
            generator_id: generator_id.to_string(),
            generator_version,
            slots: built,
            groups: out_groups,
        })
    }

    /// The last offset this table writes to. A caller with the ROM in hand uses
    /// it to say "this table is not for this ROM" before rolling anything.
    pub fn reach(&self) -> usize {
        self.slots.iter().map(|s| s.span().end).max().unwrap_or(0)
    }
}

fn overlaps(a: &std::ops::Range<usize>, b: &std::ops::Range<usize>) -> bool {
    a.start < b.end && b.start < a.end
}

fn fit(v: i64, width: Width, what: &str) -> Result<u32, String> {
    u64::try_from(v)
        .ok()
        .filter(|&x| x <= width.max())
        .and_then(|x| u32::try_from(x).ok())
        .ok_or_else(|| format!("{what}: {v} does not fit in a {}", width.name()))
}

fn fit_all(values: &[i64], width: Width, what: &str) -> Result<Vec<u32>, String> {
    values.iter().map(|v| fit(*v, width, what)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn slot(offset: i64, width: &str, values: ValuesDoc) -> PatchSlotDoc {
        PatchSlotDoc { offset, width: width.to_string(), endian: None, values, note: None }
    }

    fn pool(v: &[i64]) -> ValuesDoc {
        ValuesDoc { pool: Some(v.to_vec()), range: None, group: None }
    }

    fn group(name: &str) -> ValuesDoc {
        ValuesDoc { pool: None, range: None, group: Some(name.to_string()) }
    }

    fn build(slots: &[PatchSlotDoc], groups: &[(&str, GroupDoc)]) -> Result<PatchTable, String> {
        let g: BTreeMap<String, GroupDoc> =
            groups.iter().map(|(n, d)| ((*n).to_string(), d.clone())).collect();
        PatchTable::build("a-challenge", 1, slots, &g)
    }

    /// SMB3's Sun Storm, as `docs/games/smb3.md` records it.
    #[test]
    fn a_pool_table_builds() {
        let t = build(
            &[
                slot(0xC6BD, "u8", pool(&[0xAF, 0xBB])),
                slot(0xC6C0, "u8", pool(&[0xAF, 0xBB])),
                slot(0xC6C3, "u8", pool(&[0xAF])),
            ],
            &[],
        )
        .unwrap();
        assert_eq!(t.slots.len(), 3);
        assert_eq!(t.reach(), 0xC6C4);
    }

    /// The single most consequential line in this module: Bro Roulette's pool is
    /// four goombas against four bros, and deduplicating it would silently turn
    /// a 50% roll into a 20% one.
    #[test]
    fn a_pools_duplicates_survive_the_build() {
        let t = build(
            &[slot(0xC6CB, "u8", pool(&[0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87]))],
            &[],
        )
        .unwrap();
        match &t.slots[0].values {
            ValueSource::Pool(p) => {
                assert_eq!(p.len(), 8);
                assert_eq!(p.iter().filter(|v| **v == 0x72).count(), 4);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_value_too_wide_for_its_slot_is_refused() {
        let e = build(&[slot(0x10, "u8", pool(&[0x100]))], &[]).unwrap_err();
        assert!(e.contains("does not fit in a u8"), "{e}");
    }

    #[test]
    fn two_sources_on_one_slot_are_refused() {
        let v = ValuesDoc { pool: Some(vec![1]), range: None, group: Some("g".into()) };
        let e = build(&[slot(0x10, "u8", v)], &[]).unwrap_err();
        assert!(e.contains("exactly one"), "{e}");
    }

    #[test]
    fn overlapping_slots_are_refused() {
        let e = build(
            &[slot(0x10, "u16", pool(&[1])), slot(0x11, "u8", pool(&[2]))],
            &[],
        )
        .unwrap_err();
        assert!(e.contains("overlaps"), "{e}");
    }

    #[test]
    fn a_group_smaller_than_the_slots_drawing_from_it_is_refused() {
        let e = build(
            &[slot(0x10, "u8", group("g")), slot(0x11, "u8", group("g"))],
            &[("g", GroupDoc { pool: vec![1], mode: "permutation".into() })],
        )
        .unwrap_err();
        assert!(e.contains("without replacement"), "{e}");
    }

    #[test]
    fn a_slot_naming_an_undeclared_group_is_refused() {
        let e = build(&[slot(0x10, "u8", group("nope"))], &[]).unwrap_err();
        assert!(e.contains("no [challenge.group.nope]"), "{e}");
    }

    #[test]
    fn a_group_nothing_draws_from_is_refused() {
        let e = build(
            &[slot(0x10, "u8", pool(&[1]))],
            &[("g", GroupDoc { pool: vec![1, 2], mode: "permutation".into() })],
        )
        .unwrap_err();
        assert!(e.contains("no patch_slot draws from it"), "{e}");
    }

    #[test]
    fn endian_on_a_byte_is_refused_rather_than_ignored() {
        let mut s = slot(0x10, "u8", pool(&[1]));
        s.endian = Some("big".into());
        let e = build(&[s], &[]).unwrap_err();
        assert!(e.contains("decides nothing"), "{e}");
    }

    #[test]
    fn a_backwards_range_is_refused() {
        let v = ValuesDoc {
            pool: None,
            range: Some(RangeDoc { min: 8, max: 1 }),
            group: None,
        };
        let e = build(&[slot(0x10, "u8", v)], &[]).unwrap_err();
        assert!(e.contains("above max"), "{e}");
    }

    #[test]
    fn widths_write_in_the_endianness_they_declare() {
        let mut buf = [0u8; 4];
        Endian::Little.write(0x1234, Width::U16, &mut buf);
        assert_eq!(&buf[..2], &[0x34, 0x12]);
        Endian::Big.write(0x1234, Width::U16, &mut buf);
        assert_eq!(&buf[..2], &[0x12, 0x34]);
        assert_eq!(Endian::Little.read(Width::U16, &[0x34, 0x12]), 0x1234);
        assert_eq!(Endian::Big.read(Width::U16, &[0x12, 0x34]), 0x1234);
    }
}
