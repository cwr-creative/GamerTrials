//! The minimal `asset-store`: does this machine hold the base ROMs a match
//! needs, and are they the dumps the manifests were authored against?
//!
//! The platform never supplies a ROM (CLAUDE.md). What it can do is say, by
//! filename and SHA-1, which one a player lacks, so the host can toggle that
//! game off or the player can go and find their dump. Hacks and generated
//! content are derived from the base at prep time and need no check here;
//! states and patches ship with the app.
//!
//! What this crate will grow into — content-addressed bundles, hashing a DOS
//! zip by its members (ADR 0018), the cache of derived artifacts — is not here
//! yet. Today it hashes a file and compares.

#![forbid(unsafe_code)]

use std::fs;
use std::path::{Path, PathBuf};

/// A base ROM a manifest names: where the manifest expects it and what it
/// should hash to.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Needed {
    pub game: String,
    pub rom: String,
    /// The ROM's display name, for the message.
    pub name: String,
    pub path: PathBuf,
    /// The hash of the playtested dump; `None` means the manifest records
    /// none and any file at the path is accepted.
    pub sha1: Option<String>,
    /// Below this many bytes it is not a dump of this game.
    pub min_bytes: usize,
}

/// Why a needed ROM does not count as present.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Problem {
    Missing,
    Unreadable(String),
    TooSmall { bytes: usize, min: usize },
    WrongHash { have: String },
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Problem::Missing => f.write_str("not found"),
            Problem::Unreadable(e) => write!(f, "cannot read: {e}"),
            Problem::TooSmall { bytes, min } => {
                write!(f, "{bytes} bytes, below the {min} a dump of this game has")
            }
            Problem::WrongHash { have } => write!(f, "a different dump (hashes to {have})"),
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Verdict {
    pub needed: Needed,
    pub problem: Option<Problem>,
}

impl Verdict {
    pub fn ok(&self) -> bool {
        self.problem.is_none()
    }
}

pub fn sha1_of_file(path: &Path) -> std::io::Result<String> {
    Ok(content_gen::sha1_hex(&fs::read(path)?))
}

/// Checks one needed ROM.
pub fn check(needed: &Needed) -> Verdict {
    let problem = check_problem(needed);
    Verdict { needed: needed.clone(), problem }
}

fn check_problem(needed: &Needed) -> Option<Problem> {
    if !needed.path.exists() {
        return Some(Problem::Missing);
    }
    let bytes = match fs::read(&needed.path) {
        Ok(b) => b,
        Err(e) => return Some(Problem::Unreadable(e.to_string())),
    };
    if bytes.len() < needed.min_bytes {
        return Some(Problem::TooSmall { bytes: bytes.len(), min: needed.min_bytes });
    }
    if let Some(want) = &needed.sha1 {
        let have = content_gen::sha1_hex(&bytes);
        if !have.eq_ignore_ascii_case(want.trim()) {
            return Some(Problem::WrongHash { have });
        }
    }
    None
}

/// Checks every needed ROM, in order.
pub fn check_all(needed: &[Needed]) -> Vec<Verdict> {
    needed.iter().map(check).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, bytes: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join("gamertrials-asset-store-tests");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    fn needed(path: PathBuf, sha1: Option<&str>, min_bytes: usize) -> Needed {
        Needed {
            game: "g".into(),
            rom: "base".into(),
            name: "G".into(),
            path,
            sha1: sha1.map(str::to_string),
            min_bytes,
        }
    }

    #[test]
    fn a_matching_dump_passes() {
        let p = temp("ok.bin", b"hello");
        let n = needed(p, Some("aaf4c61ddcc5e8a2dabede0f3b482cd9aea9434d"), 1);
        assert!(check(&n).ok());
    }

    #[test]
    fn hash_case_does_not_matter() {
        let p = temp("case.bin", b"hello");
        let n = needed(p, Some("AAF4C61DDCC5E8A2DABEDE0F3B482CD9AEA9434D"), 1);
        assert!(check(&n).ok());
    }

    #[test]
    fn each_problem_is_named() {
        let gone = needed(PathBuf::from("does/not/exist.bin"), None, 1);
        assert_eq!(check(&gone).problem, Some(Problem::Missing));

        let small = needed(temp("small.bin", b"hi"), None, 100);
        assert_eq!(check(&small).problem, Some(Problem::TooSmall { bytes: 2, min: 100 }));

        let wrong = needed(temp("wrong.bin", b"hello"), Some("0000"), 1);
        assert!(matches!(check(&wrong).problem, Some(Problem::WrongHash { .. })));
    }

    #[test]
    fn no_recorded_hash_accepts_any_dump() {
        let n = needed(temp("any.bin", b"whatever"), None, 1);
        assert!(check(&n).ok());
    }
}
