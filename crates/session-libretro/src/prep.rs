//! Stage prep: turning what a manifest *declares* into the bytes a runner runs.
//!
//! ADR 0013 put generators "behind an interface owned by whatever prepares a
//! stage before handing it to a `StageRunner`". This is that thing, and it is
//! deliberately the only place in the crate that knows content can be derived at
//! all. Everything below it — [`crate::runner`], the watcher, `session-api` —
//! sees one opaque piece of content and could not tell a hack from a vanilla
//! dump if it tried.
//!
//! Three steps, and a stage may need any prefix of them:
//!
//! 1. **Read the base dump.** The player's own file, checked against `header`,
//!    `min_bytes` and `sha1` — everything `asset-store` will do properly.
//! 2. **Apply the hack.** If the stage's `[[rom]]` is derived, its BPS patch is
//!    applied to the base and the result checked against the declared `sha1`
//!    (ADR 0020).
//! 3. **Roll the challenge.** If the challenge declares `patch_slot`s, the
//!    seeded engine substitutes values into the ROM from step 2 (ADR 0015).
//!
//! Steps 2 and 3 compose in that order and only that order: a human designs the
//! level, the engine rerolls the enemies inside it. The offsets in step 3 are
//! into whatever step 2 produced, which is why the derivation key names the
//! *hack's* hash and not the base game's (ADR 0020).
//!
//! Nothing derived is ever sent anywhere. The cache below is on the machine that
//! derived it, and that is the whole of ADR 0013's copyright argument: a player
//! supplies their own dump, a contributor ships a patch, and the bytes in
//! between never leave.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use content_gen::{bps, roll::Artifact, PatchTable};
use session_api::AssetKey;

use crate::manifest::{Challenge, Manifest, RomHeader, RomSpec, Stage};

/// Where a locally derived artifact is kept, until `asset-store` owns this.
///
/// A cache, not an output directory: it exists so that repeating a challenge
/// does not repeat the work, and so that a file exists for the workflows that
/// want one — capturing an entry state on a hack, or diffing two rolls. ADR 0013
/// keys the real one on the derivation key, which is what the filename here is.
const CACHE_DIR: &str = "build/derived";

/// The bytes a runner is handed, and just enough about where they came from to
/// log it and to know what may be done with them.
pub struct Content {
    /// What ADR 0013 compares across players: the SHA-1 of these exact bytes.
    pub key: AssetKey,
    /// What the core is told the content is called. A real file for a base ROM;
    /// for a derived one, the cache entry these bytes were written to.
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    /// Whether an entry state captured on the stage's ROM may be rebound onto
    /// these bytes (ADR 0019).
    ///
    /// True only when the seeded engine produced them from the ROM the stage
    /// names — same length, same mapper, same geometry, which is what makes the
    /// transplant sound. It is false for a hand-authored hack, always: a hack is
    /// structural, its states are captured on it, and the core refusing a base
    /// state on it is the check working (ADR 0020). Encoded here rather than
    /// left to a flag because it is a property of how the content was made, not
    /// a preference.
    pub rebindable: bool,
    /// What a player should be shown. A hack has its own name, and
    /// `[game].title` is the base game's.
    pub title: String,
}

/// One `--rom <id>=<path>`: run this ROM from a file instead of deriving or
/// reading the one the manifest names.
///
/// It takes an id because ADR 0020 made the bare form ambiguous — a manifest has
/// more than one ROM now, and "the ROM" stopped meaning anything. Overriding the
/// base means "my dump lives elsewhere"; overriding a derived one means "I have
/// already applied the patch", which is the loop for anyone iterating on a hack.
pub type RomOverrides = BTreeMap<String, PathBuf>;

/// Everything between a manifest and a runnable machine.
///
/// `seed` is `None` for a vanilla challenge and for the authoring loop; a
/// generated challenge without one runs *unrolled*, loudly, because that is
/// exactly how its entry state gets captured in the first place
/// (`docs/games/smb3.md`).
pub fn content_for(
    manifest: &Manifest,
    stage: &Stage,
    challenge: Option<&Challenge>,
    seed: Option<u64>,
    overrides: &RomOverrides,
) -> Result<Content, String> {
    let rom = manifest
        .rom(&stage.rom)
        .ok_or_else(|| format!("stage `{}` names no declared rom", stage.id))?;
    let base = manifest.base_rom();

    // Step 1 and 2: the machine the stage runs on, before any seed.
    let (mut bytes, mut path) = if let Some(over) = overrides.get(&rom.id) {
        let bytes = read(over)?;
        println!("[rom]  {} <- {} (--rom)", rom.id, over.display());
        if rom.from.is_none() {
            check_dump(rom, &bytes, over)?;
        }
        check_sha1(rom, &bytes)?;
        (bytes, over.clone())
    } else if rom.from.is_none() {
        let p = base_path(manifest, rom, overrides)?;
        let bytes = read(&p)?;
        check_dump(rom, &bytes, &p)?;
        check_sha1(rom, &bytes)?;
        (bytes, p)
    } else {
        let p = base_path(manifest, base, overrides)?;
        let base_bytes = read(&p)?;
        check_dump(base, &base_bytes, &p)?;
        check_sha1(base, &base_bytes)?;
        let patched = apply_hack(manifest, rom, &base_bytes)?;
        let cached = cache(manifest, rom, &content_gen::sha1_hex(&patched), &patched, &p)?;
        (patched, cached)
    };

    let mut key = AssetKey(content_gen::sha1_hex(&bytes));
    let mut rebindable = false;

    // Step 3: the roll, if this challenge is a generated one.
    let table = challenge.and_then(|c| manifest.patch_table(&c.id));
    if table.is_none() && seed.is_some() {
        // A seed that decides nothing is someone expecting content to vary. Say
        // so rather than running the same ROM for every seed they try.
        println!(
            "[gen]  a seed was given and nothing rolls: {}. The seed is ignored",
            match challenge {
                Some(c) => format!("challenge `{}` declares no patch slots", c.id),
                None => "no --challenge was given, and patch slots hang off a challenge".into(),
            }
        );
    }
    if let Some(table) = table {
        match seed {
            Some(seed) => {
                let console = manifest
                    .console_family()
                    .map_err(|e| format!("[game].console: {e}"))?;
                let artifact = content_gen::derive(&bytes, console, table, seed)?;
                report(table, &artifact);
                path = cache(manifest, rom, &artifact.key.digest(), &artifact.bytes, &path)?;
                key = AssetKey(artifact.sha1.clone());
                bytes = artifact.bytes;
                // The one case ADR 0019 sanctions: these bytes came out of the
                // value-substitution engine, applied to the ROM this stage
                // names, so a state captured on that ROM describes this machine.
                rebindable = true;
            }
            None => {
                println!(
                    "[gen]  challenge `{}` declares {} patch slots and no seed was given, so \
                     this is the unrolled ROM. That is the right thing for capturing an entry \
                     state, and the wrong thing for playing the challenge",
                    table.generator_id,
                    table.slots.len()
                );
            }
        }
    }

    Ok(Content {
        key,
        path,
        bytes,
        rebindable,
        title: rom.name.clone().unwrap_or_else(|| manifest.title.clone()),
    })
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("cannot read content {}: {e}", path.display()))
}

fn base_path(
    manifest: &Manifest,
    rom: &RomSpec,
    overrides: &RomOverrides,
) -> Result<PathBuf, String> {
    if let Some(p) = overrides.get(&rom.id) {
        return Ok(p.clone());
    }
    let declared = rom
        .path
        .as_ref()
        .ok_or_else(|| format!("rom `{}` names no path; pass --rom {}=<PATH>", rom.id, rom.id))?;
    Ok(PathBuf::from(manifest.resolve(&declared.to_string_lossy())))
}

/// The checks that recognise a good *dump*, and nothing more. They belong to the
/// base ROM: a hack may legitimately be a different size, and its `sha1` is a
/// stricter check than either of these (ADR 0020).
fn check_dump(rom: &RomSpec, bytes: &[u8], path: &Path) -> Result<(), String> {
    if bytes.len() < rom.min_bytes {
        return Err(format!(
            "{} is {} bytes, below the {} this manifest expects — is it a different dump?",
            path.display(),
            bytes.len(),
            rom.min_bytes
        ));
    }
    let mut note = String::new();
    match rom.header {
        RomHeader::None => {}
        RomHeader::INes => {
            if bytes.len() < 16 || &bytes[0..4] != b"NES\x1a" {
                return Err(format!("{} has no iNES header", path.display()));
            }
        }
        RomHeader::CopierOptional => {
            if bytes.len() % 1024 == 512 {
                note = ", 512-byte copier header".to_string();
            }
        }
    }
    println!(
        "[rom]  {} ({} bytes{note}) <- {}",
        rom.id,
        bytes.len(),
        path.display()
    );
    Ok(())
}

/// A declared hash is the identity, and a mismatch is refused rather than
/// warned about: on a derived ROM it is what ADR 0013's cross-player comparison
/// rests on, and on a base ROM it is the difference between two revisions whose
/// addresses do not agree.
fn check_sha1(rom: &RomSpec, bytes: &[u8]) -> Result<(), String> {
    let Some(want) = &rom.sha1 else { return Ok(()) };
    let have = content_gen::sha1_hex(bytes);
    if !have.eq_ignore_ascii_case(want.trim()) {
        return Err(format!(
            "rom `{}` should hash to {want} and hashes to {have}",
            rom.id
        ));
    }
    Ok(())
}

fn apply_hack(manifest: &Manifest, rom: &RomSpec, base: &[u8]) -> Result<Vec<u8>, String> {
    let patch_path = rom.patch.as_ref().expect("validation proved a derived rom has a patch");
    let patch_path = PathBuf::from(manifest.resolve(&patch_path.to_string_lossy()));
    let patch = read(&patch_path)?;
    let out = bps::apply(base, &patch).map_err(|e| format!("rom `{}`: {e}", rom.id))?;
    check_sha1(rom, &out.bytes)?;
    println!(
        "[hack] {} <- {} ({} bytes -> {}){}",
        rom.id,
        patch_path.display(),
        base.len(),
        out.bytes.len(),
        if out.metadata.is_empty() {
            String::new()
        } else {
            format!("  {}", out.metadata)
        }
    );
    Ok(out.bytes)
}

/// Every byte the roll wrote, including the ones it did not change.
///
/// A slot rolling back to the value it already held is a normal outcome — for
/// SMB3's Sun Storm it is a quarter of all rolls — so a log that could not show
/// it would make a working engine look broken.
fn report(table: &PatchTable, artifact: &Artifact) {
    println!(
        "[gen]  {} v{} seed {} -> {}",
        artifact.key.generator_id,
        artifact.key.generator_version,
        artifact.key.seed,
        artifact.sha1
    );
    for (w, slot) in artifact.writes.iter().zip(&table.slots) {
        println!(
            "[gen]    {:#06X}  {:02X} -> {:02X}{}{}",
            w.offset,
            w.before,
            w.after,
            if w.changed() { "" } else { "  (unchanged)" },
            match &slot.note {
                Some(n) => format!("  {n}"),
                None => String::new(),
            }
        );
    }
    if let Some(fix) = artifact.fixup {
        println!(
            "[gen]    checksum at {:#X}: {:04X} -> {:04X}",
            fix.header_at, fix.was, fix.now
        );
    }
}

/// Writes a derived artifact to the local cache, and returns where it went.
///
/// Named by what produced it, so that two rolls of one challenge are two files
/// and re-deriving either is a hit rather than a rewrite. A failure to write is
/// reported and not fatal: the bytes are already in hand, and a cache that
/// cannot be written is slower rather than broken.
fn cache(
    manifest: &Manifest,
    rom: &RomSpec,
    tag: &str,
    bytes: &[u8],
    like: &Path,
) -> Result<PathBuf, String> {
    let ext = like.extension().map(|e| e.to_string_lossy().into_owned()).unwrap_or_default();
    let dir = PathBuf::from(CACHE_DIR);
    let name = format!(
        "{}.{}.{}{}{}",
        manifest.id,
        rom.id,
        &tag[..tag.len().min(12)],
        if ext.is_empty() { "" } else { "." },
        ext
    );
    let path = dir.join(name);
    if path.exists() {
        return Ok(path);
    }
    if let Err(e) = fs::create_dir_all(&dir).and_then(|()| fs::write(&path, bytes)) {
        eprintln!("[gen]  cannot cache {}: {e}", path.display());
        // Still name it: the caller only uses the path to tell the core what the
        // content is called and to site a save directory.
        return Ok(path);
    }
    println!("[gen]  cached {}", path.display());
    Ok(path)
}
