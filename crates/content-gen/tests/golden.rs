//! Golden-hash tests against the real ROMs.
//!
//! ADR 0013 said generators "cannot be integration-tested with `session-fake`;
//! they need a real base ROM, so their tests are golden-hash tests run
//! manually". These are those tests, and they are only half manual: the engine
//! has no emulator dependency, so a checkout that *has* the ROMs runs them as
//! ordinary `cargo test`, and one that does not skips them out loud.
//!
//! The hashes come from `docs/games/smb3.md`, which recorded them against
//! `tools/smb3-roll.py` before this crate existed. That is the point of them —
//! they were produced by a different implementation, in a different language,
//! from a table a human wrote down, so agreeing with them is evidence rather
//! than a tautology. The two SMB3 slot tables below are transcribed from the
//! same document.
//!
//! No ROM is in the repository and none should be. What is here is a player's
//! own dump and a contributor's patch; everything else these tests touch is
//! derived in memory and never written down.

use std::collections::BTreeMap;
use std::path::Path;

use content_gen::{
    bps, derive, roll::Rolled, Console, DerivationKey, GroupDoc, PatchSlotDoc, PatchTable, Roll,
    ValuesDoc,
};

const BASE: &str = "../../assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes";
const PATCH: &str = "../../assets/patches/smb3-rom-hack1.bps";
const DKC: &str = "../../assets/ROMs/snes/Donkey Kong Country (U) (V1.0) [!].smc";

const BASE_SHA1: &str = "6bd518e85eb46a4252af07910f61036e84b020d1";
const HACK_SHA1: &str = "11f4851c7b4e8413432eed30b0a3760e5dcac638";

/// Reads an asset, or gives up on this test. A checkout without ROMs is the
/// normal case for anyone who is not the person holding the dumps.
fn asset(path: &str) -> Option<Vec<u8>> {
    match std::fs::read(Path::new(path)) {
        Ok(b) => Some(b),
        Err(_) => {
            println!("skipped: {path} is not in this checkout");
            None
        }
    }
}

fn slot(offset: i64, pool: &[i64]) -> PatchSlotDoc {
    PatchSlotDoc {
        offset,
        width: "u8".to_string(),
        endian: None,
        values: ValuesDoc { pool: Some(pool.to_vec()), range: None, group: None },
        note: None,
    }
}

/// Sun Storm, `docs/games/smb3.md`: three Angry Sun slots in the object list at
/// `$C6BC`, the last of them fixed so the roll is one to three suns rather than
/// zero to three. `$BB` is `OBJ_CANCELEVENT` — the closest thing SMB3 has to an
/// empty list entry under pure value substitution.
fn sun_storm() -> PatchTable {
    PatchTable::build(
        "sun-storm-survival",
        1,
        &[
            slot(0xC6BD, &[0xAF, 0xBB]),
            slot(0xC6C0, &[0xAF, 0xBB]),
            slot(0xC6C3, &[0xAF]),
        ],
        &BTreeMap::new(),
    )
    .unwrap()
}

/// Bro Roulette, `docs/games/smb3.md`: four goomba slots in the object list at
/// `$C6CA`, each rolled independently over a *multiset* — four goombas against
/// one of each bro, so a coin flip decides whether the goomba is replaced.
fn bro_roulette() -> PatchTable {
    const POOL: [i64; 8] = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87];
    PatchTable::build(
        "bro-roulette-time-trial",
        1,
        &[
            slot(0xC6CB, &POOL),
            slot(0xC6CE, &POOL),
            slot(0xC6D1, &POOL),
            slot(0xC6D4, &POOL),
        ],
        &BTreeMap::new(),
    )
    .unwrap()
}

/// Both tables, as one generator, which is how the recorded hashes were taken:
/// the seven rollable bytes written into one artifact.
fn seven_slots() -> PatchTable {
    let mut slots: Vec<PatchSlotDoc> = Vec::new();
    slots.extend(
        [0xC6BD, 0xC6C0].iter().map(|o| slot(*o, &[0xAF, 0xBB])),
    );
    slots.push(slot(0xC6C3, &[0xAF]));
    const POOL: [i64; 8] = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87];
    slots.extend([0xC6CB, 0xC6CE, 0xC6D1, 0xC6D4].iter().map(|o| slot(*o, &POOL)));
    PatchTable::build("smb3-golden", 1, &slots, &BTreeMap::new()).unwrap()
}

/// A roll with every value chosen, rather than drawn. This is what makes a
/// recorded hash testable at all: the engine's *choosing* is seeded and its
/// *writing* is not, and it is the writing — offsets, header, widths, checksum —
/// that a golden hash pins down.
fn chosen(table: &PatchTable, values: &[u32]) -> Roll {
    assert_eq!(table.slots.len(), values.len());
    Roll {
        key: DerivationKey {
            base_rom_sha1: HACK_SHA1.to_string(),
            generator_id: table.generator_id.clone(),
            generator_version: table.generator_version,
            seed: 0,
        },
        values: values
            .iter()
            .enumerate()
            .map(|(i, v)| Rolled { offset: table.slots[i].offset, value: *v, slot: i })
            .collect(),
    }
}

fn hack() -> Option<Vec<u8>> {
    let base = asset(BASE)?;
    assert_eq!(content_gen::sha1_hex(&base), BASE_SHA1, "the base ROM is not the one docs/games/smb3.md records");
    let patched = bps::apply(&base, &asset(PATCH)?).expect("the hack applies to the base ROM");
    Some(patched.bytes)
}

/// ADR 0020's whole mechanism, end to end: a player's own dump plus a
/// contributor's patch is the hack, byte for byte.
#[test]
fn the_bps_patch_derives_the_hack_docs_record() {
    let Some(base) = asset(BASE) else { return };
    let Some(patch) = asset(PATCH) else { return };
    let out = bps::apply(&base, &patch).unwrap();
    assert_eq!(content_gen::sha1_hex(&out.bytes), HACK_SHA1);
    assert_eq!(out.bytes.len(), base.len());
    // The editor the hack came out of, carried in the patch's metadata.
    assert!(out.metadata.contains("Bank12"), "{}", out.metadata);
}

/// BPS refuses the wrong dump rather than producing a plausible wrong ROM,
/// which is the reason ADR 0020 chose it over IPS. One flipped byte is enough.
#[test]
fn the_patch_refuses_a_rom_that_is_not_the_base() {
    let Some(mut base) = asset(BASE) else { return };
    let Some(patch) = asset(PATCH) else { return };
    base[0x2000] ^= 0x01;
    let err = bps::apply(&base, &patch).unwrap_err();
    assert!(err.contains("a different ROM"), "{err}");
}

/// The three rolls `docs/games/smb3.md` records a hash for, reproduced by this
/// engine rather than by the Python scaffolding that first produced them.
#[test]
fn the_recorded_rolls_reproduce_their_recorded_hashes() {
    let Some(hack) = hack() else { return };
    let table = seven_slots();
    for (name, values, sha1) in [
        (
            "golden",
            [0xBB, 0xAF, 0xAF, 0x81, 0x87, 0x72, 0x86],
            "33d3ebc2183122237e9105e0e39f0c40ce902002",
        ),
        (
            "b",
            [0xBB, 0xBB, 0xAF, 0x72, 0x72, 0x82, 0x81],
            "3153bf8dedcd541de3d802fc475638559a6b1d8e",
        ),
        (
            "max",
            [0xAF, 0xAF, 0xAF, 0x87, 0x86, 0x82, 0x81],
            "c7172e2cfb051f5411bb053f222dc3de57cbfe11",
        ),
    ] {
        let art = table.apply(&hack, Console::Nes, &chosen(&table, &values)).unwrap();
        assert_eq!(art.sha1, sha1, "roll `{name}`");
        // Same length, same everything but seven bytes: ADR 0019 depends on it.
        assert_eq!(art.bytes.len(), hack.len());
        assert_eq!(art.writes.len(), 7);
        assert!(art.fixup.is_none(), "the NES has no checksum to fix");
    }
}

/// A seeded roll on the hack: every value is one the manifest declared, the
/// artifact is reproducible, and different seeds reach different content.
#[test]
fn seeded_rolls_on_the_hack_stay_inside_their_pools() {
    let Some(hack) = hack() else { return };
    let sun = sun_storm();
    let bro = bro_roulette();

    let mut hashes = std::collections::BTreeSet::new();
    for seed in 0..50u64 {
        let a = derive(&hack, Console::Nes, &sun, seed).unwrap();
        let b = derive(&hack, Console::Nes, &bro, seed).unwrap();
        assert_eq!(a.sha1, derive(&hack, Console::Nes, &sun, seed).unwrap().sha1);
        assert_eq!(a.key.base_rom_sha1, HACK_SHA1, "the key names the ROM the slots are in");

        for (art, legal) in [
            (&a, vec![0xAF, 0xBB]),
            (&b, vec![0x72, 0x81, 0x82, 0x86, 0x87]),
        ] {
            for w in &art.writes {
                assert!(legal.contains(&w.after), "{:#X} at {:#X}", w.after, w.offset);
            }
        }
        // Slot C is fixed: every Sun Storm roll leaves a sun at column $05.
        assert_eq!(a.writes[2].after, 0xAF);
        hashes.insert(b.sha1.clone());
    }
    assert!(hashes.len() > 10, "50 seeds produced {} artifacts", hashes.len());
}

/// The SNES half of the console table, against a real ROM. DKC is 4 MB, HiROM,
/// with no copier header, and its stored checksum is the sum of its own bytes —
/// so an untouched ROM must come back with the same checksum it went in with,
/// found at `$FFC0` without being told where to look.
#[test]
fn the_snes_checksum_rule_agrees_with_a_real_rom() {
    let Some(rom) = asset(DKC) else { return };
    let mut copy = rom.clone();
    let fix = Console::Snes.fixup(&rom, &mut copy).unwrap().expect("the SNES has a checksum");
    assert_eq!(fix.header_at, 0xFFC0, "DKC is HiROM");
    assert_eq!(fix.was, fix.now, "an untouched ROM's checksum should not move");
    assert_eq!(copy, rom, "and nothing at all should have been written");
    assert_eq!(fix.now, 0xEF80, "the checksum in DKC's own header");
}

/// And a substitution moves it by exactly the delta, leaving a ROM whose header
/// still checks out — which is what a core and a copier both verify.
#[test]
fn a_substitution_updates_a_real_snes_checksum() {
    let Some(rom) = asset(DKC) else { return };
    let table = PatchTable::build(
        "a-dkc-challenge",
        1,
        &[slot(0x1000, &[0x01])],
        &BTreeMap::new(),
    )
    .unwrap();
    let before = rom[0x1000];
    let art = table.apply(&rom, Console::Snes, &chosen(&table, &[0x01])).unwrap();
    let fix = art.fixup.expect("the SNES has a checksum");
    assert_eq!(fix.now, fix.was.wrapping_sub(u16::from(before)).wrapping_add(1));
    assert_eq!(art.bytes.len(), rom.len());

    // Re-running the fixup on the result is a no-op, which is the property that
    // says the artifact is internally consistent.
    let mut again = art.bytes.clone();
    let second = Console::Snes.fixup(&art.bytes, &mut again).unwrap().unwrap();
    assert_eq!(second.was, second.now);
}

/// A group is what "shuffle which enemy stands in which existing slot" is
/// spelled as, and on a real ROM it has to stay a permutation of the pool.
#[test]
fn a_permutation_group_writes_each_value_once() {
    let Some(hack) = hack() else { return };
    let mut slots = Vec::new();
    for offset in [0xC6CB, 0xC6CE, 0xC6D1, 0xC6D4] {
        slots.push(PatchSlotDoc {
            offset,
            width: "u8".to_string(),
            endian: None,
            values: ValuesDoc { pool: None, range: None, group: Some("bros".to_string()) },
            note: None,
        });
    }
    let groups: BTreeMap<String, GroupDoc> = [(
        "bros".to_string(),
        GroupDoc { pool: vec![0x81, 0x82, 0x86, 0x87], mode: "permutation".to_string() },
    )]
    .into_iter()
    .collect();
    let table = PatchTable::build("one-of-each-bro", 1, &slots, &groups).unwrap();

    for seed in 0..25u64 {
        let art = derive(&hack, Console::Nes, &table, seed).unwrap();
        let mut got: Vec<u32> = art.writes.iter().map(|w| w.after).collect();
        got.sort_unstable();
        assert_eq!(got, vec![0x81, 0x82, 0x86, 0x87], "seed {seed}");
    }
}
