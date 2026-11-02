"""Unpack a RetroArch savestate into the core's own snapshot.

RetroArch writes a state as an RZIP container (`#RZIPv\\x01#`, zlib chunks)
around a RASTATE block whose `MEM ` section is what the core serialized. The
core takes only that section; handed the container it refuses the state
(`retro_unserialize refused ...`). A state that is not compressed is passed
through unchanged, minus its RASTATE wrapping if it has one.

    python tools/rzip-unpack.py <retroarch.state> <out.state>
"""
import struct
import sys
import zlib


def unpack(data):
    if data[:8] == b"#RZIPv\x01#":
        _chunk, total = struct.unpack_from("<IQ", data, 8)
        p, out = 20, bytearray()
        while len(out) < total:
            n, = struct.unpack_from("<I", data, p)
            p += 4
            out += zlib.decompress(data[p:p + n])
            p += n
        data = bytes(out)
    if data[:8] == b"RASTATE\x01":
        p = 8
        while p < len(data):
            name = data[p:p + 4]
            size, = struct.unpack_from("<I", data, p + 4)
            p += 8
            if name == b"MEM ":
                return data[p:p + size]
            if name == b"END ":
                break
            p += (size + 7) & ~7
        raise ValueError("RASTATE block has no MEM section")
    return data


def main():
    src, dst = sys.argv[1:3]
    mem = unpack(open(src, "rb").read())
    open(dst, "wb").write(mem)
    print(f"{dst}: {len(mem)} bytes, starts {mem[:16]!r}")


if __name__ == "__main__":
    main()
