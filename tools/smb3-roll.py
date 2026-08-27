#!/usr/bin/env python3
"""Apply one SMB3 custom-challenge roll to the base ROM.

Scaffolding, not the engine. `content-gen` (ADR 0013/0015) is the seeded patch
engine and it will replace this; what this exists for is producing a patched ROM
today, so entry states can be captured and checked against one before the crate
is written. The slot tables it carries are the ones in `docs/games/smb3.md`, and
the two named rolls reproduce the golden hashes recorded there.

    python tools/smb3-roll.py --roll golden --out build/smb3-golden.nes
    python tools/smb3-roll.py --seed 7 --out build/smb3-seed7.nes

Offsets are into the ROM after the 16-byte iNES header, per the manifest schema.
Nothing here is a derivation key: `--seed` is Python's RNG, not ADR 0013's
`(base_rom_hash, generator_id, generator_version, seed)`, and two rolls from the
same seed are only reproducible on the same Python.
"""

import argparse
import hashlib
import pathlib
import random
import sys

BASE_ROM = "assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes"
INES_HEADER = 0x10

# Sun Storm - World 2 Sand Trap. PRG/objects/SandTrap.asm.
# Slot A's single-value pool is what makes the roll 1-3 suns rather than 0-3.
SUN_STORM = {
    0xC856: [0xAF],          # slot A id      - always a sun
    0xC857: [0x5B],          # slot A column
    0xC859: [0xAF, 0x6E],    # slot B id      - sun, or the original paratroopa
    0xC85A: [0x5C],          # slot B column
    0xC85C: [0xAF, 0x6E],    # slot C id
    0xC85D: [0x5D],          # slot C column
}

# Bro Roulette - World 8 Hammer Bro hand trap. PRG/objects/W8HTrap1.asm.
# The four ids are a permutation, not four independent draws: the pool is the
# four variants Nintendo already put in this level.
BRO_SLOTS = [0xD0B1, 0xD0B4, 0xD0B7, 0xD0BA]
BRO_POOL = [0x81, 0x82, 0x86, 0x87]
BRO_PARK = {0xD0BE: [0x7F]}  # column 0x7F is past the level's four screens

# The two rolls `docs/games/smb3.md` records a hash for.
NAMED_ROLLS = {
    "golden": {
        0xC856: 0xAF, 0xC857: 0x5B, 0xC859: 0xAF, 0xC85A: 0x5C,
        0xC85C: 0xAF, 0xC85D: 0x5D,
        0xD0B1: 0x86, 0xD0B4: 0x87, 0xD0B7: 0x82, 0xD0BA: 0x81,
        0xD0BE: 0x7F,
    },
    "b": {
        0xC856: 0xAF, 0xC857: 0x5B, 0xC859: 0x6E, 0xC85A: 0x5C,
        0xC85C: 0xAF, 0xC85D: 0x5D,
        0xD0B1: 0x81, 0xD0B4: 0x82, 0xD0B7: 0x86, 0xD0BA: 0x87,
        0xD0BE: 0x7F,
    },
}
NAMED_HASHES = {
    "golden": "b60ba24bca183b8c707d01b3f7b835ec7e38614d",
    "b": "bf8e21ff2e4e5958f7f7391f40187534c54bb671",
}


def roll_from_seed(seed):
    rng = random.Random(seed)
    patch = {off: rng.choice(pool) for off, pool in SUN_STORM.items()}
    patch.update(dict(zip(BRO_SLOTS, rng.sample(BRO_POOL, len(BRO_POOL)))))
    patch.update({off: pool[0] for off, pool in BRO_PARK.items()})
    return patch


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--rom", default=BASE_ROM, help=f"base ROM. Default: {BASE_ROM}")
    ap.add_argument("--roll", choices=sorted(NAMED_ROLLS), help="a roll with a recorded hash")
    ap.add_argument("--seed", type=int, help="roll from the slot tables with this seed")
    ap.add_argument("--out", required=True, help="where to write the patched ROM")
    args = ap.parse_args()

    if (args.roll is None) == (args.seed is None):
        ap.error("pass exactly one of --roll or --seed")

    rom = bytearray(pathlib.Path(args.rom).read_bytes())
    base_sha1 = hashlib.sha1(rom).hexdigest()
    print(f"base    {base_sha1}  {args.rom}")

    patch = NAMED_ROLLS[args.roll] if args.roll else roll_from_seed(args.seed)
    for off, val in sorted(patch.items()):
        was = rom[INES_HEADER + off]
        rom[INES_HEADER + off] = val
        print(f"  0x{off:04X}  {was:02X} -> {val:02X}" + ("  (unchanged)" if was == val else ""))

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_bytes(rom)
    sha1 = hashlib.sha1(rom).hexdigest()
    print(f"patched {sha1}  {out}")

    expected = NAMED_HASHES.get(args.roll)
    if expected and sha1 != expected:
        print(f"MISMATCH: docs/games/smb3.md records {expected}", file=sys.stderr)
        return 1
    if expected:
        print("matches the hash in docs/games/smb3.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
