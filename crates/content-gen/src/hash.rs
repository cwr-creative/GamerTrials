//! The two hashes content identity rests on.
//!
//! Both are hand-written rather than taken from a crate, for the same reason
//! the rest of this crate has no dependencies: an artifact's SHA-1 is compared
//! *across machines* (ADR 0013), so the code that produces it is part of the
//! contract and is cheaper to read here than to pin a version of. Neither is
//! security-sensitive — SHA-1 is an identity, and CRC32 is BPS's own accident
//! check.
//!
//! They do different jobs and neither substitutes for the other. BPS carries
//! CRC32s of its source, target and self, which catch the wrong dump cheaply at
//! apply time; SHA-1 is what a derived ROM is *identified* by, and what ADR
//! 0013's cross-player comparison rests on (ADR 0020).

/// Lowercase hex SHA-1, the spelling every hash in a manifest and in
/// `docs/games/*.md` is written in.
pub fn sha1_hex(data: &[u8]) -> String {
    let mut h: [u32; 5] = [0x6745_2301, 0xEFCD_AB89, 0x98BA_DCFE, 0x1032_5476, 0xC3D2_E1F0];

    let mut tail = Vec::with_capacity(128);
    tail.extend_from_slice(&data[data.len() - data.len() % 64..]);
    tail.push(0x80);
    while tail.len() % 64 != 56 {
        tail.push(0);
    }
    tail.extend_from_slice(&((data.len() as u64) * 8).to_be_bytes());

    for block in data[..data.len() - data.len() % 64].chunks_exact(64) {
        compress(&mut h, block.try_into().expect("chunks_exact(64)"));
    }
    for block in tail.chunks_exact(64) {
        compress(&mut h, block.try_into().expect("chunks_exact(64)"));
    }

    h.iter().map(|w| format!("{w:08x}")).collect()
}

fn compress(h: &mut [u32; 5], block: &[u8; 64]) {
    let mut w = [0u32; 80];
    for (i, word) in block.chunks_exact(4).enumerate() {
        w[i] = u32::from_be_bytes(word.try_into().expect("chunks_exact(4)"));
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }

    let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
    for (i, wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | (!b & d), 0x5A82_7999u32),
            20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1B_BCDC),
            _ => (b ^ c ^ d, 0xCA62_C1D6),
        };
        let t = a
            .rotate_left(5)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(*wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }

    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
}

/// CRC32, the ordinary zlib/PNG one, which is the flavour BPS uses.
pub fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            // The reversed polynomial, because BPS stores its CRC32s
            // little-endian and computes them the way every zip does.
            crc = (crc >> 1) ^ (0xEDB8_8320 & (0u32.wrapping_sub(crc & 1)));
        }
    }
    !crc
}

#[cfg(test)]
mod tests {
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

    /// 55 bytes is the last length whose padding fits in one block, 56 the
    /// first that needs two, 64 an exact block. A padding bug shows up here or
    /// nowhere — and a ROM is megabytes, so it is always the multi-block path.
    #[test]
    fn sha1_handles_block_boundaries() {
        assert_eq!(sha1_hex(&[b'a'; 55]), "c1c8bbdc22796e28c0e15163d20899b65621d65a");
        assert_eq!(sha1_hex(&[b'a'; 56]), "c2db330f6083854c99d4b5bfb6e8f29f201be699");
        assert_eq!(sha1_hex(&[b'a'; 64]), "0098ba824b5c16427bd7a1122a5a442a25ec644d");
        assert_eq!(sha1_hex(&[b'a'; 1000]), "291e9a6c66994949b57ba5e650361e98fc36b1ba");
    }

    #[test]
    fn crc32_matches_published_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(crc32(b"The quick brown fox jumps over the lazy dog"), 0x414F_A339);
    }
}
