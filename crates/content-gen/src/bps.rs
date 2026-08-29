//! Applying a BPS patch.
//!
//! A hand-authored ROM hack ships as a patch against the base ROM and never as
//! a ROM (ADR 0020): a patch encodes differences, and the platform does not
//! distribute a derivative of a dump the player supplied for themselves.
//!
//! BPS rather than IPS, and the reason is in this file. A BPS patch records the
//! source size, the target size, and CRC32s of source, target and the patch
//! itself, so every one of them is checked here — a patch applied to the wrong
//! revision fails with a sentence naming what did not match, rather than
//! producing a plausible wrong ROM. IPS carries none of that.
//!
//! The format, for reading the code against:
//!
//! ```text
//! "BPS1"
//! vuint  source size
//! vuint  target size
//! vuint  metadata size, then that many bytes of (optional) XML
//! actions until 12 bytes remain:
//!   vuint  (length - 1) << 2 | action
//!     0 SourceRead  copy `length` bytes from the source at the output position
//!     1 TargetRead  copy `length` bytes that follow, inline, from the patch
//!     2 SourceCopy  signed vuint moves a source cursor; copy `length` from it
//!     3 TargetCopy  signed vuint moves an output cursor; copy `length` from it,
//!                   a byte at a time, so it may overlap what it is writing
//! u32le  CRC32 of the source
//! u32le  CRC32 of the target
//! u32le  CRC32 of the patch, up to but not including this field
//! ```
//!
//! Nothing here knows what game it is patching, and nothing may learn: a BPS
//! patch is bytes in, bytes out, and that is what keeps "adding a hack is a data
//! change" true (invariant 5, ADR 0020).

use crate::hash::crc32;

/// The `patch` of a derived `[[rom]]`, applied.
#[derive(Clone, Debug)]
pub struct Patched {
    pub bytes: Vec<u8>,
    /// What the patch said its output should hash to. Checked before this is
    /// returned; carried so a caller can report it.
    pub target_crc32: u32,
    /// The patch's own metadata, verbatim. A hack out of a level editor carries
    /// the editor's name in it — `smb3-rom-hack1.bps` says
    /// `{"project":"SMB3-hack2","tool":"Bank12"}` — which is worth printing and
    /// worth nothing else.
    pub metadata: String,
}

/// Applies `patch` to `source`, verifying every CRC32 the format carries.
///
/// The three checks are not interchangeable and each earns its own message: a
/// source mismatch means the player's dump is a different revision, a target
/// mismatch means the patch applied and the result is wrong, and a patch
/// mismatch means the patch file itself is damaged.
pub fn apply(source: &[u8], patch: &[u8]) -> Result<Patched, String> {
    if patch.len() < 4 + 12 || &patch[..4] != b"BPS1" {
        return Err("not a BPS patch (no `BPS1` magic)".to_string());
    }

    let footer = patch.len() - 12;
    let want_patch_crc = u32::from_le_bytes(patch[footer + 8..].try_into().expect("4 bytes"));
    let have_patch_crc = crc32(&patch[..footer + 8]);
    if want_patch_crc != have_patch_crc {
        return Err(format!(
            "the patch file is damaged: it records CRC32 {want_patch_crc:08x} for itself \
             and hashes to {have_patch_crc:08x}"
        ));
    }

    let mut pos = 4;
    let source_size = vuint(patch, &mut pos)?;
    let target_size = vuint(patch, &mut pos)?;
    let metadata_size = vuint(patch, &mut pos)?;
    let metadata_end = pos
        .checked_add(metadata_size)
        .filter(|&e| e <= footer)
        .ok_or("the patch's metadata runs past the end of the file")?;
    let metadata = String::from_utf8_lossy(&patch[pos..metadata_end]).into_owned();
    pos = metadata_end;

    if source.len() != source_size {
        return Err(format!(
            "this patch is against a {source_size}-byte ROM and the one in hand is {} bytes",
            source.len()
        ));
    }
    let want_source_crc =
        u32::from_le_bytes(patch[footer..footer + 4].try_into().expect("4 bytes"));
    let have_source_crc = crc32(source);
    if want_source_crc != have_source_crc {
        return Err(format!(
            "this patch is against a different ROM: it expects CRC32 {want_source_crc:08x} \
             and the one in hand is {have_source_crc:08x}"
        ));
    }

    let mut out: Vec<u8> = Vec::with_capacity(target_size);
    let (mut source_rel, mut target_rel) = (0i64, 0i64);

    while pos < footer {
        let packed = vuint(patch, &mut pos)?;
        let action = packed & 3;
        let length = (packed >> 2) + 1;
        if out.len() + length > target_size {
            return Err("the patch writes past the end of the target it declared".to_string());
        }
        match action {
            // SourceRead: the output position, read out of the source.
            0 => {
                let at = out.len();
                let end = at + length;
                if end > source.len() {
                    return Err("the patch reads past the end of the source ROM".to_string());
                }
                out.extend_from_slice(&source[at..end]);
            }
            // TargetRead: bytes carried inline. This is the whole of what a hack
            // actually changed, and the reason a patch is small.
            1 => {
                let end = pos
                    .checked_add(length)
                    .filter(|&e| e <= footer)
                    .ok_or("the patch's inline data runs past the end of the file")?;
                out.extend_from_slice(&patch[pos..end]);
                pos = end;
            }
            // SourceCopy / TargetCopy: a relative move of a cursor, then a run
            // from it. TargetCopy is deliberately a byte at a time — it is
            // allowed to read what this same action is writing, which is how BPS
            // encodes a run.
            _ => {
                let moved = vuint(patch, &mut pos)?;
                let delta = (moved >> 1) as i64;
                let delta = if moved & 1 == 1 { -delta } else { delta };
                if action == 2 {
                    source_rel += delta;
                    let at = usize::try_from(source_rel)
                        .map_err(|_| "the patch moved its source cursor before the start")?;
                    let end = at
                        .checked_add(length)
                        .filter(|&e| e <= source.len())
                        .ok_or("the patch copies past the end of the source ROM")?;
                    out.extend_from_slice(&source[at..end]);
                    source_rel = end as i64;
                } else {
                    target_rel += delta;
                    let mut at = usize::try_from(target_rel)
                        .map_err(|_| "the patch moved its target cursor before the start")?;
                    for _ in 0..length {
                        let b = *out
                            .get(at)
                            .ok_or("the patch copies from output it has not written yet")?;
                        out.push(b);
                        at += 1;
                    }
                    target_rel = at as i64;
                }
            }
        }
    }

    if out.len() != target_size {
        return Err(format!(
            "the patch declared a {target_size}-byte result and produced {}",
            out.len()
        ));
    }
    let want_target_crc =
        u32::from_le_bytes(patch[footer + 4..footer + 8].try_into().expect("4 bytes"));
    let have_target_crc = crc32(&out);
    if want_target_crc != have_target_crc {
        return Err(format!(
            "the patch did not apply cleanly: it declares CRC32 {want_target_crc:08x} for its \
             result and produced {have_target_crc:08x}"
        ));
    }

    Ok(Patched { bytes: out, target_crc32: have_target_crc, metadata })
}

/// BPS's variable-length integer: seven bits per byte, low group first, the top
/// bit marking the last byte, and an extra `+1` carried into each further group
/// so that every value has exactly one encoding.
fn vuint(buf: &[u8], pos: &mut usize) -> Result<usize, String> {
    let (mut value, mut shift) = (0u64, 1u64);
    loop {
        let b = *buf.get(*pos).ok_or("the patch ends in the middle of a number")?;
        *pos += 1;
        value += u64::from(b & 0x7F) * shift;
        if b & 0x80 != 0 {
            return usize::try_from(value)
                .map_err(|_| "a number in the patch is too large".to_string());
        }
        shift <<= 7;
        value += shift;
        if shift == 0 {
            return Err("a number in the patch is too large".to_string());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal BPS encoder, only good enough to build the patches these tests
    /// apply. Deliberately not part of the crate: nothing in the platform
    /// *writes* a patch — a contributor's level editor does.
    ///
    /// An action is `(kind, inline bytes, length-or-delta)`.
    fn encode(source: &[u8], actions: &[(u8, &[u8], i64)], target: &[u8], meta: &str) -> Vec<u8> {
        fn put(mut v: u64, out: &mut Vec<u8>) {
            loop {
                let b = (v & 0x7F) as u8;
                v >>= 7;
                if v == 0 {
                    out.push(b | 0x80);
                    return;
                }
                v -= 1;
                out.push(b);
            }
        }
        let mut p = b"BPS1".to_vec();
        put(source.len() as u64, &mut p);
        put(target.len() as u64, &mut p);
        put(meta.len() as u64, &mut p);
        p.extend_from_slice(meta.as_bytes());
        for &(action, inline, n) in actions {
            let length = if action == 1 { inline.len() as i64 } else { n };
            put((((length - 1) as u64) << 2) | u64::from(action), &mut p);
            match action {
                1 => p.extend_from_slice(inline),
                2 | 3 => put(0, &mut p), // every test copy sits at the cursor already
                _ => {}
            }
        }
        p.extend_from_slice(&crc32(source).to_le_bytes());
        p.extend_from_slice(&crc32(target).to_le_bytes());
        let self_crc = crc32(&p);
        p.extend_from_slice(&self_crc.to_le_bytes());
        p
    }

    #[test]
    fn source_read_and_target_read_together_rebuild_a_target() {
        let source = b"the base rom bytes".as_slice();
        let target = b"the BASE rom bytes".as_slice();
        // Four bytes out of the source, four carried inline, then the rest.
        let patch = encode(
            source,
            &[(0, b"", 4), (1, b"BASE", 0), (0, b"", 10)],
            target,
            "{\"tool\":\"a test\"}",
        );
        let out = apply(source, &patch).unwrap();
        assert_eq!(out.bytes, target);
        assert_eq!(out.metadata, "{\"tool\":\"a test\"}");
    }

    /// The whole reason ADR 0020 chose BPS over IPS: the wrong dump is refused
    /// rather than quietly corrupted.
    #[test]
    fn a_patch_against_another_revision_is_refused_by_crc() {
        let source = b"the base rom bytes".as_slice();
        let target = b"the BASE rom bytes".as_slice();
        let patch = encode(source, &[(0, b"", 4), (1, b"BASE", 0), (0, b"", 10)], target, "");
        let other = b"the base rom bytez".as_slice();
        let err = apply(other, &patch).unwrap_err();
        assert!(err.contains("a different ROM"), "{err}");
    }

    #[test]
    fn a_source_of_the_wrong_length_is_refused_before_anything_is_read() {
        let source = b"the base rom bytes".as_slice();
        let patch = encode(source, &[(0, b"", 18)], source, "");
        let err = apply(b"short", &patch).unwrap_err();
        assert!(err.contains("18-byte ROM"), "{err}");
    }

    #[test]
    fn a_damaged_patch_is_refused() {
        let source = b"the base rom bytes".as_slice();
        let mut patch = encode(source, &[(0, b"", 18)], source, "");
        let n = patch.len();
        patch[n - 13] ^= 0xFF;
        let err = apply(source, &patch).unwrap_err();
        assert!(err.contains("damaged"), "{err}");
    }

    #[test]
    fn something_that_is_not_a_patch_at_all_is_refused() {
        let err = apply(b"rom", b"NES and then some bytes").unwrap_err();
        assert!(err.contains("BPS1"), "{err}");
    }

    /// TargetCopy may read the bytes it is in the middle of writing; that is how
    /// the format spells a run.
    #[test]
    fn target_copy_may_overlap_its_own_output() {
        let source = b"ab".as_slice();
        let target = b"ababab".as_slice();
        let patch = encode(source, &[(0, b"", 2), (3, b"", 4)], target, "");
        assert_eq!(apply(source, &patch).unwrap().bytes, target);
    }
}
