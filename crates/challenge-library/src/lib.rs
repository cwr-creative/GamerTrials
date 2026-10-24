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
//! * `[[stage]]` — its `id` and `name` only. The rest (`rom`, `entry`) is the
//!   runner's, and the runner's loader is the strict one there.
//! * `[[rule]]` — only what each rule *emits*. A criterion naming a fact no
//!   rule announces is a challenge nobody can complete, and catching that needs
//!   the emit vocabulary but none of the addresses behind it.
//!
//! Anything else in the file is the runner's, and is not read at all.
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
    objective: Option<String>,
}

/// A stage as a dealer sees it: something to name. Where it runs and how it is
/// entered are the runner's.
#[derive(Deserialize, Debug)]
struct StageDoc {
    id: String,
    #[serde(default)]
    name: Option<String>,
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
    #[serde(default)]
    note: Option<String>,

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
    #[serde(default)]
    enemies_defeated: Option<i64>,
    #[serde(default)]
    items_collected: BTreeMap<String, i64>,
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
            "objective" => Emit::Objective(
                objective
                    .ok_or_else(|| format!("{what}: emit `objective` needs an `objective` name"))?
                    .to_string(),
            ),
            "note" => Emit::Note,
            other => {
                return Err(format!(
                    "{what}: unknown emit `{other}`; try stage_cleared, stage_exited, \
                     died, damaged, enemy_defeated, item, objective, note"
                ))
            }
        };
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
}

/// A stage, as far as dealing is concerned: an id a challenge names, and what
/// to call it.
#[derive(Clone, Debug)]
pub struct Stage {
    pub id: StageId,
    pub name: String,
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
    pub note: Option<String>,
    /// `Some` when this challenge's content is rolled per attempt from a seed
    /// (ADR 0013, ADR 0015), and the version of the table that rolls it. A
    /// dealer issues a seed for exactly these. The table itself is read by the
    /// adapter, through `content-gen`.
    pub generator_version: Option<u32>,
}

impl Challenge {
    /// Whether dealing this challenge needs a seed.
    pub fn is_generated(&self) -> bool {
        self.generator_version.is_some()
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
        }
    }
}

/// One manifest's challenges, validated against the facts its own rules emit.
#[derive(Clone, Debug)]
pub struct Library {
    pub game: Game,
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

// ------------------------------------------------------------- construction

/// What a manifest's rules can actually announce. A `complete_when` is checked
/// against these, because a criterion naming a fact nothing emits is a
/// challenge nobody can ever complete — which looks exactly like a wrong
/// address, and is the same failure `deny_unknown_fields` exists to prevent,
/// one level up.
#[derive(Default)]
struct Facts {
    cleared: bool,
    enemy: bool,
    items: BTreeSet<String>,
    objectives: BTreeSet<String>,
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
                r.objective.as_deref(),
                &what,
            )?;
            match emit {
                Emit::StageCleared => f.cleared = true,
                Emit::EnemyDefeated => f.enemy = true,
                Emit::Objective(n) => {
                    f.objectives.insert(n);
                }
                Emit::Item(name) => {
                    // A per-bit rule names one item per bit — `kong_letter`
                    // becomes `kong_letter:K` — so those are the names a
                    // criterion has to use. Without labels the watcher falls
                    // back to the bit number, and there are only ever 32.
                    // `bits_set` is the runner's edge vocabulary; it is read
                    // here for this and nothing else.
                    if r.edge == "bits_set" {
                        if r.bit_labels.is_empty() {
                            f.items.extend((0..32).map(|b| format!("{name}:{b}")));
                        } else {
                            f.items.extend(r.bit_labels.iter().map(|l| format!("{name}:{l}")));
                        }
                    } else {
                        f.items.insert(name);
                    }
                }
                _ => {}
            }
        }
        Ok(f)
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

        let mut stages: Vec<Stage> = Vec::new();
        for s in &self.stage {
            if stages.iter().any(|x| x.id.0 == s.id) {
                return Err(format!("stage `{}`: declared twice", s.id));
            }
            stages.push(Stage {
                id: StageId(s.id.clone()),
                name: s.name.clone().unwrap_or_else(|| s.id.clone()),
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

            challenges.push(Challenge {
                id: ChallengeId(c.id.clone()),
                name: c.name.clone().unwrap_or_else(|| stage.name.clone()),
                stage: stage.id.clone(),
                kind,
                complete_when,
                note: c.note.clone(),
                generator_version,
            });
        }

        Ok(Library {
            game: Game {
                id: GameId(self.game.id),
                title: self.game.title,
                console: self.game.console,
                region: self.game.region,
                framerate,
            },
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
        enemies_defeated: None,
        items_collected: BTreeMap::new(),
        objectives: w.objectives.clone(),
    };

    if w.cleared == Some(false) {
        // `cleared = false` reads as a criterion and is not one. Saying so
        // beats silently dropping it.
        return Err(format!(
            "{what}: `cleared = false` is not a criterion; remove the key instead"
        ));
    }
    if out.cleared && !facts.cleared {
        return Err(format!(
            "{what}: complete_when.cleared, but no [[rule]] emits `stage_cleared`"
        ));
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

    if let Some(n) = w.enemies_defeated {
        let n = as_u32(n, &format!("{what}: enemies_defeated"))?;
        if n == 0 {
            return Err(format!("{what}: enemies_defeated must be at least 1"));
        }
        if !facts.enemy {
            return Err(format!(
                "{what}: complete_when.enemies_defeated, but no [[rule]] emits \
                 `enemy_defeated`"
            ));
        }
        out.enemies_defeated = Some(n);
    }

    for (item, count) in &w.items_collected {
        let n = as_u32(*count, &format!("{what}: items_collected.{item}"))?;
        if n == 0 {
            return Err(format!("{what}: items_collected.{item} must be at least 1"));
        }
        if !facts.items.contains(item) {
            return Err(format!(
                "{what}: no [[rule]] emits item `{item}`; this manifest emits {}",
                list_or_none(&facts.items)
            ));
        }
        out.items_collected.insert(item.clone(), n);
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
