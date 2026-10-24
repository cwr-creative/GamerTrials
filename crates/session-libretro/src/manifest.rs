//! Reading a game manifest.
//!
//! A manifest is the whole of what this crate knows about a game. There is no
//! per-game Rust anywhere below this line: SMB3 and DKC are two TOML files that
//! differ in their addresses and in six rules each, and adding a third libretro
//! game is a third file (invariant 5).
//!
//! The schema is documented in `docs/manifest-schema.md`; the two shipped
//! manifests in `manifests/` are the worked examples. This module is the
//! authority on what parses, and it is deliberately strict — every table denies
//! unknown fields, because the failure mode a permissive loader produces is a
//! detector that silently never fires from a typo in a key name, and that looks
//! exactly like a wrong address.
//!
//! It reads the *runner* half. The challenge half — `[game]`, the stages by
//! name, every `[[challenge]]` and its criteria — is `challenge-library`'s,
//! because the server deals and judges from it and must not link an emulator
//! to do so (ADR 0009, ADR 0024). [`Manifest::parse`] runs that crate first and
//! builds on what it returns, so the client and the server cannot read one file
//! into two different libraries. What stays here from `[[challenge]]` is the
//! `patch_slot` table, because building it needs `content-gen`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use challenge_library::Library;
use content_gen::{Console, GroupDoc, PatchSlotDoc, PatchTable};
use serde::de::IgnoredAny;
use serde::Deserialize;
use session_api::{
    AssetKey, ChallengeId, ChallengeSpec, EntryPoint, Framerate, GameId, StageId, StageSpec,
};

use crate::memory::{AddressSpace, Endian, Prefer, Width};

// One manifest, one version, one fact vocabulary: all three are the challenge
// half's, and re-exported so that nothing here can drift from them.
pub use challenge_library::{Challenge, Emit, SCHEMA_VERSION};

// ------------------------------------------------------------- raw documents

/// Every top-level table, so that an unknown one is still refused here. The
/// ones `challenge-library` owns are skipped rather than read twice.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Doc {
    #[serde(rename = "schema_version")]
    _schema_version: IgnoredAny,
    #[serde(rename = "game")]
    _game: IgnoredAny,
    /// One or more. Exactly one omits `from` and is the base; the rest are
    /// derivations from it (ADR 0020).
    rom: Vec<RomDoc>,
    runner: RunnerDoc,
    #[serde(default)]
    watch: BTreeMap<String, WatchDoc>,
    #[serde(default)]
    label: BTreeMap<String, BTreeMap<String, String>>,
    #[serde(default)]
    display: DisplayDoc,
    #[serde(default)]
    marker: Vec<MarkerDoc>,
    #[serde(default)]
    rule: Vec<RuleDoc>,
    #[serde(default)]
    attempt: AttemptDoc,
    #[serde(default)]
    stage: Vec<StageDoc>,
    #[serde(default)]
    challenge: Vec<ChallengeGenDoc>,
}

/// One piece of content this game runs on: the base ROM, or a hack derived from
/// it by a BPS patch (ADR 0020).
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct RomDoc {
    /// What a `[[stage]]` names. Unique inside the manifest.
    id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sha1: Option<String>,
    /// The file. On the base only: a derived ROM is not a file, it is a
    /// derivation, and the platform never distributes one (ADR 0013, ADR 0020).
    #[serde(default)]
    path: Option<String>,
    /// Its presence is what makes this ROM derived. Must name the base.
    #[serde(default)]
    from: Option<String>,
    /// The BPS patch that derives it.
    #[serde(default)]
    patch: Option<String>,
    /// Base only. `header` and `min_bytes` exist to recognise a good *dump*; a
    /// hack may legitimately be a different size, and its `sha1` is a stricter
    /// check than either.
    #[serde(default)]
    header: Option<String>,
    #[serde(default)]
    min_bytes: Option<i64>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct RunnerDoc {
    kind: String,
    #[serde(default)]
    libretro: Option<LibretroDoc>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct LibretroDoc {
    core_dir: String,
    #[serde(default)]
    controller: Option<String>,
    #[serde(default)]
    pad_layout: Option<String>,
    /// Authored in seconds, converted at load like every other duration.
    #[serde(default)]
    boot_seconds: Option<f64>,
    /// Core options this game is pinned to. Checked against what the core
    /// publishes once it has published it; an unknown key is an error.
    #[serde(default)]
    options: BTreeMap<String, String>,
    memory: MemoryDoc,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct MemoryDoc {
    base: i64,
    size: i64,
    prefer: String,
    #[serde(default)]
    endian: Option<String>,
    #[serde(default)]
    default_width: Option<String>,
    #[serde(default)]
    system_ram_bytes: Option<i64>,
    #[serde(default)]
    scan: Option<ScanDoc>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ScanDoc {
    at: i64,
    size: i64,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct WatchDoc {
    /// `None` is written as an absent key, and means "nobody has found this
    /// address yet". Every rule over the field then stays quiet.
    #[serde(default)]
    at: Option<i64>,
    #[serde(default)]
    width: Option<String>,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct DisplayDoc {
    #[serde(default)]
    fields: Vec<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct MarkerDoc {
    name: String,
    field: String,
    edge: String,
    #[serde(default)]
    value: Option<i64>,
    #[serde(default)]
    bit: Option<u32>,
    window_frames: u64,
    #[serde(default)]
    capture: Vec<CaptureDoc>,
}

/// A test over one field's current value. A rule's `guard` is exactly this;
/// a marker's `capture` is this plus a name to remember the answer under.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct TestDoc {
    field: String,
    test: String,
    #[serde(default)]
    value: Option<i64>,
    #[serde(default)]
    bit: Option<u32>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct CaptureDoc {
    name: String,
    field: String,
    test: String,
    #[serde(default)]
    value: Option<i64>,
    #[serde(default)]
    bit: Option<u32>,
}

impl CaptureDoc {
    fn as_test(&self) -> TestDoc {
        TestDoc {
            field: self.field.clone(),
            test: self.test.clone(),
            value: self.value,
            bit: self.bit,
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct RuleDoc {
    #[serde(default)]
    name: Option<String>,
    field: String,
    edge: String,
    #[serde(default)]
    value: Option<i64>,
    #[serde(default)]
    bit: Option<u32>,
    #[serde(default)]
    bit_labels: Vec<String>,
    #[serde(default)]
    guard: Option<TestDoc>,
    #[serde(default)]
    unless: Vec<String>,
    #[serde(default)]
    timed_to: Option<String>,
    #[serde(default)]
    if_marked: Option<String>,
    emit: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    objective: Option<String>,
    #[serde(default)]
    log: Option<String>,
}

#[derive(Deserialize, Debug, Default)]
#[serde(deny_unknown_fields)]
struct AttemptDoc {
    #[serde(default)]
    reset_on: Vec<TriggerDoc>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct TriggerDoc {
    field: String,
    edge: String,
    #[serde(default)]
    value: Option<i64>,
    #[serde(default)]
    bit: Option<u32>,
}

/// Where a run starts, and nothing else. A stage carries no completion
/// condition of its own and does not say which kinds it can host — that is
/// expressed by which `[[challenge]]`s name it (ADR 0017).
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct StageDoc {
    #[serde(rename = "id")]
    _id: IgnoredAny,
    /// Read by `challenge-library`, which owns what a stage is called.
    #[serde(default, rename = "name")]
    _name: Option<IgnoredAny>,
    /// Which `[[rom]]` this stage runs on. Required in every manifest,
    /// including one that declares a single ROM: nothing about a stage is
    /// inferred, for the same reason nothing about a challenge is (ADR 0017,
    /// ADR 0020).
    rom: String,
    entry: EntryDoc,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct EntryDoc {
    /// A captured machine state to restore. Mutually exclusive with `named`.
    #[serde(default)]
    state: Option<String>,
    /// A location the game can be told to start at. Not used by any libretro
    /// game here; it is what a Doom manifest would carry (ADR 0002).
    #[serde(default)]
    named: Option<String>,
}

/// The one part of a `[[challenge]]` this loader reads: ADR 0015's patch table.
/// Everything else in the table, and whether these keys appear together, is
/// `challenge-library`'s — which is also why this is not `deny_unknown_fields`.
///
/// The shapes are `content-gen`'s own, deserialized straight into its types.
/// The engine that reads a slot owns what a slot looks like; the challenge
/// half only decides where in a manifest one may appear.
#[derive(Deserialize, Debug)]
struct ChallengeGenDoc {
    id: String,
    #[serde(default)]
    patch_slot: Vec<PatchSlotDoc>,
    #[serde(default)]
    group: BTreeMap<String, GroupDoc>,
}

// -------------------------------------------------------------- loaded shape

/// One watched quantity, named.
#[derive(Clone, Debug)]
pub struct WatchDef {
    pub name: String,
    pub at: Option<usize>,
    pub width: Width,
    pub note: String,
}

/// A test evaluated against the current frame's value of one field. Used both
/// as a rule guard and as a marker capture.
#[derive(Clone, Debug)]
pub struct Test {
    pub field: String,
    pub kind: TestKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TestKind {
    Eq(u32),
    Ne(u32),
    BitSet(u32),
    BitCleared(u32),
}

/// How a field has to move for something to fire.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Edge {
    Increases,
    Decreases,
    Changes,
    /// `prev != v && cur == v`.
    Enters(u32),
    /// `prev == v && cur != v`. `leaves 0` is how "rose from zero" is spelled.
    Leaves(u32),
    BitSet(u32),
    BitCleared(u32),
    /// Every bit newly set this frame, reported one at a time.
    BitsSet,
}

#[derive(Clone, Debug)]
pub struct Rule {
    pub name: Option<String>,
    pub field: String,
    pub edge: Edge,
    pub bit_labels: Vec<String>,
    pub guard: Option<Test>,
    pub unless: Vec<String>,
    pub timed_to: Option<String>,
    pub if_marked: Option<String>,
    pub emit: Emit,
    pub log: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Marker {
    pub name: String,
    pub field: String,
    pub edge: Edge,
    pub window_frames: u64,
    pub captures: Vec<(String, Test)>,
}

#[derive(Clone, Debug)]
pub struct Trigger {
    pub field: String,
    pub edge: Edge,
}

/// A place, and the entry point that reaches it. Setup, not scoring.
#[derive(Clone, Debug)]
pub struct Stage {
    pub id: StageId,
    pub name: String,
    /// The `[[rom]]` id this stage runs on. ROM and entry point belong in one
    /// table because an entry state is bound to one ROM's bytes — ADR 0019
    /// measured that rather than assuming it — and because several challenges
    /// share a stage and must not be able to disagree about which machine they
    /// are on (ADR 0020).
    pub rom: String,
    pub entry: EntryPoint,
    pub note: Option<String>,
}

/// Everything the libretro adapter needs about a game that is not an address
/// or a rule.
#[derive(Clone, Debug)]
pub struct LibretroConfig {
    pub core_dir: PathBuf,
    pub controller: Controller,
    pub space: AddressSpace,
    pub default_width: Width,
    /// What `RETRO_MEMORY_SYSTEM_RAM` is expected to be. A core handing back
    /// something else is not wrong, but every address in the manifest was
    /// written against the expected one, so startup says so.
    pub system_ram_bytes: Option<usize>,
    /// Where the RAM discovery tools look, when the core publishes no
    /// system-RAM slice for them to work on. See [`ScanWindow`].
    pub scan: Option<ScanWindow>,
    /// Core options pinned by the manifest. Part of the game's identity rather
    /// than a preference: `dosbox_pure_cycles` decides how many emulated frames
    /// a piece of play takes, and two players on different values have
    /// incomparable scores (ADR 0018, invariant 4).
    pub options: BTreeMap<String, String>,
    /// How long a cold boot takes before the game is at its named entry point.
    /// Zero for a console, where `Named` is not used at all; ~900 frames for a
    /// DOS bundle that has to start DOS first.
    pub boot_frames: u64,
}

/// A contiguous window of the address space the RAM discovery tools work over.
///
/// The console case needs none of this: `RETRO_MEMORY_SYSTEM_RAM` *is* the
/// window, it is small, and every interesting byte is in it. A DOS core
/// publishes no such slice and 16 MB of address space instead, which is both
/// too big to snapshot every frame and mostly not worth searching. So a DOS
/// manifest says where to look, and that is a data change like every other
/// (invariant 5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ScanWindow {
    pub at: usize,
    pub size: usize,
}

/// How port 0 is driven, and how a host controller maps onto it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Controller {
    /// `RETRO_DEVICE_JOYPAD`. The libretro button ids are the same whichever
    /// console it is; the layout only decides how a modern pad's diamond lands
    /// on them.
    Pad(PadLayout),
    /// `RETRO_DEVICE_KEYBOARD`. A DOS game is bound to keys, and to *its own*
    /// keys — Doom's controls are configured inside Doom. So there is nothing
    /// to lay out: the host keyboard passes straight through, and a manifest
    /// that tried to remap it would be overriding a choice the player already
    /// made in the game.
    Keyboard,
}

/// Which physical pad the console had. It decides nothing about the core — the
/// libretro button ids are the same either way — only how a modern controller's
/// diamond is mapped onto it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PadLayout {
    /// Two face buttons. South *or* East is A, so both the "A is on the right"
    /// and "A is on the bottom" habits work without remapping.
    Nes,
    /// Four face buttons and two shoulders, in the same diamond every modern
    /// pad has, so the mapping is positional.
    Snes,
}

/// One piece of content the game runs on.
///
/// Either it is the base — a dump the player supplies, recognised by `header`,
/// `min_bytes` and eventually `sha1` — or it is derived from the base by a BPS
/// patch, in which case `sha1` is the artifact's and is the identity (ADR 0020).
/// There is no third kind, and in particular there is no field holding a path to
/// a hacked ROM: that would be the platform distributing a derivative of a
/// copyrighted dump, which is the thing ADR 0013 was built to avoid.
#[derive(Clone, Debug)]
pub struct RomSpec {
    /// What a `[[stage]]` names.
    pub id: String,
    pub name: Option<String>,
    pub sha1: Option<String>,
    pub path: Option<PathBuf>,
    /// The base this one is derived from, and the patch that derives it. Both
    /// present or both absent; absent means this *is* the base.
    pub from: Option<String>,
    pub patch: Option<PathBuf>,
    /// `"ines"` checks the 16-byte iNES magic; `"copier-optional"` notes a
    /// 512-byte SNES copier header when the size implies one; `"none"` checks
    /// nothing. The set is closed and small, per console family, which is the
    /// same closed set ADR 0015 needs for checksum fixup.
    pub header: RomHeader,
    pub min_bytes: usize,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RomHeader {
    None,
    INes,
    CopierOptional,
}

/// A whole game, loaded and checked.
#[derive(Clone, Debug)]
pub struct Manifest {
    pub id: GameId,
    pub title: String,
    pub console: String,
    pub region: Option<String>,
    pub framerate: Framerate,
    /// Every ROM this game runs on, base first. One entry is the ordinary case;
    /// a second is a hand-authored hack (ADR 0020).
    pub roms: Vec<RomSpec>,
    pub libretro: LibretroConfig,
    pub watches: Vec<WatchDef>,
    pub labels: BTreeMap<String, BTreeMap<u32, String>>,
    pub display: Vec<String>,
    pub markers: Vec<Marker>,
    pub rules: Vec<Rule>,
    pub reset_on: Vec<Trigger>,
    pub stages: Vec<Stage>,
    /// As `challenge-library` read them: the same values a server deals from.
    pub challenges: Vec<Challenge>,
    /// How each generated challenge's ROM is derived from the one its stage
    /// names (ADR 0013, ADR 0015), by challenge. Validated at load; rolled by
    /// `content-gen` when a stage is prepared, from a seed the session issues.
    /// A vanilla challenge has no entry, and that is the ordinary case.
    patch_tables: BTreeMap<ChallengeId, PatchTable>,
    /// Where it was read from, for messages and for resolving relative paths.
    pub source: PathBuf,
}

impl Manifest {
    pub fn load(path: &Path) -> Result<Manifest, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read manifest {}: {e}", path.display()))?;
        Manifest::parse(&text, path)
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The challenge half first, then the runner half on top of it. The
    /// version, `[game]`, stage names and every criterion are checked once, by
    /// the crate the server also uses.
    pub fn parse(text: &str, source: &Path) -> Result<Manifest, String> {
        let library = Library::parse(text)?;
        let doc: Doc = toml::from_str(text).map_err(|e| e.to_string())?;
        doc.into_manifest(source, library)
    }

    pub fn watch(&self, name: &str) -> Option<&WatchDef> {
        self.watches.iter().find(|w| w.name == name)
    }

    pub fn stage(&self, id: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id.0 == id)
    }

    pub fn rom(&self, id: &str) -> Option<&RomSpec> {
        self.roms.iter().find(|r| r.id == id)
    }

    /// The one ROM that is not derived from another. Validation proved there is
    /// exactly one, and it is first.
    pub fn base_rom(&self) -> &RomSpec {
        self.roms.first().expect("validation proved a manifest declares a base ROM")
    }

    /// The console family, for the two things that key off it: the header a
    /// patch-slot offset is counted from, and the checksum a substitution has to
    /// repair (ADR 0015). Everything else about a game is declared, not implied
    /// by the name of its machine.
    pub fn console_family(&self) -> Result<Console, String> {
        Console::parse(&self.console)
    }

    /// What a player should be shown for a stage: the ROM's name if it has one,
    /// because a hack has its own title and `[game].title` is the base game's
    /// (ADR 0020).
    pub fn content_title(&self, stage: &Stage) -> String {
        self.rom(&stage.rom)
            .and_then(|r| r.name.clone())
            .unwrap_or_else(|| self.title.clone())
    }

    pub fn challenge(&self, id: &str) -> Option<&Challenge> {
        self.challenges.iter().find(|c| c.id.0 == id)
    }

    /// Every challenge that runs on one stage, in declaration order.
    pub fn challenges_on<'a>(
        &'a self,
        stage: &'a StageId,
    ) -> impl Iterator<Item = &'a Challenge> {
        self.challenges.iter().filter(move |c| &c.stage == stage)
    }

    /// The table a generated challenge rolls from, or `None` for a vanilla one.
    pub fn patch_table(&self, challenge: &ChallengeId) -> Option<&PatchTable> {
        self.patch_tables.get(challenge)
    }

    /// Applies one `--addr name=ADDR[:width]` override, or unsets the field.
    pub fn set_addr(
        &mut self,
        name: &str,
        at: Option<usize>,
        width: Option<Width>,
    ) -> Result<(), String> {
        let space = self.libretro.space;
        let known: Vec<String> = self.watches.iter().map(|w| w.name.clone()).collect();
        let w = self
            .watches
            .iter_mut()
            .find(|w| w.name == name)
            .ok_or_else(|| {
                format!("unknown watch `{name}`; this manifest declares {}", known.join(", "))
            })?;
        w.at = match at {
            None => None,
            Some(a) => {
                // An address outside every route to a byte is refused rather
                // than silently pointing a detector at nothing. The bound is
                // the space the manifest declared, not a constant: a DOS
                // machine's is 16 MB and a console's is a few KB, and a
                // hard-coded 24-bit ceiling would be wrong for both.
                let reach = space.base.saturating_add(space.size);
                if a >= reach {
                    return Err(format!(
                        "{} is past the end of this machine's address space ({})",
                        space.show(a),
                        space.show(reach.saturating_sub(1))
                    ));
                }
                // Folded to the offset form the manifest itself uses, so
                // `$7E08A6` and `$08A6` are stored identically however they
                // were typed.
                Some(space.offset(a).unwrap_or(a))
            }
        };
        if let Some(x) = width {
            w.width = x;
        }
        Ok(())
    }

    /// The spec a `StageRunner` is handed, resolved against the manifest's own
    /// directory so a manifest can name its states relatively.
    ///
    /// `content` is whatever stage prep actually derived — the base ROM's key
    /// for a vanilla stage, an artifact's hash for a hack or a roll (ADR 0020).
    /// It is passed in rather than computed here because this module reads
    /// declarations and `content-gen` produces bytes, and only the second knows
    /// what the first turned into.
    pub fn stage_spec(&self, stage: &Stage, content: AssetKey) -> StageSpec {
        let entry = match &stage.entry {
            EntryPoint::State(key) => EntryPoint::State(AssetKey(self.resolve(&key.0))),
            other => other.clone(),
        };
        StageSpec { game: self.id.clone(), stage: stage.id.clone(), content, entry }
    }

    /// The challenge as `challenge-core` will see it — built by
    /// `challenge-library`, so it is the spec a server would build too.
    pub fn challenge_spec(&self, challenge: &Challenge) -> ChallengeSpec {
        challenge.spec(&self.id)
    }

    /// Resolves a path a manifest names.
    ///
    /// Absolute paths are left alone. A relative one is tried as written first —
    /// the shipped manifests point at `assets/` from the repository root, which
    /// is where the harness is run from — and then relative to the manifest
    /// itself, so a manifest kept beside its own assets also works. If neither
    /// exists it is returned as written, and the caller's open fails with the
    /// path the author typed rather than one this function invented.
    ///
    /// `asset-store` replaces all of this with content addressing (ADR 0013).
    pub fn resolve(&self, p: &str) -> String {
        let path = Path::new(p);
        if path.is_absolute() || path.exists() {
            return p.to_string();
        }
        match self.source.parent() {
            Some(dir) if dir.join(path).exists() => dir.join(path).to_string_lossy().into_owned(),
            _ => p.to_string(),
        }
    }
}

// ------------------------------------------------------------- construction

/// Every `[[rom]]`, checked into the shape ADR 0020 describes: exactly one base,
/// every other one derived from it by a patch, and no field on the wrong kind.
///
/// The six rules here are the whole of what makes an array of ROMs mean one
/// thing, so each refusal says which one it is rather than "invalid".
fn build_roms(docs: &[RomDoc], console: &str) -> Result<Vec<RomSpec>, String> {
    if docs.is_empty() {
        return Err("no [[rom]]: a manifest declares the content its stages run on".to_string());
    }

    let bases: Vec<&RomDoc> = docs.iter().filter(|r| r.from.is_none()).collect();
    let base = match bases.as_slice() {
        [one] => *one,
        [] => {
            return Err("every [[rom]] declares a `from`, so none of them is the base. \
                        Exactly one omits it, and that one is the dump the player supplies"
                .to_string())
        }
        many => {
            return Err(format!(
                "{} [[rom]] entries omit `from` ({}), so there is no single base. A hack is \
                 declared as a derivation from the base, never as a second ROM of its own \
                 (ADR 0020)",
                many.len(),
                many.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", ")
            ))
        }
    };

    let mut out: Vec<RomSpec> = Vec::with_capacity(docs.len());
    // Base first, so that `base_rom()` is `roms[0]` and the order a manifest
    // happens to declare them in decides nothing.
    for doc in std::iter::once(base).chain(docs.iter().filter(|r| r.from.is_some())) {
        let what = format!("rom `{}`", doc.id);
        if out.iter().any(|r| r.id == doc.id) {
            return Err(format!("{what}: declared twice"));
        }

        if let Some(from) = &doc.from {
            if from != &base.id {
                return Err(format!(
                    "{what}: `from` names `{from}`, which is not the base (`{}`). A patch \
                     against a patch is refused; a derivation stays two deep",
                    base.id
                ));
            }
            if doc.patch.is_none() {
                return Err(format!(
                    "{what}: declares `from` and no `patch`. A derived ROM *is* its patch — \
                     the platform carries the difference, never the hacked ROM (ADR 0020)"
                ));
            }
            if doc.path.is_some() {
                return Err(format!(
                    "{what}: a derived ROM has no `path`. A field holding a path to a hacked \
                     ROM is the platform distributing a derivative of a copyrighted dump, \
                     which is what ADR 0013 generates client-side to avoid"
                ));
            }
            if doc.sha1.is_none() {
                return Err(format!(
                    "{what}: a derived ROM needs the `sha1` of the artifact its patch \
                     produces. BPS's CRC32s catch an accident; the sha1 is the identity \
                     ADR 0013's cross-player comparison rests on"
                ));
            }
            // `header` and `min_bytes` recognise a good dump. A hack may
            // legitimately be a different size, and its sha1 is stricter than
            // either, so declaring one here is an error rather than ignored.
            if doc.header.is_some() || doc.min_bytes.is_some() {
                return Err(format!(
                    "{what}: `header` and `min_bytes` belong to the base ROM only — they \
                     recognise a dump, and this one is derived from a dump that was already \
                     recognised"
                ));
            }
            // A BPS patch against a zip is meaningless: the bundle's hash
            // depends on how it was zipped. Refused by name rather than
            // silently accepted (ADR 0018, ADR 0020).
            if let Err(e) = Console::parse(console).and_then(|c| c.is_derivable()) {
                return Err(format!("{what}: {e}"));
            }
        } else if doc.patch.is_some() {
            return Err(format!(
                "{what}: declares a `patch` and no `from`, so there is nothing to apply it to"
            ));
        }

        out.push(RomSpec {
            id: doc.id.clone(),
            name: doc.name.clone(),
            sha1: doc.sha1.clone(),
            path: doc.path.clone().map(PathBuf::from),
            from: doc.from.clone(),
            patch: doc.patch.clone().map(PathBuf::from),
            header: match doc.header.as_deref() {
                None | Some("none") => RomHeader::None,
                Some("ines") => RomHeader::INes,
                Some("copier-optional") => RomHeader::CopierOptional,
                Some(other) => {
                    return Err(format!(
                        "{what}: header is `{other}`; try none, ines or copier-optional"
                    ))
                }
            },
            min_bytes: match doc.min_bytes {
                Some(v) => as_usize(v, &format!("{what}: min_bytes"))?,
                None => 0,
            },
        });
    }

    Ok(out)
}

fn parse_int(s: &str) -> Result<u32, String> {
    let t = s.trim();
    let (radix, digits) = if let Some(h) = t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
        (16, h)
    } else if let Some(h) = t.strip_prefix('$') {
        (16, h)
    } else {
        (10, t)
    };
    u32::from_str_radix(digits, radix).map_err(|_| format!("`{s}` is not a number"))
}

fn as_usize(v: i64, what: &str) -> Result<usize, String> {
    usize::try_from(v).map_err(|_| format!("{what} cannot be negative, got {v}"))
}

fn as_u32(v: i64, what: &str) -> Result<u32, String> {
    u32::try_from(v).map_err(|_| format!("{what} is out of range, got {v}"))
}

fn edge(
    kind: &str,
    value: Option<i64>,
    bit: Option<u32>,
    what: &str,
) -> Result<Edge, String> {
    let need_value = || -> Result<u32, String> {
        let v = value.ok_or_else(|| format!("{what}: edge `{kind}` needs a `value`"))?;
        as_u32(v, "value")
    };
    let need_bit = || -> Result<u32, String> {
        bit.ok_or_else(|| format!("{what}: edge `{kind}` needs a `bit`"))
    };
    let e = match kind {
        "increases" => Edge::Increases,
        "decreases" => Edge::Decreases,
        "changes" => Edge::Changes,
        "enters" => Edge::Enters(need_value()?),
        "leaves" => Edge::Leaves(need_value()?),
        "bit_set" => Edge::BitSet(need_bit()?),
        "bit_cleared" => Edge::BitCleared(need_bit()?),
        "bits_set" => Edge::BitsSet,
        other => {
            return Err(format!(
                "{what}: unknown edge `{other}`; try increases, decreases, changes, \
                 enters, leaves, bit_set, bit_cleared, bits_set"
            ))
        }
    };
    // A value or a bit that the edge does not read is a sign the author meant a
    // different edge, so it is refused rather than ignored.
    let uses_value = matches!(e, Edge::Enters(_) | Edge::Leaves(_));
    let uses_bit = matches!(e, Edge::BitSet(_) | Edge::BitCleared(_));
    if value.is_some() && !uses_value {
        return Err(format!("{what}: edge `{kind}` does not take a `value`"));
    }
    if bit.is_some() && !uses_bit {
        return Err(format!("{what}: edge `{kind}` does not take a `bit`"));
    }
    Ok(e)
}

fn test(d: &TestDoc, what: &str) -> Result<Test, String> {
    let kind = match d.test.as_str() {
        "eq" => TestKind::Eq(as_u32(
            d.value.ok_or_else(|| format!("{what}: test `eq` needs a `value`"))?,
            "value",
        )?),
        "ne" => TestKind::Ne(as_u32(
            d.value.ok_or_else(|| format!("{what}: test `ne` needs a `value`"))?,
            "value",
        )?),
        "bit_set" => TestKind::BitSet(
            d.bit.ok_or_else(|| format!("{what}: test `bit_set` needs a `bit`"))?,
        ),
        "bit_cleared" => TestKind::BitCleared(
            d.bit.ok_or_else(|| format!("{what}: test `bit_cleared` needs a `bit`"))?,
        ),
        other => {
            return Err(format!(
                "{what}: unknown test `{other}`; try eq, ne, bit_set, bit_cleared"
            ))
        }
    };
    Ok(Test { field: d.field.clone(), kind })
}

impl Doc {
    /// `library` is the challenge half of the same text, already validated —
    /// including the schema version.
    fn into_manifest(self, source: &Path, library: Library) -> Result<Manifest, String> {
        if self.runner.kind != "libretro" {
            return Err(format!(
                "runner.kind is `{}`; this crate only runs `libretro` games",
                self.runner.kind
            ));
        }
        let lr = self
            .runner
            .libretro
            .ok_or("runner.kind is `libretro` but there is no [runner.libretro] table")?;

        let default_width = match lr.memory.default_width.as_deref() {
            Some(w) => Width::parse(w)?,
            None => Width::U8,
        };
        let space = AddressSpace {
            base: as_usize(lr.memory.base, "runner.libretro.memory.base")?,
            size: as_usize(lr.memory.size, "runner.libretro.memory.size")?,
            prefer: match lr.memory.prefer.as_str() {
                "system_ram" => Prefer::SystemRam,
                "memory_map" => Prefer::MemoryMap,
                other => {
                    return Err(format!(
                        "runner.libretro.memory.prefer is `{other}`; try system_ram or memory_map"
                    ))
                }
            },
            endian: match lr.memory.endian.as_deref() {
                None | Some("little") => Endian::Little,
                Some("big") => Endian::Big,
                Some(other) => {
                    return Err(format!(
                        "runner.libretro.memory.endian is `{other}`; try little or big"
                    ))
                }
            },
        };
        let pad_layout = match lr.pad_layout.as_deref() {
            None | Some("snes") => PadLayout::Snes,
            Some("nes") => PadLayout::Nes,
            Some(other) => {
                return Err(format!("runner.libretro.pad_layout is `{other}`; try nes or snes"))
            }
        };
        let controller = match lr.controller.as_deref() {
            None | Some("pad") => Controller::Pad(pad_layout),
            Some("keyboard") => {
                // Refused rather than ignored: a pad layout on a keyboard game
                // is a contributor who believes something about the input that
                // is not true, and a silently discarded key is how a manifest
                // ends up describing a game nobody is playing.
                if lr.pad_layout.is_some() {
                    return Err("runner.libretro.pad_layout means nothing when \
                                controller = \"keyboard\": the host keyboard passes \
                                straight through, and the game maps its own keys"
                        .into());
                }
                Controller::Keyboard
            }
            Some(other) => {
                return Err(format!(
                    "runner.libretro.controller is `{other}`; try pad or keyboard"
                ))
            }
        };
        let scan = match &lr.memory.scan {
            None => None,
            Some(sc) => {
                let at = as_usize(sc.at, "runner.libretro.memory.scan.at")?;
                let size = as_usize(sc.size, "runner.libretro.memory.scan.size")?;
                if size == 0 {
                    return Err("runner.libretro.memory.scan.size is 0, so there is \
                                nothing to search"
                        .into());
                }
                Some(ScanWindow { at, size })
            }
        };

        let mut watches = Vec::new();
        for (name, w) in &self.watch {
            let at = match w.at {
                Some(a) => Some(as_usize(a, &format!("watch.{name}.at"))?),
                None => None,
            };
            watches.push(WatchDef {
                name: name.clone(),
                at,
                width: match w.width.as_deref() {
                    Some(x) => Width::parse(x)?,
                    None => default_width,
                },
                note: w.note.clone().unwrap_or_default(),
            });
        }

        let known = |f: &str, what: &str| -> Result<(), String> {
            if watches.iter().any(|w| w.name == f) {
                Ok(())
            } else {
                Err(format!("{what}: no [watch.{f}] is declared"))
            }
        };

        let mut labels = BTreeMap::new();
        for (field, table) in &self.label {
            known(field, &format!("[label.{field}]"))?;
            let mut out = BTreeMap::new();
            for (k, v) in table {
                out.insert(
                    parse_int(k).map_err(|e| format!("[label.{field}]: {e}"))?,
                    v.clone(),
                );
            }
            labels.insert(field.clone(), out);
        }

        for f in &self.display.fields {
            known(f, "display.fields")?;
        }

        let mut markers = Vec::new();
        for m in &self.marker {
            let what = format!("marker `{}`", m.name);
            known(&m.field, &what)?;
            let mut captures = Vec::new();
            for c in &m.capture {
                known(&c.field, &format!("{what} capture `{}`", c.name))?;
                captures.push((c.name.clone(), test(&c.as_test(), &what)?));
            }
            markers.push(Marker {
                name: m.name.clone(),
                field: m.field.clone(),
                edge: edge(&m.edge, m.value, m.bit, &what)?,
                window_frames: m.window_frames,
                captures,
            });
        }

        let mut rules = Vec::new();
        for (i, r) in self.rule.iter().enumerate() {
            let what = match &r.name {
                Some(n) => format!("rule `{n}`"),
                None => format!("rule #{} (on `{}`)", i + 1, r.field),
            };
            known(&r.field, &what)?;
            // Parsed by the same function `challenge-library` already ran over
            // this rule, so it cannot fail here; it is called again rather than
            // carried across so that the challenge half holds no rule state.
            let emit = Emit::parse(
                &r.emit,
                r.reason.as_deref(),
                r.item.as_deref(),
                r.objective.as_deref(),
                &what,
            )?;
            let guard = match &r.guard {
                Some(g) => {
                    known(&g.field, &format!("{what} guard"))?;
                    Some(test(g, &what)?)
                }
                None => None,
            };
            if let Some(m) = &r.timed_to {
                if !markers.iter().any(|x| &x.name == m) {
                    return Err(format!("{what}: timed_to `{m}` names no [[marker]]"));
                }
            }
            if let Some(c) = &r.if_marked {
                let m = r.timed_to.as_ref().ok_or_else(|| {
                    format!("{what}: if_marked needs a timed_to naming the marker it reads")
                })?;
                let marker = markers.iter().find(|x| &x.name == m).unwrap();
                if !marker.captures.iter().any(|(n, _)| n == c) {
                    return Err(format!("{what}: marker `{m}` captures no `{c}`"));
                }
            }
            for n in &r.unless {
                if !self.rule.iter().any(|x| x.name.as_deref() == Some(n.as_str())) {
                    return Err(format!("{what}: unless `{n}` names no rule"));
                }
            }
            rules.push(Rule {
                name: r.name.clone(),
                field: r.field.clone(),
                edge: edge(&r.edge, r.value, r.bit, &what)?,
                bit_labels: r.bit_labels.clone(),
                guard,
                unless: r.unless.clone(),
                timed_to: r.timed_to.clone(),
                if_marked: r.if_marked.clone(),
                emit,
                log: r.log.clone(),
            });
        }

        let mut reset_on = Vec::new();
        for t in &self.attempt.reset_on {
            known(&t.field, "attempt.reset_on")?;
            reset_on.push(Trigger {
                field: t.field.clone(),
                edge: edge(&t.edge, t.value, t.bit, "attempt.reset_on")?,
            });
        }

        let game = library.game;
        let framerate: Framerate = game.framerate;
        let boot_frames = match lr.boot_seconds {
            None => 0,
            Some(secs) if secs.is_finite() && secs >= 0.0 => framerate.frames(secs),
            Some(secs) => {
                return Err(format!(
                    "runner.libretro.boot_seconds must be a non-negative number of \
                     seconds, got {secs}"
                ))
            }
        };

        let roms = build_roms(&self.rom, &game.console)?;

        // The same `[[stage]]` array, read twice: `challenge-library` has the
        // ids and names, in order and without duplicates, and this has where
        // each one runs and how it is entered.
        let mut stages = Vec::new();
        for (named, s) in library.stages.into_iter().zip(&self.stage) {
            let what = format!("stage `{}`", named.id.0);
            let entry = match (&s.entry.state, &s.entry.named) {
                (Some(p), None) => EntryPoint::State(AssetKey(p.clone())),
                (None, Some(n)) => EntryPoint::Named(n.clone()),
                _ => {
                    return Err(format!("{what}: entry needs exactly one of `state` or `named`"))
                }
            };
            if !roms.iter().any(|r| r.id == s.rom) {
                return Err(format!(
                    "{what}: runs on rom `{}`, which this manifest does not declare; it has {}",
                    s.rom,
                    roms.iter().map(|r| r.id.as_str()).collect::<Vec<_>>().join(", ")
                ));
            }
            stages.push(Stage {
                id: named.id,
                name: named.name,
                rom: s.rom.clone(),
                entry,
                note: s.note.clone(),
            });
        }

        // The one part of a challenge that is the adapter's: how a generated
        // one's ROM is derived. `challenge-library` has already checked that a
        // table and a version come together; building the table needs
        // `content-gen`, which the challenge half must never link.
        let mut patch_tables = BTreeMap::new();
        for (c, g) in library.challenges.iter().zip(&self.challenge) {
            let Some(version) = c.generator_version else { continue };
            let what = format!("challenge `{}`", c.id.0);
            // The console decides the header a slot's offset is counted from
            // and the checksum a substitution has to repair — and for a DOS
            // bundle it decides that there is no such thing.
            let console = Console::parse(&game.console).map_err(|e| format!("{what}: {e}"))?;
            console.is_derivable().map_err(|e| format!("{what}: {e}"))?;
            let table = PatchTable::build(&g.id, version, &g.patch_slot, &g.group)
                .map_err(|e| format!("{what}: {e}"))?;
            patch_tables.insert(c.id.clone(), table);
        }

        Ok(Manifest {
            id: game.id,
            title: game.title,
            console: game.console,
            region: game.region,
            framerate,
            roms,
            libretro: LibretroConfig {
                core_dir: PathBuf::from(lr.core_dir),
                controller,
                space,
                default_width,
                system_ram_bytes: match lr.memory.system_ram_bytes {
                    Some(v) => Some(as_usize(v, "runner.libretro.memory.system_ram_bytes")?),
                    None => None,
                },
                scan,
                options: lr.options,
                boot_frames,
            },
            watches,
            labels,
            display: self.display.fields,
            markers,
            rules,
            reset_on,
            stages,
            challenges: library.challenges,
            patch_tables,
            source: source.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
schema_version = 3
[game]
id = "t"
title = "Test"
console = "nes"
framerate = 60.0
[[rom]]
id = "base"
[runner]
kind = "libretro"
[runner.libretro]
core_dir = "cores"
[runner.libretro.memory]
base = 0x0000
size = 0x0800
prefer = "memory_map"
[watch.lives]
at = 0x0736
"#;

    fn parse(text: &str) -> Result<Manifest, String> {
        Manifest::parse(text, Path::new("manifests/test.toml"))
    }

    #[test]
    fn a_minimal_manifest_loads() {
        let m = parse(MINIMAL).unwrap();
        assert_eq!(m.id.0, "t");
        assert_eq!(m.watch("lives").unwrap().at, Some(0x0736));
        // No width given anywhere, so the console default applies.
        assert_eq!(m.watch("lives").unwrap().width, Width::U8);
    }

    /// A stage, and a rule that can clear it. The starting point for anything
    /// testing challenges.
    const CLEARABLE: &str = r#"
[[rule]]
field = "lives"
edge = "increases"
emit = "stage_cleared"
[[stage]]
id = "s"
rom = "base"
entry = { state = "a.state" }
"#;

    fn with_challenge(body: &str) -> Result<Manifest, String> {
        parse(&format!("{MINIMAL}{CLEARABLE}\n[[challenge]]\nstage = \"s\"\n{body}"))
    }

    #[test]
    fn a_future_schema_version_is_refused_rather_than_half_read() {
        let text = MINIMAL.replace("schema_version = 3", "schema_version = 4");
        assert!(parse(&text).unwrap_err().contains("schema_version"));
    }

    /// Schema 1 put `challenges` and `endurance` on the stage. Both are gone,
    /// and a manifest still carrying them says so at the version rather than
    /// dying on an unknown key.
    #[test]
    fn a_schema_1_manifest_is_refused_at_the_version() {
        let text = MINIMAL.replace("schema_version = 3", "schema_version = 1");
        assert!(parse(&text).unwrap_err().contains("schema_version"));
    }

    /// The failure this strictness exists to prevent: a mistyped key that
    /// leaves a detector silently switched off.
    #[test]
    fn an_unknown_key_is_an_error_not_a_silently_ignored_line() {
        let text = format!("{MINIMAL}\n[watch.bananas]\nadr = 0x057B\n");
        assert!(parse(&text).is_err());
    }

    // ------------------------------------------------ the DOS-shaped fields

    /// Refused rather than ignored. A pad layout on a keyboard game is a
    /// contributor who believes something about the input that is not true,
    /// and a silently discarded key is how a manifest ends up describing a
    /// game nobody is playing.
    #[test]
    fn a_pad_layout_on_a_keyboard_game_is_refused() {
        let text = MINIMAL.replace(
            "core_dir = \"cores\"",
            "core_dir = \"cores\"\ncontroller = \"keyboard\"\npad_layout = \"nes\"",
        );
        assert!(parse(&text).unwrap_err().contains("pad_layout"));
    }

    #[test]
    fn a_keyboard_game_needs_no_layout_at_all() {
        let text = MINIMAL.replace("core_dir = \"cores\"", "core_dir = \"cores\"\ncontroller = \"keyboard\"");
        assert_eq!(parse(&text).unwrap().libretro.controller, Controller::Keyboard);
    }

    #[test]
    fn an_unknown_controller_is_refused() {
        let text = MINIMAL.replace("core_dir = \"cores\"", "core_dir = \"cores\"\ncontroller = \"mouse\"");
        assert!(parse(&text).unwrap_err().contains("controller"));
    }

    /// A console manifest says none of this and gets the console behaviour,
    /// which is what stops the DOS work leaking into the other two games.
    #[test]
    fn omitting_the_dos_fields_leaves_a_console_manifest_alone() {
        let m = parse(MINIMAL).unwrap();
        assert_eq!(m.libretro.controller, Controller::Pad(PadLayout::Snes));
        assert_eq!(m.libretro.scan, None);
        assert_eq!(m.libretro.boot_frames, 0);
        assert!(m.libretro.options.is_empty());
    }

    /// Authored in seconds, stored in frames — like every other duration in
    /// the schema, so that seconds never reach a `SessionEvent` (ADR 0005).
    #[test]
    fn boot_seconds_become_frames_at_the_game_rate() {
        let text = MINIMAL.replace("core_dir = \"cores\"", "core_dir = \"cores\"\nboot_seconds = 16.0");
        assert_eq!(parse(&text).unwrap().libretro.boot_frames, 960);
    }

    #[test]
    fn a_negative_boot_time_is_refused() {
        let text = MINIMAL.replace("core_dir = \"cores\"", "core_dir = \"cores\"\nboot_seconds = -1.0");
        assert!(parse(&text).unwrap_err().contains("boot_seconds"));
    }

    /// A window of nothing would send the discovery tools searching an empty
    /// slice and report no candidates, which looks exactly like a game that
    /// writes nothing.
    #[test]
    fn an_empty_scan_window_is_refused() {
        let text = format!("{MINIMAL}\n[runner.libretro.memory.scan]\nat = 0x380000\nsize = 0\n");
        assert!(parse(&text).unwrap_err().contains("scan.size"));
    }

    #[test]
    fn a_scan_window_is_read_as_an_address_and_a_length() {
        let text =
            format!("{MINIMAL}\n[runner.libretro.memory.scan]\nat = 0x380000\nsize = 0x10000\n");
        let scan = parse(&text).unwrap().libretro.scan.expect("declared");
        assert_eq!((scan.at, scan.size), (0x38_0000, 0x1_0000));
    }

    /// Pinned options are carried through verbatim. What they are *worth* is
    /// checked against the core at startup, because only the core knows which
    /// keys exist — see `libretro::check_options`.
    #[test]
    fn pinned_options_survive_the_load() {
        let text = format!(
            "{MINIMAL}\n[runner.libretro.options]\ndosbox_pure_cycles = \"26800\"\n"
        );
        let m = parse(&text).unwrap();
        assert_eq!(m.libretro.options.get("dosbox_pure_cycles").map(String::as_str), Some("26800"));
    }

    /// `u32` is what a DOS game's C `int`s need, and it is the only width
    /// wider than 16 bits the library uses.
    #[test]
    fn a_watch_may_be_four_bytes_wide() {
        let text = MINIMAL.replace("at = 0x0736", "at = 0x0736\nwidth = \"u32\"");
        assert_eq!(parse(&text).unwrap().watch("lives").unwrap().width, Width::U32);
    }

    #[test]
    fn a_rule_naming_an_undeclared_field_is_refused() {
        let text = format!(
            "{MINIMAL}\n[[rule]]\nfield = \"livess\"\nedge = \"decreases\"\nemit = \"died\"\n"
        );
        assert!(parse(&text).unwrap_err().contains("no [watch.livess]"));
    }

    #[test]
    fn an_edge_that_needs_a_value_says_so() {
        let text = format!(
            "{MINIMAL}\n[[rule]]\nfield = \"lives\"\nedge = \"enters\"\nemit = \"died\"\n"
        );
        assert!(parse(&text).unwrap_err().contains("needs a `value`"));
    }

    /// A `value` on an edge that never reads one is a sign the author meant a
    /// different edge.
    #[test]
    fn a_value_on_an_edge_that_ignores_it_is_refused() {
        let text = format!(
            "{MINIMAL}\n[[rule]]\nfield = \"lives\"\nedge = \"decreases\"\nvalue = 3\nemit = \"died\"\n"
        );
        assert!(parse(&text).unwrap_err().contains("does not take a `value`"));
    }

    #[test]
    fn timed_to_must_name_a_marker_that_exists() {
        let text = format!(
            "{MINIMAL}\n[[rule]]\nfield = \"lives\"\nedge = \"decreases\"\n\
             timed_to = \"fade\"\nemit = \"died\"\n"
        );
        assert!(parse(&text).unwrap_err().contains("names no [[marker]]"));
    }

    #[test]
    fn an_entry_needs_exactly_one_kind() {
        let text =
            format!("{MINIMAL}\n[[stage]]\nid = \"s\"\nrom = \"base\"\nentry = {{ }}\n");
        assert!(parse(&text).unwrap_err().contains("exactly one"));
    }

    // ------------------------------------------------------------ challenges

    /// The challenge half is `challenge-library`'s and is tested there. What
    /// matters here is that its refusals still stop this loader, and that what
    /// it returns is what the manifest carries.
    #[test]
    fn the_challenge_half_is_read_by_challenge_library() {
        let ok = with_challenge(
            "id = \"c\"
kind = \"survival\"
complete_when = { survive_seconds = 30 }
",
        )
        .unwrap();
        assert_eq!(ok.challenge("c").unwrap().complete_when.survive_frames, Some(1800));
        assert_eq!(ok.challenge("c").unwrap().name, "s");

        let e = with_challenge("id = \"c\"
kind = \"survival\"
complete_when = { }
")
            .unwrap_err();
        assert!(e.contains("no criteria"), "{e}");
    }

    /// Both halves read one `[[stage]]` array; the names come from one and the
    /// entry points from the other, and they must not come apart.
    #[test]
    fn a_stage_keeps_its_name_and_its_entry() {
        let text = format!("{MINIMAL}{CLEARABLE}name = \"The Stage\"
");
        let m = parse(&text).unwrap();
        let s = m.stage("s").unwrap();
        assert_eq!(s.name, "The Stage");
        assert_eq!(s.entry, EntryPoint::State(AssetKey("a.state".into())));
        assert_eq!(s.rom, "base");
    }

    // -------------------------------------------------- generated challenges

    /// A patch table reaches `content-gen` intact — including the duplicates in
    /// its pool, which are the weighting and are not a mistake
    /// (`docs/games/smb3.md`).
    #[test]
    fn a_generated_challenge_carries_its_patch_table() {
        let text = format!(
            "{MINIMAL}{CLEARABLE}\
             [[challenge]]\nid = \"reroll\"\nstage = \"s\"\nkind = \"time_trial\"\n\
             complete_when = {{ cleared = true }}\ngenerator_version = 2\n\
             [[challenge.patch_slot]]\noffset = 0xC6BD\nwidth = \"u8\"\n\
             values = {{ pool = [0x72, 0x72, 0x81] }}\n"
        );
        let m = parse(&text).unwrap();
        let t = m.patch_table(&ChallengeId("reroll".into())).unwrap();
        assert_eq!(t.generator_id, "reroll");
        assert_eq!(t.generator_version, 2);
        assert_eq!(t.slots.len(), 1);
        assert_eq!(t.slots[0].offset, 0xC6BD);
        match &t.slots[0].values {
            content_gen::ValueSource::Pool(p) => assert_eq!(p, &[0x72, 0x72, 0x81]),
            other => panic!("{other:?}"),
        }
    }

    /// An ordinary challenge has no table at all, and that is the common case.
    #[test]
    fn a_vanilla_challenge_has_no_patch_table() {
        let m = with_challenge("id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleared = true }\n")
            .unwrap();
        assert!(m.patch_table(&ChallengeId("c".into())).is_none());
    }

    /// The engine's own validation is reached rather than duplicated here: a
    /// value that does not fit its slot is refused by `content-gen`, and the
    /// loader only says which challenge it was in.
    #[test]
    fn the_engines_validation_is_reported_against_the_challenge() {
        let text = format!(
            "{MINIMAL}{CLEARABLE}\
             [[challenge]]\nid = \"reroll\"\nstage = \"s\"\nkind = \"time_trial\"\n\
             complete_when = {{ cleared = true }}\ngenerator_version = 1\n\
             [[challenge.patch_slot]]\noffset = 0x10\nwidth = \"u8\"\n\
             values = {{ pool = [0x100] }}\n"
        );
        let e = parse(&text).unwrap_err();
        assert!(e.contains("challenge `reroll`"), "{e}");
        assert!(e.contains("does not fit in a u8"), "{e}");
    }

    /// A DOS bundle is a zip, and there is nothing to substitute a value into.
    /// Refused by name rather than by producing a corrupt archive (ADR 0020).
    #[test]
    fn a_generated_challenge_on_a_dos_game_is_refused() {
        let text = format!("{MINIMAL}{CLEARABLE}")
            .replace("console = \"nes\"", "console = \"dos\"")
            + "[[challenge]]\nid = \"reroll\"\nstage = \"s\"\nkind = \"time_trial\"\n\
               complete_when = { cleared = true }\ngenerator_version = 1\n\
               [[challenge.patch_slot]]\noffset = 0x10\nwidth = \"u8\"\n\
               values = { pool = [1] }\n";
        let e = parse(&text).unwrap_err();
        assert!(e.contains("member-level overlay"), "{e}");
    }

    // ------------------------------------------------------ alternative ROMs

    const HACKED: &str = r#"
[[rom]]
id = "hack"
name = "A Hack Of It"
from = "base"
patch = "a.bps"
sha1 = "55c34fe15e7753ee6781f7f5de477f5610fc3428"
"#;

    #[test]
    fn a_hack_is_a_patch_against_the_base_and_a_stage_names_it() {
        let text = format!(
            "{MINIMAL}{HACKED}\n[[stage]]\nid = \"s\"\nrom = \"hack\"\n\
             entry = {{ state = \"a.state\" }}\n"
        );
        let m = parse(&text).unwrap();
        assert_eq!(m.roms.len(), 2);
        assert_eq!(m.base_rom().id, "base");
        assert_eq!(m.stage("s").unwrap().rom, "hack");
        // A hack has its own name, and it is what a player is shown — ADR 0020
        // accepted the base game's *labels* leaking, not its title.
        assert_eq!(m.content_title(m.stage("s").unwrap()), "A Hack Of It");
    }

    #[test]
    fn a_stage_naming_an_undeclared_rom_is_refused() {
        let text = format!(
            "{MINIMAL}\n[[stage]]\nid = \"s\"\nrom = \"hack\"\n\
             entry = {{ state = \"a.state\" }}\n"
        );
        let e = parse(&text).unwrap_err();
        assert!(e.contains("does not declare"), "{e}");
    }

    /// One level of derivation, so that content stays two deep and a patch is
    /// always against the dump the player actually owns.
    #[test]
    fn a_patch_against_a_patch_is_refused() {
        let text = format!(
            "{MINIMAL}{HACKED}\n[[rom]]\nid = \"hack2\"\nfrom = \"hack\"\n\
             patch = \"b.bps\"\nsha1 = \"abc\"\n"
        );
        let e = parse(&text).unwrap_err();
        assert!(e.contains("not the base"), "{e}");
    }

    #[test]
    fn two_roms_without_a_from_leave_no_base() {
        let text = format!("{MINIMAL}\n[[rom]]\nid = \"other\"\npath = \"b.nes\"\n");
        let e = parse(&text).unwrap_err();
        assert!(e.contains("no single base"), "{e}");
    }

    /// The rule that keeps a derivative of a copyrighted dump out of the
    /// manifest: a hack is a patch, never a file.
    #[test]
    fn a_path_on_a_derived_rom_is_refused() {
        let text = format!("{MINIMAL}{HACKED}").replace("patch = \"a.bps\"", "path = \"h.nes\"");
        let e = parse(&text).unwrap_err();
        assert!(e.contains("no `patch`"), "{e}");

        let text = format!("{MINIMAL}{HACKED}\npath = \"h.nes\"\n");
        let e = parse(&text).unwrap_err();
        assert!(e.contains("distributing a derivative"), "{e}");
    }

    #[test]
    fn a_derived_rom_needs_the_hash_of_what_it_produces() {
        let text = format!("{MINIMAL}{HACKED}")
            .replace("sha1 = \"55c34fe15e7753ee6781f7f5de477f5610fc3428\"\n", "");
        let e = parse(&text).unwrap_err();
        assert!(e.contains("sha1"), "{e}");
    }

    /// `header` and `min_bytes` recognise a *dump*. A hack is derived from one
    /// that was already recognised, and may legitimately be a different size.
    #[test]
    fn dump_checks_on_a_derived_rom_are_refused_rather_than_ignored() {
        let text = format!("{MINIMAL}{HACKED}\nheader = \"ines\"\n");
        let e = parse(&text).unwrap_err();
        assert!(e.contains("base ROM only"), "{e}");
    }

    /// A BPS patch against a zip is meaningless — the bundle's hash depends on
    /// how it was zipped — so a DOS alternative waits on member hashing.
    #[test]
    fn a_hack_of_a_dos_game_is_refused_with_its_reason() {
        let text =
            format!("{MINIMAL}{HACKED}").replace("console = \"nes\"", "console = \"dos\"");
        let e = parse(&text).unwrap_err();
        assert!(e.contains("member-level overlay"), "{e}");
    }

    #[test]
    fn an_address_override_retargets_a_watch() {
        let mut m = parse(MINIMAL).unwrap();
        m.set_addr("lives", Some(0x0575), Some(Width::U16)).unwrap();
        assert_eq!(m.watch("lives").unwrap().at, Some(0x0575));
        assert_eq!(m.watch("lives").unwrap().width, Width::U16);
        m.set_addr("lives", None, None).unwrap();
        assert_eq!(m.watch("lives").unwrap().at, None);
        assert!(m.set_addr("livess", Some(1), None).is_err());
    }

    #[test]
    fn label_keys_may_be_written_in_hex() {
        let text = format!("{MINIMAL}\n[label.lives]\n0x03 = \"three\"\n4 = \"four\"\n");
        let m = parse(&text).unwrap();
        let l = &m.labels["lives"];
        assert_eq!(l[&3], "three");
        assert_eq!(l[&4], "four");
    }
}
