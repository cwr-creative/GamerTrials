//! One bout on one shared machine (ADR 0036).
//!
//! A head-to-head round puts several players on one stage, one per
//! [`Slot`](session_api::Slot). Two mechanisms keep their machines in step,
//! and this crate drives both:
//!
//! * [`Rollback`] — for a machine the frontend can save and restore every
//!   frame ([`SharedRunner`](session_api::SharedRunner)): the local slot's
//!   input goes in at once, the remote slots' is guessed, and when the real
//!   input arrives and the guess was wrong the machine is restored and run
//!   again. The engine is the `ggrs` crate; this crate answers its save,
//!   load and advance requests with the trait, and **holds back the facts
//!   a frame produced until every slot's input for it is final** — so a
//!   judge never sees a fact from a frame that was rolled back.
//! * [`Relay`] — for a machine that brings its own network
//!   ([`Networked`](session_api::Networked)): the game's own netcode keeps
//!   the machines in step; this crate only carries the packets.
//!
//! Both run over a [`Link`]: bytes between slots, over UDP between the
//! players' machines ([`link::Udp`]) or over channels in a test
//! ([`link::Channels`]). The only socket this crate opens is that one.
//!
//! No emulator, no protocol, no server: a consumer of `session-api` like
//! `challenge-core` is, handed a machine and a link.

#![forbid(unsafe_code)]

pub mod link;
pub mod relay;
pub mod rollback;

pub use link::{is_public, reach, Channels, Link, Udp};
pub use relay::Relay;
pub use rollback::{Confirmed, Error, Rollback, Settings, State, Stats};
