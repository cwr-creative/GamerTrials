//! The challenge half of a game manifest.
//!
//! A manifest has two readers. The adapter that runs the game needs its runner
//! half — the core, the memory map, `[watch]`, `[[rule]]`, entry states — and
//! `session-libretro` reads that. Whatever deals and judges needs the other
//! half: the game, its stages by name, and every `[[challenge]]` with the
//! criteria that complete it. That is this crate, and it is the only thing the
//! server reads from a manifest (ADR 0009, ADR 0024).
//!
//! Both halves are one file, one `schema_version` and one schema
//! (`docs/manifest-schema.md`); the split is in who reads what, not in what a
//! contributor writes. `session-libretro` parses through this crate first, so a
//! client and a server cannot load the same file into two different libraries.
//!
//! What is read here, and how strictly:
//!
//! * `schema_version` — checked before anything else is deserialized, so a
//!   manifest from another version is refused at the version rather than at
//!   whichever key moved.
//! * `[game]` and `[[challenge]]` — owned here, and every unknown key is
//!   refused, for the reason `session-libretro` gives: a mistyped key is a
//!   criterion silently switched off.
//! * `[[stage]]` — its `id`, `name` and `rom`. The entry point is the runner's,
//!   and the runner's loader is the strict one there.
//! * `[[rom]]` — each one's `id`, `name`, and whether it carries a `from`. That
//!   is what a lobby needs: whether a challenge runs on a hack, and what to
//!   call it (ADR 0020). The patch, the path, the hash and the dump checks are
//!   the runner's, and so is the rule that exactly one entry is the base — the
//!   six rules of ADR 0020 have one authority, and it is `session-libretro`.
//! * `[[rule]]` — only what each rule *emits*. A criterion naming a fact no
//!   rule announces is a challenge nobody can complete, and catching that needs
//!   the emit vocabulary but none of the addresses behind it.
//!
//! Anything else in the file is the runner's, and is not read at all.
//!
//! A [`Catalog`] is several libraries together — the whole of what a lobby
//! lists — with a hash over the texts they were read from, so two machines can
//! tell whether they hold the same library before a match starts.
//!
//! A generated challenge's `patch_slot` table is accepted and not interpreted.
//! It belongs to `content-gen`, which the server must never link; what a dealer
//! needs is that the challenge *is* generated, so it knows to issue a seed
//! (ADR 0013). The adapter builds the table itself.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use session_api::{
    ChallengeId, ChallengeKind, ChallengeSpec, Completion, ExitReason, Framerate, GameId, StageId,
    DAMAGED, ENEMY_DEFEATED,
};

/// The only schema version this build understands. A manifest states its own,
/// and one from the future is refused rather than half-read.
///
/// 2 was ADR 0017: `[[challenge]]` became the contributor-authored definition of
/// a challenge, and `[[stage]]` lost `challenges` and `endurance`.
///
/// 3 is ADR 0020 and ADR 0015, and it is the version at which content stopped
/// being a single file this manifest was written against. `[rom]` became an
/// array with an `id`, so a hand-authored hack can live beside the base ROM it
/// patches; `[[stage]]` gained a required `rom`, because an entry state is bound
/// to one piece of content's bytes; and `[[challenge]]` may now carry the
/// `patch_slot` table `content-gen` rolls from. The first two are a breaking
/// change to every manifest — twelve stages, one line each — which is why this
/// is a version rather than an addition.
pub const SCHEMA_VERSION: u32 = 3;

// ------------------------------------------------------------- raw documents

/// Read on its own, first, so that the version decides whether the rest is
/// worth deserializing.
#[derive(Deserialize)]
struct VersionDoc {
    schema_version: u32,
}

/// Deliberately not `deny_unknown_fields`: everything this struct does not name
/// is the runner half, and is someone else's to police.
#[derive(Deserialize, Debug)]
struct Doc {
    game: GameDoc,
    #[serde(default)]
    rom: Vec<RomDoc>,
    #[serde(default)]
    rule: Vec<RuleDoc>,
    #[serde(default)]
    stage: Vec<StageDoc>,
    #[serde(default)]
    challenge: Vec<ChallengeDoc>,
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
    /// Free text, lowercase by convention. What a lobby groups "by genre" on.
    #[serde(default)]
    genre: Option<String>,
    /// Year of release in the manifest's region. What a lobby groups "by era"
    /// on: the era is the decade.
    #[serde(default)]
    year: Option<u16>,
}

/// One piece of content, as far as a lobby is concerned: a name to show, and
/// whether it is the player's own dump or something derived from it. Not
/// `deny_unknown_fields`: `path`, `patch`, `sha1` and the dump checks are the
/// runner's, and this crate must not learn them (ADR 0020).
#[derive(Deserialize, Debug)]
struct RomDoc {
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// Its presence is what makes a ROM derived. What it names is the runner's
    /// to check.
    #[serde(default)]
    from: Option<String>,
}

/// The part of a rule that says what it announces. The field it watches and
/// the transition it fires on are the runner's.
#[derive(Deserialize, Debug)]
struct RuleDoc {
    #[serde(default)]
    name: Option<String>,
    field: String,
    edge: String,
    #[serde(default)]
    bit_labels: Vec<String>,
    emit: String,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    counter: Option<String>,
    #[serde(default)]
    objective: Option<String>,
}

/// A stage as a dealer sees it: something to name, on some content. How it is
/// entered is the runner's.
#[derive(Deserialize, Debug)]
struct StageDoc {
    id: String,
    #[serde(default)]
    name: Option<String>,
    /// Required on every stage, in every manifest (ADR 0020).
    rom: String,
}

/// What a player is actually dealt: one stage, one kind, and the criteria that
/// complete it. Contributors author every one they want to exist; nothing is
/// inferred from a stage (ADR 0017).
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct ChallengeDoc {
    id: String,
    #[serde(default)]
    name: Option<String>,
    stage: String,
    kind: String,
    complete_when: CompleteWhenDoc,
    /// Facts that end an attempt as a death does (ADR 0025).
    #[serde(default)]
    ends_attempt_on: Vec<String>,
    #[serde(default)]
    note: Option<String>,
    /// The goal in plain English, for the intro and the lobby. Composed from
    /// `complete_when` when absent.
    #[serde(default)]
    goal: Option<String>,
    /// The run's bound, authored in seconds and converted to frames at load
    /// like `survive_seconds`. Absent means the lobby's default applies.
    #[serde(default)]
    limit_seconds: Option<f64>,

    // ADR 0015's generated-challenge keys. The table's shape is `content-gen`'s
    // and is checked by the adapter that links it; here only their presence is
    // read, because the three go together: a table with no version cannot be
    // bumped without silently invalidating every artifact derived from it, and
    // a version with no table describes a generator that does nothing.
    #[serde(default)]
    patch_slot: Vec<toml::Value>,
    #[serde(default)]
    group: toml::Table,
    #[serde(default)]
    generator_version: Option<i64>,
}

/// A conjunction over the facts this manifest's own rules emit. Every criterion
/// present must hold; there is no `any`, no negation, no arithmetic.
#[derive(Deserialize, Debug)]
#[serde(deny_unknown_fields)]
struct CompleteWhenDoc {
    #[serde(default)]
    cleared: Option<bool>,
    /// Authored in seconds, converted to frames at load. Seconds never leave
    /// this crate.
    #[serde(default)]
    survive_seconds: Option<f64>,
    /// Spelling for `counts.enemy_defeated`.
    #[serde(default)]
    enemies_defeated: Option<i64>,
    /// Spelling for `counts` restricted to items, kept because it reads well
    /// and because its errors can list the items a manifest has.
    #[serde(default)]
    items_collected: BTreeMap<String, i64>,
    /// Any tally: an item, a counter, `damaged` or `enemy_defeated` (ADR 0025).
    #[serde(default)]
    counts: BTreeMap<String, i64>,
    #[serde(default)]
    objectives: Vec<String>,
}

// -------------------------------------------------------------- loaded shape

/// What a `[[rule]]` announces when it fires. The manifest's fact vocabulary:
/// `session-libretro`'s watcher turns each into a
/// [`SessionEvent`](session_api::SessionEvent), and this crate checks
/// `complete_when` against the set a manifest can produce.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Emit {
    StageCleared,
    StageExited(ExitReason),
    Died,
    Damaged,
    EnemyDefeated,
    Item(String),
    /// A named tally that is not an item — notes placed, laps, points. Counted
    /// exactly like an item; the difference is only what it is called
    /// (ADR 0025).
    Count(String),
    /// A named milestone the game announced — a boss dying, a gate opening.
    /// Still a fact: whether reaching it completes anything is a challenge's
    /// `complete_when`, not this rule's business.
    Objective(String),
    /// Logged, and nothing more. For a signal worth watching during a playtest
    /// that is not yet known to mean anything — a powerup change, a pipe
    /// transit — where inventing a `SessionEvent` for it would be a guess.
    Note,
}

impl Emit {
    /// A rule's `emit` and the keys that qualify it, as the manifest wrote
    /// them. The one place this vocabulary is parsed: the runner's loader calls
    /// it too, so a rule cannot mean one thing to a dealer and another to the
    /// watcher. `what` names the rule in errors.
    pub fn parse(
        emit: &str,
        reason: Option<&str>,
        item: Option<&str>,
        counter: Option<&str>,
        objective: Option<&str>,
        what: &str,
    ) -> Result<Emit, String> {
        let e = match emit {
            "stage_cleared" => Emit::StageCleared,
            "stage_exited" => Emit::StageExited(match reason {
                Some("death") => ExitReason::Death,
                Some("quit") => ExitReason::Quit,
                None | Some("unknown") => ExitReason::Unknown,
                Some(other) => {
                    return Err(format!("{what}: reason `{other}`; try death, quit or unknown"))
                }
            }),
            "died" => Emit::Died,
            "damaged" => Emit::Damaged,
            "enemy_defeated" => Emit::EnemyDefeated,
            "item" => Emit::Item(
                item.ok_or_else(|| format!("{what}: emit `item` needs an `item` name"))?
                    .to_string(),
            ),
            "count" => Emit::Count(
                counter
                    .ok_or_else(|| format!("{what}: emit `count` needs a `counter` name"))?
                    .to_string(),
            ),
            "objective" => Emit::Objective(
                objective
                    .ok_or_else(|| format!("{what}: emit `objective` needs an `objective` name"))?
                    .to_string(),
            ),
            "note" => Emit::Note,
            other => {
                return Err(format!(
                    "{what}: unknown emit `{other}`; try stage_cleared, stage_exited, \
                     died, damaged, enemy_defeated, item, count, objective, note"
                ))
            }
        };
        if counter.is_some() && !matches!(e, Emit::Count(_)) {
            return Err(format!("{what}: only `count` takes a `counter` name"));
        }
        if reason.is_some() && !matches!(e, Emit::StageExited(_)) {
            return Err(format!("{what}: only `stage_exited` takes a `reason`"));
        }
        if item.is_some() && !matches!(e, Emit::Item(_)) {
            return Err(format!("{what}: only `item` takes an `item` name"));
        }
        if objective.is_some() && !matches!(e, Emit::Objective(_)) {
            return Err(format!("{what}: only `objective` takes an `objective` name"));
        }
        Ok(e)
    }
}

/// The game a manifest describes.
#[derive(Clone, Debug)]
pub struct Game {
    pub id: GameId,
    pub title: String,
    /// The machine, as the manifest names it. A dealer has no use for it beyond
    /// display; the adapter keys its console family off it.
    pub console: String,
    pub region: Option<String>,
    pub framerate: Framerate,
    /// Free text, lowercase by convention (`platformer`, `fps`). A lobby
    /// groups on it; nothing else reads it.
    pub genre: Option<String>,
    /// Year of release in the manifest's region. Its decade is the game's era.
    pub year: Option<u16>,
}

/// One piece of content a stage runs on, as a lobby sees it: the player's own
/// dump, or a hack derived from it. What derives it is the runner's (ADR 0020).
#[derive(Clone, Debug)]
pub struct Rom {
    pub id: String,
    /// What a player is shown for a stage on this ROM, over the game's title.
    /// Defaults to the id.
    pub name: String,
    /// Declared with a `from`: a hand-authored hack rather than the base dump.
    /// A challenge on one is a custom challenge, as a generated one is.
    pub derived: bool,
}

/// A stage, as far as dealing is concerned: an id a challenge names, what to
/// call it, and which content it runs on.
#[derive(Clone, Debug)]
pub struct Stage {
    pub id: StageId,
    pub name: String,
    /// A `[[rom]]` id this manifest declares.
    pub rom: String,
}

/// One challenge this game offers. Several may share a `stage`: the same place
/// with different criteria is different challenges, and that is the normal case.
#[derive(Clone, Debug)]
pub struct Challenge {
    pub id: ChallengeId,
    pub name: String,
    pub stage: StageId,
    pub kind: ChallengeKind,
    /// Already in frames wherever a duration is involved. A manifest may author
    /// seconds; the conversion happens at load so that seconds never reach a
    /// `SessionEvent` or the wire.
    pub complete_when: Completion,
    /// Facts that end an attempt the way a death does — tally or objective
    /// names, validated against what the manifest emits (ADR 0025).
    pub ends_attempt_on: BTreeSet<String>,
    pub note: Option<String>,
    /// The goal as the contributor wrote it, if they did. [`Self::goal_text`]
    /// is what to show; this is here so a lobby can tell authored from
    /// composed.
    pub goal: Option<String>,
    /// The run's bound, already in frames at the game's framerate, when the
    /// manifest declares one. `None` means the lobby's default limit applies;
    /// a run always has one (ADR 0023, ADR 0026).
    pub limit_frames: Option<u64>,
    /// `Some` when this challenge's content is rolled per attempt from a seed
    /// (ADR 0013, ADR 0015), and the version of the table that rolls it. A
    /// dealer issues a seed for exactly these. The table itself is read by the
    /// adapter, through `content-gen`.
    pub generator_version: Option<u32>,
    /// Resolved at load: generated, or on a derived ROM.
    custom: bool,
    /// Composed at load from `complete_when`, for when `goal` is absent.
    goal_fallback: String,
}

impl Challenge {
    /// Whether dealing this challenge needs a seed.
    pub fn is_generated(&self) -> bool {
        self.generator_version.is_some()
    }

    /// A custom challenge is one that does not run on the game as it shipped:
    /// generated content (ADR 0013), or a stage on a hand-authored hack (ADR
    /// 0020). A lobby's "allow custom challenges" toggle filters on exactly
    /// this.
    pub fn is_custom(&self) -> bool {
        self.custom
    }

    /// What a player is shown as the game's name for this challenge: the ROM's
    /// name on a hack, because a hack has its own title and `[game].title` is
    /// the base game's; the game's title otherwise (ADR 0020).
    pub fn display_title(&self, lib: &Library) -> String {
        match lib.rom_of(&self.stage) {
            Some(rom) if rom.derived => rom.name.clone(),
            _ => lib.game.title.clone(),
        }
    }

    /// The goal in plain English, for the challenge intro and the lobby: what
    /// the contributor wrote, or one composed from `complete_when` — "Reach
    /// the end!", "Defeat 5 enemies!", "Survive for 45 seconds!". It says the
    /// goal, not the kind: the intro already names the kind.
    pub fn goal_text(&self) -> String {
        self.goal.clone().unwrap_or_else(|| self.goal_fallback.clone())
    }

    /// The challenge as `challenge-core` sees it. Nothing adapter-shaped
    /// crosses: a spec names a stage, never the state file behind it.
    pub fn spec(&self, game: &GameId) -> ChallengeSpec {
        ChallengeSpec {
            id: self.id.clone(),
            name: self.name.clone(),
            game: game.clone(),
            stage: self.stage.clone(),
            kind: self.kind,
            complete_when: self.complete_when.clone(),
            ends_attempt_on: self.ends_attempt_on.clone(),
        }
    }
}

/// One manifest's challenges, validated against the facts its own rules emit.
#[derive(Clone, Debug)]
pub struct Library {
    pub game: Game,
    /// In declaration order. Which one is the base, and that there is exactly
    /// one, is the runner's loader's to check.
    pub roms: Vec<Rom>,
    /// In declaration order.
    pub stages: Vec<Stage>,
    /// In declaration order.
    pub challenges: Vec<Challenge>,
}

impl Library {
    /// Reads the challenge half of a manifest's text. The caller reads the file;
    /// errors do not name it, so prefix them with the path if there is one.
    pub fn parse(text: &str) -> Result<Library, String> {
        let v: VersionDoc = toml::from_str(text).map_err(|e| e.to_string())?;
        if v.schema_version != SCHEMA_VERSION {
            return Err(format!(
                "schema_version {} is not {SCHEMA_VERSION}, which is what this build reads",
                v.schema_version
            ));
        }
        let doc: Doc = toml::from_str(text).map_err(|e| e.to_string())?;
        doc.into_library()
    }

    pub fn stage(&self, id: &str) -> Option<&Stage> {
        self.stages.iter().find(|s| s.id.0 == id)
    }

    pub fn rom(&self, id: &str) -> Option<&Rom> {
        self.roms.iter().find(|r| r.id == id)
    }

    /// The content a stage runs on. Load checked that every stage names a
    /// declared ROM, so this is `None` only for a stage this library does not
    /// have.
    pub fn rom_of(&self, stage: &StageId) -> Option<&Rom> {
        self.stage(&stage.0).and_then(|s| self.rom(&s.rom))
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

    /// Every challenge as `challenge-core` sees it — what a dealer deals from.
    pub fn specs(&self) -> Vec<ChallengeSpec> {
        self.challenges.iter().map(|c| c.spec(&self.game.id)).collect()
    }
}

// ------------------------------------------------------------------ catalog

/// Every library a lobby can offer, and a hash that says which library it is.
///
/// Two machines about to play a match have to be holding the same challenges
/// — the same criteria, the same patch tables, the same limits — and the
/// manifests ship with the app rather than being fetched, so each side hashes
/// what it loaded and the host compares. The hash is over the manifest texts
/// in a canonical order, which makes it independent of the order the files
/// were read in; it is for agreement, not security, and is FNV-1a so that
/// nothing new is linked for it.
///
/// No I/O: the caller reads the files and hands over the texts, as with
/// [`Library::parse`].
#[derive(Clone, Debug)]
pub struct Catalog {
    /// Sorted by game id.
    libraries: Vec<Library>,
    hash: u64,
}

/// How a lobby groups the games it lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Grouping {
    /// The decade of `[game].year`: "1990s". A game with no year is "unknown".
    Era,
    /// `[game].genre` as written. A game with none is "uncategorised".
    Genre,
    /// `[game].console` as written.
    Console,
}

impl Grouping {
    /// The category one game falls in under this grouping.
    pub fn category(self, game: &Game) -> String {
        match self {
            Grouping::Era => match game.year {
                Some(y) => format!("{}s", y / 10 * 10),
                None => "unknown".into(),
            },
            Grouping::Genre => game
                .genre
                .clone()
                .unwrap_or_else(|| "uncategorised".into()),
            Grouping::Console => game.console.clone(),
        }
    }
}

impl Catalog {
    /// Parses every text as a manifest. An error names the game where it can,
    /// and the text's position where the manifest did not get that far.
    pub fn parse_all(texts: &[&str]) -> Result<Catalog, String> {
        let mut entries = Vec::with_capacity(texts.len());
        for (i, text) in texts.iter().enumerate() {
            let lib = Library::parse(text).map_err(|e| format!("manifest #{}: {e}", i + 1))?;
            entries.push((text.to_string(), lib));
        }
        Catalog::new(entries)
    }

    /// Builds a catalog from libraries already parsed, each paired with the
    /// text it was parsed from — the hash is over the texts.
    ///
    /// Two games with one id, or two challenges with one id across games, are
    /// refused: a dealt challenge is named by its id alone, and a match has to
    /// mean one thing by it.
    pub fn new(mut entries: Vec<(String, Library)>) -> Result<Catalog, String> {
        entries.sort_by(|a, b| a.1.game.id.0.cmp(&b.1.game.id.0));

        let mut hash = FNV_OFFSET;
        let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
        for pair in entries.windows(2) {
            let (a, b) = (&pair[0].1.game.id.0, &pair[1].1.game.id.0);
            if a == b {
                return Err(format!("game `{a}`: two manifests declare it"));
            }
        }
        for (text, lib) in &entries {
            for c in &lib.challenges {
                if let Some(other) = seen.insert(&c.id.0, &lib.game.id.0) {
                    return Err(format!(
                        "challenge `{}`: declared by both `{other}` and `{}`; a challenge id \
                         names one challenge across the whole library",
                        c.id.0, lib.game.id.0
                    ));
                }
            }
            // Length-prefixed, so two texts cannot hash the same by moving a
            // boundary. Carriage returns are dropped so a checkout that
            // rewrote line endings still agrees with one that did not.
            let bytes: Vec<u8> = text.bytes().filter(|&b| b != b'\r').collect();
            hash = fnv1a(hash, &(bytes.len() as u64).to_le_bytes());
            hash = fnv1a(hash, &bytes);
        }

        Ok(Catalog { libraries: entries.into_iter().map(|(_, lib)| lib).collect(), hash })
    }

    /// Over every manifest text, in game-id order. Equal on two machines
    /// holding the same manifests, whatever order they read them in.
    pub fn hash(&self) -> u64 {
        self.hash
    }

    /// Every library, sorted by game id.
    pub fn games(&self) -> &[Library] {
        &self.libraries
    }

    pub fn game(&self, id: &str) -> Option<&Library> {
        self.libraries.iter().find(|l| l.game.id.0 == id)
    }

    /// A challenge and the library it belongs to. Ids are unique across the
    /// catalog, so there is at most one.
    pub fn challenge(&self, id: &ChallengeId) -> Option<(&Library, &Challenge)> {
        self.libraries
            .iter()
            .find_map(|l| l.challenge(&id.0).map(|c| (l, c)))
    }

    /// Every challenge in the catalog with its library, games in id order and
    /// challenges in declaration order.
    pub fn challenges(&self) -> impl Iterator<Item = (&Library, &Challenge)> {
        self.libraries
            .iter()
            .flat_map(|l| l.challenges.iter().map(move |c| (l, c)))
    }

    /// The games grouped for a lobby's tree, categories in sorted order and
    /// each category's games in id order. Every game appears exactly once.
    pub fn grouping(&self, by: Grouping) -> Vec<(String, Vec<&Library>)> {
        let mut groups: BTreeMap<String, Vec<&Library>> = BTreeMap::new();
        for lib in &self.libraries {
            groups.entry(by.category(&lib.game)).or_default().push(lib);
        }
        groups.into_iter().collect()
    }
}

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// FNV-1a, 64-bit, continued from `hash`. Written out rather than taken from a
/// crate for the reason the content generator's RNG is: the value is compared
/// across machines, so it has to be exactly this.
fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

// ------------------------------------------------------------- construction

/// What a manifest's rules can actually announce. A `complete_when` is checked
/// against these, because a criterion naming a fact nothing emits is a
/// challenge nobody can ever complete — which looks exactly like a wrong
/// address, and is the same failure `deny_unknown_fields` exists to prevent,
/// one level up.
#[derive(Default)]
struct Facts {
    cleared: bool,
    /// Every tally name a rule can advance, and what kind of fact advances it.
    /// One namespace (ADR 0025): `counts` and `ends_attempt_on` look names up
    /// here, so two kinds of fact sharing a name would make both ambiguous.
    tallies: BTreeMap<String, Tally>,
    objectives: BTreeSet<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tally {
    /// `damaged` or `enemy_defeated`: named by `session-api`, not the manifest.
    BuiltIn,
    Item,
    Counter,
}

impl Tally {
    fn word(self) -> &'static str {
        match self {
            Tally::BuiltIn => "built-in tally",
            Tally::Item => "item",
            Tally::Counter => "counter",
        }
    }
}

impl Facts {
    fn of(rules: &[RuleDoc]) -> Result<Facts, String> {
        let mut f = Facts::default();
        for (i, r) in rules.iter().enumerate() {
            let what = match &r.name {
                Some(n) => format!("rule `{n}`"),
                None => format!("rule #{} (on `{}`)", i + 1, r.field),
            };
            let emit = Emit::parse(
                &r.emit,
                r.reason.as_deref(),
                r.item.as_deref(),
                r.counter.as_deref(),
                r.objective.as_deref(),
                &what,
            )?;
            let (name, kind) = match emit {
                Emit::StageCleared => {
                    f.cleared = true;
                    continue;
                }
                Emit::EnemyDefeated => {
                    f.tally(ENEMY_DEFEATED.into(), Tally::BuiltIn, &what)?;
                    continue;
                }
                Emit::Damaged => {
                    f.tally(DAMAGED.into(), Tally::BuiltIn, &what)?;
                    continue;
                }
                Emit::Objective(n) => {
                    if let Some(t) = f.tallies.get(&n) {
                        return Err(format!(
                            "{what}: objective `{n}` has the same name as a {}",
                            t.word()
                        ));
                    }
                    f.objectives.insert(n);
                    continue;
                }
                Emit::Item(name) => (name, Tally::Item),
                Emit::Count(name) => (name, Tally::Counter),
                Emit::StageExited(_) | Emit::Died | Emit::Note => continue,
            };
            // A per-bit rule names one tally per bit — `kong_letter` becomes
            // `kong_letter:K` — so those are the names a criterion has to use.
            // Without labels the watcher falls back to the bit number, and
            // there are only ever 32. `bits_set` is the runner's edge
            // vocabulary; it is read here for this and nothing else.
            let names: Vec<String> = if r.edge == "bits_set" {
                if r.bit_labels.is_empty() {
                    (0..32).map(|b| format!("{name}:{b}")).collect()
                } else {
                    r.bit_labels.iter().map(|l| format!("{name}:{l}")).collect()
                }
            } else {
                vec![name]
            };
            for n in names {
                f.tally(n, kind, &what)?;
            }
        }
        Ok(f)
    }

    /// Records a tally name, refusing one that another kind of fact already
    /// uses — an item called `damaged`, or an item and a counter both called
    /// `note`. Several rules advancing one tally is fine and normal.
    fn tally(&mut self, name: String, kind: Tally, what: &str) -> Result<(), String> {
        if self.objectives.contains(&name) {
            return Err(format!(
                "{what}: {} `{name}` has the same name as an objective",
                kind.word()
            ));
        }
        match self.tallies.get(&name) {
            Some(&k) if k != kind => Err(format!(
                "{what}: {} `{name}` has the same name as a {}; a tally name means one thing",
                kind.word(),
                k.word()
            )),
            _ => {
                self.tallies.insert(name, kind);
                Ok(())
            }
        }
    }

    fn names(&self, kind: Option<Tally>) -> BTreeSet<String> {
        self.tallies
            .iter()
            .filter(|(_, k)| kind.map_or(true, |want| **k == want))
            .map(|(n, _)| n.clone())
            .collect()
    }
}

impl Doc {
    fn into_library(self) -> Result<Library, String> {
        if !(1.0..=1000.0).contains(&self.game.framerate) {
            return Err(format!(
                "game.framerate {} is not a plausible rate",
                self.game.framerate
            ));
        }
        let framerate = Framerate(self.game.framerate);

        // Only what a lobby needs from a ROM. Whether the array has exactly one
        // base, and whether a derived one is well-formed, is the runner's
        // loader's — one authority per rule (ADR 0020).
        let mut roms: Vec<Rom> = Vec::new();
        for r in &self.rom {
            if roms.iter().any(|x| x.id == r.id) {
                return Err(format!("rom `{}`: declared twice", r.id));
            }
            roms.push(Rom {
                id: r.id.clone(),
                name: r.name.clone().unwrap_or_else(|| r.id.clone()),
                derived: r.from.is_some(),
            });
        }

        let mut stages: Vec<Stage> = Vec::new();
        for s in &self.stage {
            if stages.iter().any(|x| x.id.0 == s.id) {
                return Err(format!("stage `{}`: declared twice", s.id));
            }
            if !roms.iter().any(|r| r.id == s.rom) {
                return Err(format!(
                    "stage `{}`: runs on rom `{}`, which this manifest does not declare; it has {}",
                    s.id,
                    s.rom,
                    list_or_none(&roms.iter().map(|r| r.id.clone()).collect())
                ));
            }
            stages.push(Stage {
                id: StageId(s.id.clone()),
                name: s.name.clone().unwrap_or_else(|| s.id.clone()),
                rom: s.rom.clone(),
            });
        }

        let facts = Facts::of(&self.rule)?;

        let mut challenges: Vec<Challenge> = Vec::new();
        for c in &self.challenge {
            let what = format!("challenge `{}`", c.id);
            if challenges.iter().any(|x| x.id.0 == c.id) {
                return Err(format!("{what}: declared twice"));
            }
            let kind = match c.kind.as_str() {
                "time_trial" => ChallengeKind::TimeTrial,
                "survival" => ChallengeKind::Survival,
                other => {
                    return Err(format!("{what}: kind `{other}`; try time_trial or survival"))
                }
            };
            let stage = stages.iter().find(|s| s.id.0 == c.stage).ok_or_else(|| {
                format!("{what}: stage `{}` is not declared by this manifest", c.stage)
            })?;

            let complete_when = completion(&c.complete_when, kind, framerate, &facts, &what)?;
            let ends_attempt_on = ends_attempt_on(&c.ends_attempt_on, &complete_when, &facts, &what)?;

            let generator_version = match (c.patch_slot.is_empty(), c.generator_version) {
                (true, None) => {
                    if !c.group.is_empty() {
                        return Err(format!(
                            "{what}: declares [challenge.group.*] and no patch_slot; a group \
                             is a pool that slots draw from"
                        ));
                    }
                    None
                }
                (false, None) => {
                    return Err(format!(
                        "{what}: declares patch_slot and no generator_version. A generated \
                         challenge is identified by (base rom, generator, version, seed), so \
                         the version is what a correction to the table bumps rather than \
                         editing it in place (ADR 0013)"
                    ))
                }
                (true, Some(_)) => {
                    return Err(format!(
                        "{what}: declares generator_version and no patch_slot, so nothing is \
                         generated. Remove it, or say what rolls"
                    ))
                }
                (false, Some(v)) => Some(as_u32(v, &format!("{what}: generator_version"))?),
            };

            let limit_frames = match c.limit_seconds {
                None => None,
                Some(secs) => {
                    if !secs.is_finite() || secs <= 0.0 {
                        return Err(format!(
                            "{what}: limit_seconds must be a positive number of seconds, got {secs}"
                        ));
                    }
                    let limit = framerate.frames(secs);
                    // A limit shorter than the survival the criteria ask for
                    // is a run that can never complete.
                    if let Some(need) = complete_when.survive_frames {
                        if limit < need {
                            return Err(format!(
                                "{what}: limit_seconds ({secs}) is shorter than survive_seconds, \
                                 so the run is over before it can complete"
                            ));
                        }
                    }
                    Some(limit)
                }
            };

            let derived = roms
                .iter()
                .find(|r| r.id == stage.rom)
                .is_some_and(|r| r.derived);
            let goal_fallback = compose_goal(&complete_when, framerate, &facts);

            challenges.push(Challenge {
                id: ChallengeId(c.id.clone()),
                name: c.name.clone().unwrap_or_else(|| stage.name.clone()),
                stage: stage.id.clone(),
                kind,
                complete_when,
                ends_attempt_on,
                note: c.note.clone(),
                goal: c.goal.clone(),
                limit_frames,
                generator_version,
                custom: generator_version.is_some() || derived,
                goal_fallback,
            });
        }

        Ok(Library {
            game: Game {
                id: GameId(self.game.id),
                title: self.game.title,
                console: self.game.console,
                region: self.game.region,
                framerate,
                genre: self.game.genre,
                year: self.game.year,
            },
            roms,
            stages,
            challenges,
        })
    }
}

/// One `complete_when`, checked against what the manifest can emit and
/// converted to frames.
fn completion(
    w: &CompleteWhenDoc,
    kind: ChallengeKind,
    framerate: Framerate,
    facts: &Facts,
    what: &str,
) -> Result<Completion, String> {
    let mut out = Completion {
        cleared: w.cleared.unwrap_or(false),
        survive_frames: None,
        counts: BTreeMap::new(),
        objectives: w.objectives.clone(),
    };
    // Three spellings feed one `counts`; a name given twice is two numbers for
    // one criterion, and neither is obviously the one meant.
    let mut count = |name: &str, n: u32, key: &str| -> Result<(), String> {
        if out.counts.insert(name.to_string(), n).is_some() {
            return Err(format!(
                "{what}: `{name}` is counted twice — {key} and another key name the same tally"
            ));
        }
        Ok(())
    };

    if w.cleared == Some(false) {
        // `cleared = false` reads as a criterion and is not one. Saying so
        // beats silently dropping it.
        return Err(format!(
            "{what}: `cleared = false` is not a criterion; remove the key instead"
        ));
    }
    if w.cleared == Some(true) && !facts.cleared {
        return Err(format!(
            "{what}: complete_when.cleared, but no [[rule]] emits `stage_cleared`"
        ));
    }

    if let Some(n) = w.enemies_defeated {
        let n = as_u32(n, &format!("{what}: enemies_defeated"))?;
        if n == 0 {
            return Err(format!("{what}: enemies_defeated must be at least 1"));
        }
        if !facts.tallies.contains_key(ENEMY_DEFEATED) {
            return Err(format!(
                "{what}: complete_when.enemies_defeated, but no [[rule]] emits \
                 `enemy_defeated`"
            ));
        }
        count(ENEMY_DEFEATED, n, "enemies_defeated")?;
    }

    for (item, n) in &w.items_collected {
        let n = as_u32(*n, &format!("{what}: items_collected.{item}"))?;
        if n == 0 {
            return Err(format!("{what}: items_collected.{item} must be at least 1"));
        }
        if facts.tallies.get(item) != Some(&Tally::Item) {
            return Err(format!(
                "{what}: no [[rule]] emits item `{item}`; this manifest emits {}",
                list_or_none(&facts.names(Some(Tally::Item)))
            ));
        }
        count(item, n, "items_collected")?;
    }

    for (name, n) in &w.counts {
        let n = as_u32(*n, &format!("{what}: counts.{name}"))?;
        if n == 0 {
            return Err(format!("{what}: counts.{name} must be at least 1"));
        }
        if !facts.tallies.contains_key(name) {
            return Err(format!(
                "{what}: no [[rule]] advances a tally named `{name}`; this manifest counts {}",
                list_or_none(&facts.names(None))
            ));
        }
        count(name, n, "counts")?;
    }

    if let Some(secs) = w.survive_seconds {
        if kind != ChallengeKind::Survival {
            return Err(format!(
                "{what}: survive_seconds is a survival criterion; \"survive {secs} \
                 seconds fastest\" is not something a time trial can be won at"
            ));
        }
        // Infinity and NaN both reach `Framerate::frames`, where the cast
        // saturates instead of failing, so they are caught here.
        if !secs.is_finite() || secs <= 0.0 {
            return Err(format!(
                "{what}: survive_seconds must be a positive number of seconds, got {secs}"
            ));
        }
        // Seconds in, frames out, once, here.
        out.survive_frames = Some(framerate.frames(secs));
    }

    for objective in &out.objectives {
        if !facts.objectives.contains(objective) {
            return Err(format!(
                "{what}: no [[rule]] emits objective `{objective}`; this manifest emits {}",
                list_or_none(&facts.objectives)
            ));
        }
    }

    if out.is_empty() {
        return Err(format!(
            "{what}: complete_when has no criteria, so nothing can ever complete it"
        ));
    }
    Ok(out)
}

/// The facts that end an attempt as a death does. Each must be something this
/// manifest emits, and none may be something the criteria need: a fact that
/// ends the attempt on the frame it arrives can never also complete the run on
/// it, for the same reason a death cannot (ADR 0023, ADR 0025).
fn ends_attempt_on(
    names: &[String],
    complete_when: &Completion,
    facts: &Facts,
    what: &str,
) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    for name in names {
        match name.as_str() {
            "died" | "stage_exited" | "stage_cleared" => {
                return Err(format!(
                    "{what}: ends_attempt_on `{name}` — that already ends an attempt; \
                     list facts that otherwise would not"
                ))
            }
            _ => {}
        }
        if !facts.tallies.contains_key(name) && !facts.objectives.contains(name) {
            let mut known = facts.names(None);
            known.extend(facts.objectives.iter().cloned());
            return Err(format!(
                "{what}: ends_attempt_on `{name}`, but no [[rule]] emits it; this manifest \
                 emits {}",
                list_or_none(&known)
            ));
        }
        if complete_when.counts.contains_key(name) || complete_when.objectives.contains(name) {
            return Err(format!(
                "{what}: `{name}` both completes the run and ends the attempt, so the run \
                 can never complete"
            ));
        }
        if !out.insert(name.clone()) {
            return Err(format!("{what}: ends_attempt_on lists `{name}` twice"));
        }
    }
    Ok(out)
}

/// The goal a challenge shows when its contributor wrote none: one phrase per
/// criterion, in a fixed order, joined with "and" and ending with "!".
///
/// A table, not a grammar. An item or counter is named as the manifest names
/// it — "Collect 50 banana" — because the name is the contributor's, and a
/// contributor who wants better prose writes a `goal`. `damaged` gets no
/// phrase: taking hits is not something anyone is asked to go and do.
fn compose_goal(w: &Completion, framerate: Framerate, facts: &Facts) -> String {
    let mut phrases: Vec<String> = Vec::new();
    if w.cleared {
        phrases.push("Reach the end".into());
    }
    if let Some(frames) = w.survive_frames {
        // Back to whole seconds for display only; the frames are what is
        // judged.
        let secs = (frames as f64 / framerate.0).round() as u64;
        phrases.push(format!("Survive for {secs} seconds"));
    }
    for (name, n) in &w.counts {
        let phrase = match (name.as_str(), facts.tallies.get(name)) {
            (ENEMY_DEFEATED, _) => {
                format!("Defeat {n} {}", if *n == 1 { "enemy" } else { "enemies" })
            }
            (DAMAGED, _) => continue,
            (_, Some(Tally::Item)) => format!("Collect {n} {name}"),
            _ => format!("Reach {n} {name}"),
        };
        phrases.push(phrase);
    }
    for objective in &w.objectives {
        phrases.push(format!("Reach {objective}"));
    }
    if phrases.is_empty() {
        // Only a `damaged` count on its own gets here. Something has to be
        // shown, and the manifest can always say it better.
        phrases.push("Complete the challenge".into());
    }
    format!("{}!", phrases.join(" and "))
}

fn as_u32(v: i64, what: &str) -> Result<u32, String> {
    u32::try_from(v).map_err(|_| format!("{what} is out of range, got {v}"))
}

/// For the tail of a "you named X, this manifest has Y" error. An empty set is
/// the interesting case — it means the rule was never written, not that the
/// name was misspelled — so it says so rather than trailing off after a colon.
fn list_or_none(names: &BTreeSet<String>) -> String {
    if names.is_empty() {
        "none at all".to_string()
    } else {
        names.iter().cloned().collect::<Vec<_>>().join(", ")
    }
}

#[cfg(test)]
mod tests;
