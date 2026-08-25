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

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use session_api::{AssetKey, ChallengeKind, EntryPoint, Framerate, GameId, StageId, StageSpec};

use crate::memory::{AddressSpace, Endian, Prefer, Width};

/// The only schema version this build understands. A manifest states its own,
/// and one from the future is refused rather than half-read.
pub const SCHEMA_VERSION: u32 = 1;

// ------------------------------------------------------------- raw documents

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct Doc {
    schema_version: u32,
    game: GameDoc,
    rom: RomDoc,
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
    /// Declared so the error can say what is actually going on. ADR 0015
    /// settled the shape of a generated challenge's patch slots, and
    /// `content-gen` will read them — but nothing in this crate does, and a
    /// half-implemented path is worse than an honest refusal.
    #[serde(default)]
    challenge: Vec<toml::Value>,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct GameDoc {
    id: String,
    title: String,
    console: String,
    #[serde(default)]
    region: Option<String>,
    framerate: f64,
}

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct RomDoc {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    sha1: Option<String>,
    #[serde(default)]
    path: Option<String>,
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
    pad_layout: Option<String>,
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

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct StageDoc {
    id: String,
    #[serde(default)]
    name: Option<String>,
    entry: EntryDoc,
    #[serde(default)]
    challenges: Vec<String>,
    #[serde(default)]
    endurance: Option<EnduranceDoc>,
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

#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct EnduranceDoc {
    /// Authored in seconds, converted to frames at load. Seconds never leave
    /// this module.
    survive_seconds: f64,
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

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Emit {
    StageCleared,
    StageExited(session_api::ExitReason),
    Died,
    Damaged,
    EnemyDefeated,
    Item(String),
    /// Logged, and nothing more. For a signal worth watching during a playtest
    /// that is not yet known to mean anything — a powerup change, a pipe
    /// transit — where inventing a `SessionEvent` for it would be a guess.
    Note,
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

#[derive(Clone, Debug)]
pub struct Stage {
    pub id: StageId,
    pub name: String,
    pub entry: EntryPoint,
    pub challenges: Vec<ChallengeKind>,
    /// Already in frames. A manifest may author it in seconds; the conversion
    /// happens here so that seconds never reach a `SessionEvent` or the wire.
    pub survive_frames: Option<u64>,
    pub note: Option<String>,
}

/// What the libretro adapter needs that a Doom adapter would not have.
#[derive(Clone, Debug)]
pub struct LibretroConfig {
    pub core_dir: PathBuf,
    pub pad_layout: PadLayout,
    pub space: AddressSpace,
    pub default_width: Width,
    /// What `RETRO_MEMORY_SYSTEM_RAM` is expected to be. A core handing back
    /// something else is not wrong, but every address in the manifest was
    /// written against the expected one, so startup says so.
    pub system_ram_bytes: Option<usize>,
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

#[derive(Clone, Debug)]
pub struct RomSpec {
    pub name: Option<String>,
    pub sha1: Option<String>,
    pub path: Option<PathBuf>,
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
    pub rom: RomSpec,
    pub libretro: LibretroConfig,
    pub watches: Vec<WatchDef>,
    pub labels: BTreeMap<String, BTreeMap<u32, String>>,
    pub display: Vec<String>,
    pub markers: Vec<Marker>,
    pub rules: Vec<Rule>,
    pub reset_on: Vec<Trigger>,
    pub stages: Vec<Stage>,
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

    pub fn parse(text: &str, source: &Path) -> Result<Manifest, String> {
        let doc: Doc = toml::from_str(text).map_err(|e| e.to_string())?;
        doc.into_manifest(source)
    }

    pub fn watch(&self, name: &str) -> Option<&WatchDef> {
        self.watches.iter().find(|w| w.name == name)
    }

    pub fn stage(&self, id: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id.0 == id)
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
                // than silently pointing a detector at nothing.
                if a > 0xFF_FFFF {
                    return Err(format!("{} is not an address this console has", space.show(a)));
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
    pub fn stage_spec(&self, stage: &Stage) -> StageSpec {
        let entry = match &stage.entry {
            EntryPoint::State(key) => EntryPoint::State(AssetKey(self.resolve(&key.0))),
            other => other.clone(),
        };
        StageSpec { game: self.id.clone(), stage: stage.id.clone(), entry }
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
    fn into_manifest(self, source: &Path) -> Result<Manifest, String> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "schema_version {} is not {SCHEMA_VERSION}, which is what this build reads",
                self.schema_version
            ));
        }
        if !self.challenge.is_empty() {
            return Err(
                "[[challenge]] declares a generated challenge's patch slots (ADR 0015). \
                 The shape is documented in docs/manifest-schema.md, but no crate reads it \
                 yet — `content-gen` will. Remove it until then."
                    .to_string(),
            );
        }
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
            let emit = match r.emit.as_str() {
                "stage_cleared" => Emit::StageCleared,
                "stage_exited" => Emit::StageExited(match r.reason.as_deref() {
                    Some("death") => session_api::ExitReason::Death,
                    Some("quit") => session_api::ExitReason::Quit,
                    None | Some("unknown") => session_api::ExitReason::Unknown,
                    Some(other) => {
                        return Err(format!(
                            "{what}: reason `{other}`; try death, quit or unknown"
                        ))
                    }
                }),
                "died" => Emit::Died,
                "damaged" => Emit::Damaged,
                "enemy_defeated" => Emit::EnemyDefeated,
                "item" => Emit::Item(
                    r.item
                        .clone()
                        .ok_or_else(|| format!("{what}: emit `item` needs an `item` name"))?,
                ),
                "note" => Emit::Note,
                other => {
                    return Err(format!(
                        "{what}: unknown emit `{other}`; try stage_cleared, stage_exited, \
                         died, damaged, enemy_defeated, item, note"
                    ))
                }
            };
            if r.reason.is_some() && !matches!(emit, Emit::StageExited(_)) {
                return Err(format!("{what}: only `stage_exited` takes a `reason`"));
            }
            if r.item.is_some() && !matches!(emit, Emit::Item(_)) {
                return Err(format!("{what}: only `item` takes an `item` name"));
            }
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

        let framerate = Framerate(self.game.framerate);
        if !(1.0..=1000.0).contains(&self.game.framerate) {
            return Err(format!(
                "game.framerate {} is not a plausible rate",
                self.game.framerate
            ));
        }

        let mut stages = Vec::new();
        for s in &self.stage {
            let what = format!("stage `{}`", s.id);
            let entry = match (&s.entry.state, &s.entry.named) {
                (Some(p), None) => EntryPoint::State(AssetKey(p.clone())),
                (None, Some(n)) => EntryPoint::Named(n.clone()),
                _ => {
                    return Err(format!("{what}: entry needs exactly one of `state` or `named`"))
                }
            };
            let mut challenges = Vec::new();
            for c in &s.challenges {
                challenges.push(match c.as_str() {
                    "time_trial" => ChallengeKind::TimeTrial,
                    "endurance" => ChallengeKind::Endurance,
                    other => {
                        return Err(format!(
                            "{what}: challenge `{other}`; try time_trial or endurance"
                        ))
                    }
                });
            }
            if let Some(e) = &s.endurance {
                if !challenges.contains(&ChallengeKind::Endurance) {
                    return Err(format!(
                        "{what}: has an [endurance] block but does not list `endurance` \
                         among its challenges"
                    ));
                }
                let _ = e;
            }
            if stages.iter().any(|x: &Stage| x.id.0 == s.id) {
                return Err(format!("{what}: declared twice"));
            }
            stages.push(Stage {
                id: StageId(s.id.clone()),
                name: s.name.clone().unwrap_or_else(|| s.id.clone()),
                entry,
                challenges,
                // Seconds in, frames out, once, here.
                survive_frames: s.endurance.as_ref().map(|e| framerate.frames(e.survive_seconds)),
                note: s.note.clone(),
            });
        }

        Ok(Manifest {
            id: GameId(self.game.id),
            title: self.game.title,
            console: self.game.console,
            region: self.game.region,
            framerate,
            rom: RomSpec {
                name: self.rom.name,
                sha1: self.rom.sha1,
                path: self.rom.path.map(PathBuf::from),
                header: match self.rom.header.as_deref() {
                    None | Some("none") => RomHeader::None,
                    Some("ines") => RomHeader::INes,
                    Some("copier-optional") => RomHeader::CopierOptional,
                    Some(other) => {
                        return Err(format!(
                            "rom.header is `{other}`; try none, ines or copier-optional"
                        ))
                    }
                },
                min_bytes: match self.rom.min_bytes {
                    Some(v) => as_usize(v, "rom.min_bytes")?,
                    None => 0,
                },
            },
            libretro: LibretroConfig {
                core_dir: PathBuf::from(lr.core_dir),
                pad_layout,
                space,
                default_width,
                system_ram_bytes: match lr.memory.system_ram_bytes {
                    Some(v) => Some(as_usize(v, "runner.libretro.memory.system_ram_bytes")?),
                    None => None,
                },
            },
            watches,
            labels,
            display: self.display.fields,
            markers,
            rules,
            reset_on,
            stages,
            source: source.to_path_buf(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
schema_version = 1
[game]
id = "t"
title = "Test"
console = "nes"
framerate = 60.0
[rom]
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

    #[test]
    fn a_future_schema_version_is_refused_rather_than_half_read() {
        let text = MINIMAL.replace("schema_version = 1", "schema_version = 2");
        assert!(parse(&text).unwrap_err().contains("schema_version"));
    }

    /// The failure this strictness exists to prevent: a mistyped key that
    /// leaves a detector silently switched off.
    #[test]
    fn an_unknown_key_is_an_error_not_a_silently_ignored_line() {
        let text = format!("{MINIMAL}\n[watch.bananas]\nadr = 0x057B\n");
        assert!(parse(&text).is_err());
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
    fn durations_authored_in_seconds_are_stored_as_frames() {
        let text = format!(
            "{MINIMAL}\n[[stage]]\nid = \"s\"\nentry = {{ state = \"a.state\" }}\n\
             challenges = [\"endurance\"]\nendurance = {{ survive_seconds = 30 }}\n"
        );
        let m = parse(&text).unwrap();
        assert_eq!(m.stage("s").unwrap().survive_frames, Some(1800));
    }

    #[test]
    fn an_endurance_block_on_a_stage_that_is_not_an_endurance_stage_is_refused() {
        let text = format!(
            "{MINIMAL}\n[[stage]]\nid = \"s\"\nentry = {{ state = \"a.state\" }}\n\
             challenges = [\"time_trial\"]\nendurance = {{ survive_seconds = 30 }}\n"
        );
        assert!(parse(&text).is_err());
    }

    #[test]
    fn an_entry_needs_exactly_one_kind() {
        let text = format!("{MINIMAL}\n[[stage]]\nid = \"s\"\nentry = {{ }}\n");
        assert!(parse(&text).unwrap_err().contains("exactly one"));
    }

    /// ADR 0015 documented patch slots; nothing implements them. Saying so is
    /// better than accepting a table that will not do anything.
    #[test]
    fn patch_slots_are_refused_with_an_explanation() {
        let text = format!("{MINIMAL}\n[[challenge]]\nid = \"reroll\"\n");
        let e = parse(&text).unwrap_err();
        assert!(e.contains("ADR 0015"), "{e}");
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
