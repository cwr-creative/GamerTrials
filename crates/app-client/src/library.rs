//! What this client holds: every manifest in `manifests/`, read twice — the
//! challenge half through `challenge-library`, as the server reads it, and the
//! runner half through `session-libretro`, which only this side needs.
//!
//! The file check lives here too: which base ROMs the manifests name, whether
//! the player's own dump of each is the one they were authored against
//! (`asset-store`), said as what is missing (ADR 0009's lobby).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use challenge_library::{Catalog, Grouping};
use protocol::{FileReport, MissingFile};
use session_libretro::manifest::Manifest;

pub struct Library {
    pub catalog: Catalog,
    /// The runner half, by game id.
    pub runner: BTreeMap<String, Manifest>,
    /// Where each manifest was read from, by game id.
    pub paths: BTreeMap<String, PathBuf>,
}

impl Library {
    pub fn load(dir: &Path) -> Result<Library, String> {
        let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        let mut texts = Vec::new();
        let mut runner = BTreeMap::new();
        let mut paths = BTreeMap::new();
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
            runner.insert(manifest.id.0.clone(), manifest);
        }
        if runner.is_empty() {
            return Err(format!("no manifests in {}", dir.display()));
        }
        let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let catalog = Catalog::parse_all(&refs)?;
        Ok(Library { catalog, runner, paths })
    }

    /// `Catalog::hash` as the wire carries it.
    pub fn hash_hex(&self) -> String {
        format!("{:016x}", self.catalog.hash())
    }

    /// Every base ROM the manifests name, checked against this machine.
    pub fn file_report(&self) -> FileReport {
        let mut missing = Vec::new();
        for (game, m) in &self.runner {
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
                if let Some(problem) = asset_store::check(&needed).problem {
                    missing.push(MissingFile {
                        game: game.clone(),
                        rom: rom.id.clone(),
                        name: format!(
                            "{} ({})",
                            path.file_name().map(|f| f.to_string_lossy()).unwrap_or_default(),
                            name
                        ),
                        sha1: rom.sha1.clone(),
                        problem: problem.to_string(),
                    });
                }
            }
        }
        FileReport { missing }
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
