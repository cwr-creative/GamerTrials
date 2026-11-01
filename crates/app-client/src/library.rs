//! What this client holds: every manifest in `manifests/`, read twice — the
//! challenge half through `challenge-library`, as the server reads it, and the
//! runner half through `session-libretro`, which only this side needs.
//!
//! The file check lives here too: which base ROMs the manifests name, whether
//! the player's own dump of each is the one they were authored against
//! (`asset-store`), said as what is missing (ADR 0009's lobby). It covers
//! cores as well, so that nothing the lobby passes is found missing only once
//! a round is dealt (ADR 0029). The lobby runs it every few seconds, so a dump
//! is hashed again only when its size or modification time moved.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use challenge_library::{Catalog, Grouping};
use protocol::{FileReport, MissingFile};
use session_libretro::manifest::Manifest;
use session_libretro::runner;

pub struct Library {
    pub catalog: Catalog,
    /// The runner half, by game id.
    pub runner: BTreeMap<String, Manifest>,
    /// Where each manifest was read from, by game id.
    pub paths: BTreeMap<String, PathBuf>,
    /// Which core each game runs on, by game id: what a player's per-core
    /// settings are keyed by (ADR 0028).
    pub cores: BTreeMap<String, String>,
    /// What each dump's check found, while its size and mtime stand.
    checked: Mutex<BTreeMap<PathBuf, Checked>>,
}

/// A dump's size and mtime when it was checked, and what the check found.
type Checked = (u64, Option<SystemTime>, Option<String>);

/// A core's id: its DLL's name without `_libretro` — `mesen`, `snes9x`,
/// `dosbox_pure`. A `core_dir` holding no core is named for the directory,
/// so its games still have a key and the page can say the core is missing.
pub fn core_id(m: &Manifest) -> String {
    let dir = &m.libretro.core_dir;
    match runner::find_core(dir) {
        Ok(p) => p
            .file_stem()
            .map(|s| s.to_string_lossy().trim_end_matches("_libretro").to_string())
            .unwrap_or_default(),
        Err(_) => format!(
            "missing:{}",
            dir.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
        ),
    }
}

impl Library {
    pub fn load(dir: &Path) -> Result<Library, String> {
        let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut texts = Vec::new();
        let mut runner = BTreeMap::new();
        let mut paths = BTreeMap::new();
        let mut cores = BTreeMap::new();
        let mut files: Vec<PathBuf> = entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "toml"))
            .collect();
        files.sort();
        for path in files {
            let manifest = Manifest::load(&path)?;
            let text =
                fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
            texts.push(text);
            paths.insert(manifest.id.0.clone(), path);
            cores.insert(manifest.id.0.clone(), core_id(&manifest));
            runner.insert(manifest.id.0.clone(), manifest);
        }
        if runner.is_empty() {
            return Err(format!("no manifests in {}", dir.display()));
        }
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let catalog = Catalog::parse_all(&refs)?;
        Ok(Library { catalog, runner, paths, cores, checked: Mutex::default() })
    }

    /// `Catalog::hash` as the wire carries it.
    pub fn hash_hex(&self) -> String {
        format!("{:016x}", self.catalog.hash())
    }

    /// Every core and base ROM the manifests name, checked against this machine.
    pub fn file_report(&self) -> FileReport {
        let mut missing = Vec::new();
        for (game, m) in &self.runner {
            let dir = &m.libretro.core_dir;
            if let Err(e) = runner::find_core(dir) {
                // The runner's wording names its own `--core` flag; say it plainly.
                let problem = if e.starts_with("several") {
                    format!("more than one *{} there; keep one", runner::CORE_SUFFIX)
                } else {
                    format!("no *{} there", runner::CORE_SUFFIX)
                };
                missing.push(MissingFile {
                    game: game.clone(),
                    rom: "core".into(),
                    name: format!("libretro core in {}", dir.display()),
                    sha1: None,
                    problem: problem.into(),
                });
            }
            for rom in &m.roms {
                if rom.from.is_some() {
                    // Derived from the base by a patch that ships with the app.
                    continue;
                }
                let name = rom.name.clone().unwrap_or_else(|| rom.id.clone());
                let Some(path) = &rom.path else {
                    missing.push(MissingFile {
                        game: game.clone(),
                        rom: rom.id.clone(),
                        name,
                        sha1: rom.sha1.clone(),
                        problem: "the manifest names no path for it".into(),
                    });
                    continue;
                };
                let needed = asset_store::Needed {
                    game: game.clone(),
                    rom: rom.id.clone(),
                    name: name.clone(),
                    path: PathBuf::from(m.resolve(&path.to_string_lossy())),
                    sha1: rom.sha1.clone(),
                    min_bytes: rom.min_bytes,
                };
                if let Some(problem) = self.check(&needed) {
                    missing.push(MissingFile {
                        game: game.clone(),
                        rom: rom.id.clone(),
                        name: format!(
                            "{} ({})",
                            path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default(),
                            name
                        ),
                        sha1: rom.sha1.clone(),
                        problem,
                    });
                }
            }
        }
        FileReport { missing }
    }

    /// `asset_store::check`, remembered while the file's size and mtime stand.
    fn check(&self, needed: &asset_store::Needed) -> Option<String> {
        let stamp = fs::metadata(&needed.path).ok().map(|m| (m.len(), m.modified().ok()));
        let mut checked = self.checked.lock().unwrap();
        if let (Some((len, mtime)), Some((l, t, problem))) = (stamp, checked.get(&needed.path)) {
            if (len, mtime) == (*l, *t) {
                return problem.clone();
            }
        }
        let problem = asset_store::check(needed).problem.map(|p| p.to_string());
        match stamp {
            Some((len, mtime)) => {
                checked.insert(needed.path.clone(), (len, mtime, problem.clone()));
            }
            None => {
                checked.remove(&needed.path);
            }
        }
        problem
    }

    /// What the settings page needs to know about every game: its core, how
    /// it is driven, which RetroPad buttons it has, the manifest's suggested
    /// pad keys, and the built-in mapping each layer finally inherits.
    pub fn settings_meta(&self) -> serde_json::Value {
        use session_libretro::keymap::{RetroButton, GUEST_KEYS};
        use session_libretro::manifest::{Controller, PadLayout};
        let games: Vec<serde_json::Value> = self
            .runner
            .iter()
            .map(|(id, m)| {
                let controller = m.libretro.controller;
                let layout = crate::settings::layout_of(controller);
                let builtin = serde_json::json!({
                    "keys": crate::settings::builtin(layout, crate::settings::Family::Keys),
                    "pad": crate::settings::builtin(layout, crate::settings::Family::Pad),
                });
                let lib = self.catalog.game(id);
                serde_json::json!({
                    "id": id,
                    "title": m.title,
                    "console": lib.map(|l| l.game.console.clone()).unwrap_or_else(|| m.console.clone()),
                    "core": self.cores[id],
                    "controller": match controller {
                        Controller::Pad(_) => "pad",
                        Controller::Keyboard => "keyboard",
                    },
                    "layout": match crate::settings::layout_of(controller) {
                        PadLayout::Nes => "nes",
                        PadLayout::Snes => "snes",
                    },
                    "buttons": crate::settings::buttons_of(controller).iter().map(|b| b.name()).collect::<Vec<_>>(),
                    "builtin": builtin,
                    "pad_keys": m.libretro.pad_keys,
                })
            })
            .collect();
        serde_json::json!({
            "games": games,
            "retropad": RetroButton::ALL.map(RetroButton::name),
            "guest_keys": GUEST_KEYS.iter().map(|g| g.name).collect::<Vec<_>>(),
            "max_slots": crate::settings::MAX_SLOTS,
        })
    }

    /// What the lobby page builds its tree from: every game under each
    /// grouping, with every challenge and what a player should read about it.
    pub fn ui_catalog(&self) -> serde_json::Value {
        let games: Vec<serde_json::Value> = self
            .catalog
            .games()
            .iter()
            .map(|lib| {
                let challenges: Vec<serde_json::Value> = lib
                    .challenges
                    .iter()
                    .map(|c| {
                        serde_json::json!({
                            "id": c.id.0,
                            "name": c.name,
                            "title": c.display_title(lib),
                            "kind": match c.kind {
                                session_api::ChallengeKind::TimeTrial => "time_trial",
                                session_api::ChallengeKind::Survival => "survival",
                            },
                            "goal": c.goal_text(),
                            "custom": c.is_custom(),
                            "generated": c.is_generated(),
                            "limit_frames": c.limit_frames,
                        })
                    })
                    .collect();
                serde_json::json!({
                    "id": lib.game.id.0,
                    "title": lib.game.title,
                    "console": lib.game.console,
                    "era": Grouping::Era.category(&lib.game),
                    "genre": Grouping::Genre.category(&lib.game),
                    "framerate": lib.game.framerate.0,
                    "challenges": challenges,
                })
            })
            .collect();
        serde_json::json!({ "hash": self.hash_hex(), "games": games })
    }
}
