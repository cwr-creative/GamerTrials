#!/usr/bin/env python3
"""Apply one SMB3 custom-challenge roll to the ROM a challenge runs on.

Scaffolding, not the engine. `content-gen` (ADR 0013/0015) is the seeded patch
engine and it will replace this; what this exists for is producing a patched ROM
today, so entry states can be captured and checked against one before the crate
is written. The slot tables it carries are the ones in `docs/games/smb3.md`, and
the two named rolls reproduce the golden hashes recorded there.

Both custom challenges live on a hand-authored hack, not on the base ROM
(ADR 0020), so the roll's input is the *derived* ROM:

    python tools/smb3-roll.py --apply-patch --out build/rom-hack1.nes
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
import zlib

BASE_ROM = "assets/ROMs/Super Mario Bros. 3 (USA) (Rev 1).nes"
HACK_PATCH = "assets/patches/smb3-rom-hack1.bps"
HACK_ROM = "build/rom-hack1.nes"
INES_HEADER = 0x10

BASE_SHA1 = "6bd518e85eb46a4252af07910f61036e84b020d1"
HACK_SHA1 = "11f4851c7b4e8413432eed30b0a3760e5dcac638"

# Sun Storm - W2_32 on the hack, entered from the World 1 map. Object list at
# $C6BC. Three Angry Sun slots; the last one is fixed so the roll is 1-3 suns
# rather than 0-3. $BB is OBJ_CANCELEVENT, which spawns nothing and consumes no
# object slot - the closest thing SMB3 has to an empty entry under pure value
# substitution.
SUN_SLOT_FIXED = {
    0xC6C3: [0xAF],                # column $05 - always a sun, and the first to activate
}
SUN_SLOTS = {
    0xC6BD: [0xAF, 0xBB],          # column $01
    0xC6C0: [0xAF, 0xBB],          # column $03
}

# Bro Roulette - W1_03 on the hack. Object list at $C6CA. Four goomba slots,
# each rolled independently over a *multiset*: four goombas against one of each
# bro, so a coin flip decides whether the goomba is replaced and the four bro
# variants share the other half evenly.
BRO_SLOTS = [0xC6CB, 0xC6CE, 0xC6D1, 0xC6D4]
BRO_POOL = [0x72, 0x72, 0x72, 0x72, 0x81, 0x82, 0x86, 0x87]

# The rolls `docs/games/smb3.md` records a hash for.
NAMED_ROLLS = {
    "golden": {
        0xC6BD: 0xBB, 0xC6C0: 0xAF, 0xC6C3: 0xAF,
        0xC6CB: 0x81, 0xC6CE: 0x87, 0xC6D1: 0x72, 0xC6D4: 0x86,
    },
    "b": {
        0xC6BD: 0xBB, 0xC6C0: 0xBB, 0xC6C3: 0xAF,
        0xC6CB: 0x72, 0xC6CE: 0x72, 0xC6D1: 0x82, 0xC6D4: 0x81,
    },
    "max": {
        0xC6BD: 0xAF, 0xC6C0: 0xAF, 0xC6C3: 0xAF,
        0xC6CB: 0x87, 0xC6CE: 0x86, 0xC6D1: 0x82, 0xC6D4: 0x81,
    },
}
NAMED_HASHES = {
    "golden": "33d3ebc2183122237e9105e0e39f0c40ce902002",
    "b": "3153bf8dedcd541de3d802fc475638559a6b1d8e",
    "max": "c7172e2cfb051f5411bb053f222dc3de57cbfe11",
}

OBJ_NAMES = {
    0x72: "OBJ_GOOMBA", 0x81: "OBJ_HAMMERBRO", 0x82: "OBJ_BOOMERANGBRO",
    0x86: "OBJ_HEAVYBRO", 0x87: "OBJ_FIREBRO", 0xAF: "OBJ_ENEMYSUN",
    0xBB: "OBJ_CANCELEVENT",
}


# ------------------------------------------------------------------- BPS apply
#
# The hack ships as a patch, never as a ROM (ADR 0020). This is the same
# transform `content-gen` owes; it lives here so there is one command that gets
# from a player's own dump to something runnable.

def _vuint(buf, pos):
    val, shift = 0, 1
    while True:
        b = buf[pos]
        pos += 1
        val += (b & 0x7F) * shift
        if b & 0x80:
            return val, pos
        shift <<= 7
        val += shift


def apply_bps(src, patch):
    if patch[:4] != b"BPS1":
        raise ValueError("not a BPS patch")
    pos = 4
    src_size, pos = _vuint(patch, pos)
    dst_size, pos = _vuint(patch, pos)
    meta_size, pos = _vuint(patch, pos)
    pos += meta_size
    if len(src) != src_size:
        raise ValueError(f"source is {len(src)} bytes, patch expects {src_size}")
    if zlib.crc32(src) != int.from_bytes(patch[-12:-8], "little"):
        raise ValueError("source CRC32 mismatch: wrong ROM revision")
    dst = bytearray(dst_size)
    out = src_rel = dst_rel = 0
    end = len(patch) - 12
    while pos < end:
        data, pos = _vuint(patch, pos)
        action, length = data & 3, (data >> 2) + 1
        if action == 0:
            dst[out:out + length] = src[out:out + length]
            out += length
        elif action == 1:
            dst[out:out + length] = patch[pos:pos + length]
            pos += length
            out += length
        else:
            d, pos = _vuint(patch, pos)
            delta = (-1 if d & 1 else 1) * (d >> 1)
            if action == 2:
                src_rel += delta
                dst[out:out + length] = src[src_rel:src_rel + length]
                src_rel += length
                out += length
            else:
                dst_rel += delta
                for _ in range(length):
                    dst[out] = dst[dst_rel]
                    dst_rel += 1
                    out += 1
    if zlib.crc32(bytes(dst)) != int.from_bytes(patch[-8:-4], "little"):
        raise ValueError("target CRC32 mismatch: the patch did not apply cleanly")
    return bytes(dst)


# ----------------------------------------------------------------------- rolls

def roll_from_seed(seed):
    rng = random.Random(seed)
    patch = {off: pool[0] for off, pool in SUN_SLOT_FIXED.items()}
    patch.update({off: rng.choice(pool) for off, pool in SUN_SLOTS.items()})
    patch.update({off: rng.choice(BRO_POOL) for off in BRO_SLOTS})
    return patch


def main():
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--rom", help=f"ROM to roll. Default: {HACK_ROM}")
    ap.add_argument("--base", default=BASE_ROM, help=f"base ROM. Default: {BASE_ROM}")
    ap.add_argument("--patch", default=HACK_PATCH, help=f"BPS patch. Default: {HACK_PATCH}")
    ap.add_argument("--apply-patch", action="store_true",
                    help="derive the hack from --base + --patch and write it to --out, without rolling")
    ap.add_argument("--roll", choices=sorted(NAMED_ROLLS), help="a roll with a recorded hash")
    ap.add_argument("--seed", type=int, help="roll from the slot tables with this seed")
    ap.add_argument("--out", required=True, help="where to write the ROM")
    args = ap.parse_args()

    chosen = [bool(args.apply_patch), args.roll is not None, args.seed is not None]
    if sum(chosen) != 1:
        ap.error("pass exactly one of --apply-patch, --roll or --seed")

    out = pathlib.Path(args.out)
    out.parent.mkdir(parents=True, exist_ok=True)

    if args.apply_patch:
        base = pathlib.Path(args.base).read_bytes()
        print(f"base    {hashlib.sha1(base).hexdigest()}  {args.base}")
        rom = apply_bps(base, pathlib.Path(args.patch).read_bytes())
        sha1 = hashlib.sha1(rom).hexdigest()
        out.write_bytes(rom)
        print(f"hack    {sha1}  {out}")
        if sha1 != HACK_SHA1:
            print(f"MISMATCH: docs/games/smb3.md records {HACK_SHA1}", file=sys.stderr)
            return 1
        print("matches the hash in docs/games/smb3.md")
        return 0

    rom_path = args.rom or HACK_ROM
    if not pathlib.Path(rom_path).exists():
        print(f"{rom_path} does not exist; derive it first:\n"
              f"  python tools/smb3-roll.py --apply-patch --out {HACK_ROM}", file=sys.stderr)
        return 1
    rom = bytearray(pathlib.Path(rom_path).read_bytes())
    print(f"input   {hashlib.sha1(rom).hexdigest()}  {rom_path}")

    patch = NAMED_ROLLS[args.roll] if args.roll else roll_from_seed(args.seed)
    for off, val in sorted(patch.items()):
        was = rom[INES_HEADER + off]
        rom[INES_HEADER + off] = val
        note = "  (unchanged)" if was == val else ""
        print(f"  0x{off:04X}  {was:02X} -> {val:02X}  {OBJ_NAMES.get(val, '?'):<16}{note}")

    out.write_bytes(rom)
    sha1 = hashlib.sha1(rom).hexdigest()
    print(f"rolled  {sha1}  {out}")

    expected = NAMED_HASHES.get(args.roll)
    if expected and sha1 != expected:
        print(f"MISMATCH: docs/games/smb3.md records {expected}", file=sys.stderr)
        return 1
    if expected:
        print("matches the hash in docs/games/smb3.md")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
