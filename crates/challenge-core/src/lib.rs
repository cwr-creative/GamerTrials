//! The only party that judges anything (ADR 0017).
//!
//! An adapter produces facts — [`SessionEvent`](session_api::SessionEvent).
//! A manifest declares criteria — [`ChallengeSpec`](session_api::ChallengeSpec).
//! This crate compares one against the other:
//!
//! * [`run`] — one player's go at one challenge: when an attempt ends, whether
//!   the next one is a re-entry or the end of the run, and what the run was
//!   worth.
//! * [`deal`] — which challenges a session plays, in what order.
//! * [`rank`] — how the players who ran one challenge place against each other.
//!
//! Pure (invariant 1). Nothing here reads a file, a clock or a socket, and
//! nothing here drives a [`StageRunner`](session_api::StageRunner): a run is
//! told what happened and answers what should happen next. Whatever owns the
//! runner — the harness today, the client and server later — does the doing.
//! That is what lets every rule below be tested with a literal list of events.

#![forbid(unsafe_code)]

pub mod deal;
pub mod rank;
pub mod run;

pub use deal::{deal, DealError, Mode};
pub use rank::places;
pub use run::{AttemptEnd, Ending, Next, Run, RunError, RunResult};
