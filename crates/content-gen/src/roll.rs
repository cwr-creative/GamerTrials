//! Turning a seed into an artifact.
//!
//! ADR 0013's rule is that every player generates the *same* bytes from the
//! *same* key on their own machine, and that a mismatch aborts the challenge
//! rather than letting two people race on different content. That makes every
//! line in this file part of a wire contract, even though nothing here touches
//! a wire:
//!
//! * **The random number generator is written out here**, not taken from a
//!   crate. `rand`'s stream is not a stable format across versions and was never
//!   promised to be; ours has to be identical on every machine for as long as a
//!   recorded score means anything.
//! * **The key is hashed into the generator's state through one documented
//!   string.** Change the format of that string and every artifact ever derived
//!   changes with it.
//! * **Slots roll in declaration order**, and groups are permuted before any of
//!   them, in name order. Order is part of the output, so it is fixed here
//!   rather than left to whatever the caller iterates.
//!
//! None of that constrains a manifest author, and all of it constrains us. A
//! change to any of it is a `generator_version` bump for every challenge in the
//! library, which is a thing to do deliberately or not at all.

use std::collections::BTreeMap;

use crate::console::{Console, Fixup};
use crate::hash::sha1_hex;
use crate::slots::{PatchSlot, PatchTable, ValueSource};

/// ADR 0013's derivation key, in full. Two players who agree on all four agree
/// on the artifact, and `asset-store` caches under it.
///
/// `base_rom_sha1` is the hash of the content the slot offsets are counted into
/// — which for a challenge on a hack is the *hack's* hash, not the base game's.
/// ADR 0020 says why that is a widening of the first element rather than a
/// change to it: the key names a hash, not "the manifest's ROM".
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DerivationKey {
    pub base_rom_sha1: String,
    pub generator_id: String,
    pub generator_version: u32,
    pub seed: u64,
}

impl DerivationKey {
    /// The exact bytes the generator is seeded from. Written down as one
    /// function so that "what does the key hash to" has a single answer that can
    /// be pointed at in a review.
    pub fn canonical(&self) -> String {
        format!(
            "content-gen/1\n{}\n{}\n{}\n{}\n",
            self.base_rom_sha1.to_lowercase(),
            self.generator_id,
            self.generator_version,
            self.seed
        )
    }

    /// A cache path component, for `asset-store` and for the local cache that
    /// stands in for it today. Short, and collision-resistant enough that two
    /// keys sharing one would be a bigger problem than a cache miss.
    pub fn digest(&self) -> String {
        sha1_hex(self.canonical().as_bytes())
    }

    fn stream(&self) -> Rng {
        Rng::from_digest(&self.digest())
    }
}

/// splitmix64. Chosen because it is eleven lines, has no state to get wrong, and
/// can be re-implemented from this file by anyone who needs to check our output.
struct Rng(u64);

impl Rng {
    fn from_digest(hex: &str) -> Rng {
        let mut state = 0u64;
        for b in hex.as_bytes().iter().take(16) {
            state = (state << 4) | u64::from((*b as char).to_digit(16).unwrap_or(0));
        }
        Rng(state)
    }

    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform over `0..n`, by rejection rather than by modulo. The bias a plain
    /// `% n` introduces is small, and it is also invisible: a pool of three
    /// would be very slightly skewed forever and nothing would ever say so.
    fn below(&mut self, n: u64) -> u64 {
        debug_assert!(n > 0);
        let limit = u64::MAX - (u64::MAX % n);
        loop {
            let x = self.next();
            if x < limit {
                return x % n;
            }
        }
    }
}

/// One slot's decided value, before anything is written.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Rolled {
    pub offset: usize,
    pub value: u32,
    /// Which slot of the table this is, for a message that can be matched back
    /// to the manifest.
    pub slot: usize,
}

/// Every slot of one table, decided. Separated from applying it so that a roll
/// can be printed, compared or golden-tested without a ROM in hand — and so that
/// a *chosen* roll can be applied, which is how the hashes in
/// `docs/games/smb3.md` are reproduced.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Roll {
    pub key: DerivationKey,
    pub values: Vec<Rolled>,
}

/// A derived ROM, and everything worth saying about how it got that way.
#[derive(Clone, Debug)]
pub struct Artifact {
    pub bytes: Vec<u8>,
    /// The identity ADR 0013 compares across players.
    pub sha1: String,
    pub key: DerivationKey,
    pub writes: Vec<Write>,
    /// What the console's checksum rule did, if it has one.
    pub fixup: Option<Fixup>,
}

/// One byte-range this derivation changed, and what was there before. The
/// `before` is worth carrying: a slot rolling back to the value it already held
/// is a normal outcome, and a log that cannot show it looks like a bug.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Write {
    pub offset: usize,
    pub before: u32,
    pub after: u32,
}

impl Write {
    pub fn changed(&self) -> bool {
        self.before != self.after
    }
}

impl PatchTable {
    /// Decides a value for every slot, deterministically from the key.
    pub fn roll(&self, key: &DerivationKey) -> Roll {
        let mut rng = key.stream();

        // Groups first, in name order, so that adding an independent slot to a
        // table does not reshuffle a group that has nothing to do with it.
        // `groups` is a BTreeMap, so "name order" needs no sorting here — it is
        // the order it iterates in, and that is why it is a BTreeMap.
        let mut drawn: BTreeMap<&str, Vec<u32>> = BTreeMap::new();
        for (name, group) in &self.groups {
            let mut pool = group.pool.clone();
            // Fisher-Yates, downward, taking the first `members` of the result.
            for i in (1..pool.len()).rev() {
                let j = rng.below(i as u64 + 1) as usize;
                pool.swap(i, j);
            }
            pool.truncate(group.members);
            drawn.insert(name.as_str(), pool);
        }

        let mut values = Vec::with_capacity(self.slots.len());
        for (i, slot) in self.slots.iter().enumerate() {
            let value = match &slot.values {
                ValueSource::Pool(pool) => pool[rng.below(pool.len() as u64) as usize],
                ValueSource::Range { min, max } => {
                    let span = u64::from(*max - *min) + 1;
                    *min + rng.below(span) as u32
                }
                ValueSource::Group(name) => drawn
                    .get_mut(name.as_str())
                    .and_then(|left| left.pop())
                    .expect("PatchTable::build sized every group against its members"),
            };
            values.push(Rolled { offset: slot.offset, value, slot: i });
        }

        Roll { key: key.clone(), values }
    }

    /// Writes a decided roll into an otherwise-untouched copy of `rom`.
    ///
    /// Everything this does not do is the decision (ADR 0015): no slot is
    /// created, nothing is relocated, nothing is compressed or recomputed except
    /// the console's own checksum, and the file's length never changes. ADR 0019
    /// turned that from a scoping preference into a correctness requirement —
    /// rebinding an entry state onto a derived ROM is only sound because the
    /// derived ROM is the same length, mapper and geometry as the one the state
    /// was captured on.
    pub fn apply(
        &self,
        rom: &[u8],
        console: Console,
        roll: &Roll,
    ) -> Result<Artifact, String> {
        console.is_derivable()?;
        let header = console.header_bytes(rom)?;
        let reach = header + self.reach();
        if reach > rom.len() {
            return Err(format!(
                "this table writes up to {reach:#X} and the ROM is {:#X} bytes: either the \
                 offsets are into a different ROM, or they were written counting the \
                 {header}-byte header that they should not count",
                rom.len()
            ));
        }

        let mut out = rom.to_vec();
        let mut writes = Vec::with_capacity(roll.values.len());
        for rolled in &roll.values {
            let slot: &PatchSlot = self
                .slots
                .get(rolled.slot)
                .filter(|s| s.offset == rolled.offset)
                .ok_or_else(|| {
                    format!(
                        "this roll was made against a different patch table: it decides slot \
                         {} at offset {:#X}, which this table does not have there",
                        rolled.slot, rolled.offset
                    )
                })?;
            let at = header + slot.offset;
            let span = at..at + slot.width.bytes();
            let before = slot.endian.read(slot.width, &out[span.clone()]);
            slot.endian.write(rolled.value, slot.width, &mut out[span]);
            writes.push(Write { offset: slot.offset, before, after: rolled.value });
        }

        let fixup = console.fixup(rom, &mut out)?;
        let sha1 = sha1_hex(&out);
        Ok(Artifact { bytes: out, sha1, key: roll.key.clone(), writes, fixup })
    }
}

/// The whole of what stage prep asks for: a ROM, its table, a seed, an artifact.
///
/// The key is built here rather than passed in, so that the hash in it is
/// always the hash of the ROM the slots were actually written into. A key whose
/// `base_rom_sha1` names something other than the bytes it was applied to is the
/// one way this could produce two different artifacts under one identity, and
/// the type system cannot catch it — so the API does not offer the chance.
pub fn derive(
    rom: &[u8],
    console: Console,
    table: &PatchTable,
    seed: u64,
) -> Result<Artifact, String> {
    let key = DerivationKey {
        base_rom_sha1: sha1_hex(rom),
        generator_id: table.generator_id.clone(),
        generator_version: table.generator_version,
        seed,
    };
    let roll = table.roll(&key);
    table.apply(rom, console, &roll)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slots::{GroupDoc, PatchSlotDoc, ValuesDoc};
    use std::collections::BTreeMap;

    fn table(slots: &[PatchSlotDoc], groups: &[(&str, GroupDoc)]) -> PatchTable {
        let g: BTreeMap<String, GroupDoc> =
            groups.iter().map(|(n, d)| ((*n).to_string(), d.clone())).collect();
        PatchTable::build("a-challenge", 1, slots, &g).unwrap()
    }

    fn slot(offset: i64, values: ValuesDoc) -> PatchSlotDoc {
        PatchSlotDoc { offset, width: "u8".into(), endian: None, values, note: None }
    }

    fn pool(v: &[i64]) -> ValuesDoc {
        ValuesDoc { pool: Some(v.to_vec()), range: None, group: None }
    }

    fn nes(len: usize) -> Vec<u8> {
        let mut rom = b"NES\x1a".to_vec();
        rom.resize(16 + len, 0);
        rom
    }

    fn key(seed: u64) -> DerivationKey {
        DerivationKey {
            base_rom_sha1: "6bd518e85eb46a4252af07910f61036e84b020d1".into(),
            generator_id: "a-challenge".into(),
            generator_version: 1,
            seed,
        }
    }

    /// The property ADR 0013 aborts a session over. If this ever fails, every
    /// recorded score in the library is against content nobody can reproduce.
    #[test]
    fn the_same_key_rolls_the_same_values_every_time() {
        let t = table(&[slot(0x10, pool(&[1, 2, 3, 4, 5, 6, 7, 8]))], &[]);
        let a = t.roll(&key(9));
        let b = t.roll(&key(9));
        assert_eq!(a, b);
        assert_eq!(t.apply(&nes(64), Console::Nes, &a).unwrap().sha1,
                   t.apply(&nes(64), Console::Nes, &b).unwrap().sha1);
    }

    #[test]
    fn different_seeds_roll_differently() {
        let t = table(&[slot(0x10, ValuesDoc {
            pool: None,
            range: Some(crate::slots::RangeDoc { min: 0, max: 255 }),
            group: None,
        })], &[]);
        let seen: std::collections::BTreeSet<u32> =
            (0..40).map(|s| t.roll(&key(s)).values[0].value).collect();
        assert!(seen.len() > 20, "40 seeds produced only {} values", seen.len());
    }

    /// Everything else about the key is part of the key too: two challenges
    /// sharing a table and a seed must not share an artifact, or one challenge's
    /// roll would be known from having played the other.
    #[test]
    fn the_generator_id_and_version_are_part_of_the_key() {
        let t = table(&[slot(0x10, pool(&(0..64).collect::<Vec<_>>()))], &[]);
        let mut other = key(1);
        other.generator_id = "another-challenge".into();
        let mut bumped = key(1);
        bumped.generator_version = 2;
        let a = t.roll(&key(1)).values[0].value;
        assert_ne!(a, t.roll(&other).values[0].value);
        assert_ne!(a, t.roll(&bumped).values[0].value);
    }

    /// A pool's duplicates are weights all the way through the engine, not only
    /// through the validator: four of eight entries being `1` has to come out
    /// near half the time. Bro Roulette is exactly this shape.
    #[test]
    fn a_pools_duplicates_weight_the_roll() {
        let t = table(&[slot(0x10, pool(&[1, 1, 1, 1, 2, 3, 4, 5]))], &[]);
        let ones = (0..400).filter(|s| t.roll(&key(*s)).values[0].value == 1).count();
        assert!((150..250).contains(&ones), "{ones} of 400 rolled the weighted value");
    }

    #[test]
    fn a_group_is_drawn_without_replacement() {
        let t = table(
            &[
                slot(0x10, ValuesDoc { pool: None, range: None, group: Some("g".into()) }),
                slot(0x11, ValuesDoc { pool: None, range: None, group: Some("g".into()) }),
                slot(0x12, ValuesDoc { pool: None, range: None, group: Some("g".into()) }),
            ],
            &[("g", GroupDoc { pool: vec![1, 2, 3], mode: "permutation".into() })],
        );
        for seed in 0..50 {
            let r = t.roll(&key(seed));
            let mut got: Vec<u32> = r.values.iter().map(|v| v.value).collect();
            got.sort_unstable();
            assert_eq!(got, vec![1, 2, 3], "seed {seed} repeated a group member");
        }
        // ...and it is actually shuffled, not handed out in pool order.
        let orders: std::collections::BTreeSet<Vec<u32>> = (0..50)
            .map(|s| t.roll(&key(s)).values.iter().map(|v| v.value).collect())
            .collect();
        assert!(orders.len() > 1, "the permutation never permuted anything");
    }

    #[test]
    fn a_range_covers_both_of_its_ends() {
        let t = table(&[slot(0x10, ValuesDoc {
            pool: None,
            range: Some(crate::slots::RangeDoc { min: 3, max: 5 }),
            group: None,
        })], &[]);
        let seen: std::collections::BTreeSet<u32> =
            (0..60).map(|s| t.roll(&key(s)).values[0].value).collect();
        assert_eq!(seen, [3, 4, 5].into_iter().collect());
    }

    #[test]
    fn a_write_records_what_was_there_before() {
        let t = table(&[slot(0x10, pool(&[0x99]))], &[]);
        let mut rom = nes(64);
        rom[16 + 0x10] = 0x11;
        let art = t.apply(&rom, Console::Nes, &t.roll(&key(1))).unwrap();
        assert_eq!(art.writes[0], Write { offset: 0x10, before: 0x11, after: 0x99 });
        assert!(art.writes[0].changed());
        // The offset is counted after the header, and nothing else moved.
        assert_eq!(art.bytes[16 + 0x10], 0x99);
        assert_eq!(art.bytes.len(), rom.len());
        assert_eq!(art.bytes[..16 + 0x10], rom[..16 + 0x10]);
        assert_eq!(art.bytes[16 + 0x11..], rom[16 + 0x11..]);
    }

    /// A table whose offsets are past the end of the ROM is the "wrong ROM"
    /// mistake, and the message says so rather than panicking on a slice.
    #[test]
    fn a_table_that_reaches_past_the_rom_is_refused() {
        let t = table(&[slot(0x400, pool(&[1]))], &[]);
        let e = t.apply(&nes(64), Console::Nes, &t.roll(&key(1))).unwrap_err();
        assert!(e.contains("different ROM"), "{e}");
    }

    #[test]
    fn a_dos_bundle_is_refused_before_a_byte_is_written() {
        let t = table(&[slot(0x10, pool(&[1]))], &[]);
        let e = t.apply(b"PK\x03\x04", Console::Dos, &t.roll(&key(1))).unwrap_err();
        assert!(e.contains("member-level overlay"), "{e}");
    }

    /// The one-call form is what stage prep uses, and it hashes the ROM it was
    /// handed rather than trusting a caller's idea of what that ROM is.
    #[test]
    fn derive_keys_on_the_rom_it_was_given() {
        let t = table(&[slot(0x10, pool(&[1, 2, 3, 4]))], &[]);
        let art = derive(&nes(64), Console::Nes, &t, 7).unwrap();
        assert_eq!(art.key.base_rom_sha1, crate::hash::sha1_hex(&nes(64)));
        assert_eq!(art.key.generator_id, "a-challenge");
        assert_eq!(art.key.seed, 7);
        assert_eq!(art.sha1, crate::hash::sha1_hex(&art.bytes));
    }

    /// The digest is what a cache is keyed on, so two keys that differ anywhere
    /// have to differ in it.
    #[test]
    fn the_digest_follows_every_field_of_the_key() {
        let a = key(1);
        let mut b = key(2);
        assert_ne!(a.digest(), b.digest());
        b.seed = 1;
        assert_eq!(a.digest(), b.digest());
        assert!(a.canonical().contains("6bd518e8"));
    }
}
