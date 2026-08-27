//! The libretro adapter. One crate, every libretro game.
//!
//! Phase 1 built an SMB3 harness. Phase 2 built a DKC harness beside it, on
//! purpose, without sharing a line — because ADR 0012 says an interface is read
//! off two implementations rather than guessed at from one. This crate is what
//! the two turned out to have in common, and the surprise was how much:
//!
//! | Module | Was |
//! |---|---|
//! | [`libretro`] | two near-identical FFI layers; the SNES one was a superset |
//! | [`audio`] | byte-for-byte identical in both |
//! | [`input`] | identical but for the pad's face buttons |
//! | [`memory`] | two `read_byte`s that disagreed only on which route to try first |
//! | [`watch`] | two hand-written `tick`s that differed only in field, edge and meaning |
//!
//! The last row is the interesting one. Once the differences are named — which
//! field, which kind of transition, what it means — they are small enough to be
//! data, and they moved into the manifest. There is no SMB3 or DKC code in this
//! crate; there are two TOML files in `manifests/`, and a third game is a third
//! file (invariant 5).
//!
//! ```no_run
//! use session_api::StageRunner;
//! use session_libretro::{manifest::Manifest, runner::LibretroRunner};
//! # fn main() -> Result<(), String> {
//! let manifest = Manifest::load(std::path::Path::new("manifests/dkc.toml"))?;
//! let stage = manifest.stage("jungle-hijinx").expect("declared in the manifest");
//! let spec = manifest.stage_spec(stage);
//! let mut runner = LibretroRunner::start(manifest, None, None, Vec::new())?;
//! runner.enter(&spec).map_err(|e| e.to_string())?;
//! loop {
//!     for event in runner.advance().map_err(|e| e.to_string())? {
//!         println!("{event}");
//!     }
//! }
//! # }
//! ```

pub mod audio;
pub mod input;
pub mod libretro;
pub mod manifest;
pub mod memory;
pub mod ramsearch;
pub mod runner;
pub mod statebind;
pub mod watch;

pub use manifest::Manifest;
pub use runner::LibretroRunner;
pub use watch::Watcher;

#[cfg(test)]
mod manifest_tests;
