//! SMB3 RAM watching. Deliberately game-specific — Phase 1 has one game, and
//! ADR 0012 says the interface gets extracted once DKC exists to abstract
//! against, not before.
//!
//! Addresses here are CPU addresses, and there are two ways to reach them.
//!
//! `RETRO_MEMORY_SYSTEM_RAM` gives the NES's 2 KB internal RAM ($0000-$07FF)
//! as a plain slice. It does *not* reach cartridge WRAM at $6000-$7FFF, where
//! SMB3 keeps the card-match state. Mesen exposes no `RETRO_MEMORY_SAVE_RAM`
//! at all for this cart — that region is only classified as "save RAM" when
//! it's battery-backed, and SMB3's isn't — so querying it returns nothing.
//!
//! The route that does work is the memory map the core publishes via
//! `SET_MEMORY_MAPS`, which covers the whole $0000-$7FFF space. `read_byte`
//! below prefers that and falls back to the system-RAM slice, so callers
//! (including `--watch`) never have to know which region an address is in,
//! and a core that publishes no map still works for low addresses.
//!
//! Provenance matters:
//!
//!   $0736 lives            verified (playtested against this harness)
//!   $0727 world - 1        verified (playtested against this harness)
//!   $0014 return-to-map    verified (playtested against this harness)
//!   $0078 / $0079          verified (playtested against this harness)
//!   $7D9C card count       verified (playtested against this harness) —
//!                          increments when Mario is awarded a card at a
//!                          level's end, well before $0014 moves
//!   $0578 powerup/form     documented (Data Crystal SMB3 RAM map), not yet
//!                          playtested here. Values 00-06: small, super, fire,
//!                          raccoon, frog, tanooki, hammer.
//!   $0552 hit-invuln timer documented (Data Crystal), not yet playtested here
//!   $0510 pipe transit     documented (Data Crystal), not yet playtested here
//!   $05F4 stomp counter    documented (Data Crystal), not yet playtested here
//!
//! SMB3 has no documented "current level number" — a level is identified by the
//! map tile you entered it from, so the map position pair *is* the level id.
//! Run with `--watch` to confirm any of this against the real game; every
//! address is a single constant to change. `--watch <lo>-<hi>` sweeps a range,
//! for hunting down anything not listed here.
//!
//! The stomp counter only counts stomps — a kill by fireball, shell, or star
//! does not touch it. That is a gap, not a bug; there is no known address for
//! "any enemy died" independent of method.
//!
//! Not every level awards a card (fortresses, castles, and airships don't run
//! the card-match bonus), so $7D9C alone cannot detect completion of those.
//! It is still strictly better than $0014 for the levels it does cover: it
//! moves the moment the card is awarded, seconds before the return-to-map
//! flag does, and unlike $0014 it never fires on death — so a card increase
//! is an unambiguous, early success signal with no death-vs-win disambiguation
//! needed. $0014 remains the only signal for a level that doesn't award a
//! card, which means every $0014 rise not already explained by a card is
//! read as failure (see `Watcher::tick`).

use std::fmt;

pub const ADDR_RETURN_TO_MAP: usize = 0x0014;
pub const ADDR_MAP_Y: usize = 0x0078;
pub const ADDR_MAP_X: usize = 0x0079;
pub const ADDR_OBJECT_SET: usize = 0x070A;
pub const ADDR_WORLD: usize = 0x0727;
pub const ADDR_LIVES: usize = 0x0736;
pub const ADDR_POWERUP: usize = 0x0578;
pub const ADDR_HIT_TIMER: usize = 0x0552;
pub const ADDR_PIPE_TIMER: usize = 0x0510;
pub const ADDR_STOMP_COUNTER: usize = 0x05F4;
pub const ADDR_CARDS: usize = 0x7D9C;

/// Size of `RETRO_MEMORY_SYSTEM_RAM` on the NES: internal RAM, $0000-$07FF.
const SYSTEM_RAM_SIZE: usize = 0x0800;

/// Reads one byte at a CPU address. Prefers the core's published memory map,
/// which is the only route to $6000-$7FFF; falls back to the system-RAM slice
/// so cores that publish no map still resolve $0000-$07FF. `None` if neither
/// route covers `addr`.
pub fn read_byte(addr: usize, ram: &[u8]) -> Option<u8> {
    // The fallback is bounded by the slice itself: real system RAM is 2 KB, so
    // anything at $0800 or above misses and returns None, which is what a core
    // without a memory map should report for those addresses.
    crate::libretro::read_addr(addr).or_else(|| ram.get(addr).copied())
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Snapshot {
    pub world: u8,
    pub map_x: u8,
    pub map_y: u8,
    pub lives: u8,
    pub object_set: u8,
    pub return_to_map: u8,
    pub powerup: u8,
    pub hit_timer: u8,
    pub pipe_timer: u8,
    pub stomp_counter: u8,
    pub cards: u8,
}

impl Snapshot {
    pub fn read(ram: &[u8]) -> Option<Snapshot> {
        // Every system-RAM address above is inside the 2 KB internal RAM. A
        // core handing back something smaller is not the NES core we think
        // it is.
        if ram.len() < SYSTEM_RAM_SIZE {
            return None;
        }
        Some(Snapshot {
            world: ram[ADDR_WORLD],
            map_x: ram[ADDR_MAP_X],
            map_y: ram[ADDR_MAP_Y],
            lives: ram[ADDR_LIVES],
            object_set: ram[ADDR_OBJECT_SET],
            return_to_map: ram[ADDR_RETURN_TO_MAP],
            powerup: ram[ADDR_POWERUP],
            hit_timer: ram[ADDR_HIT_TIMER],
            pipe_timer: ram[ADDR_PIPE_TIMER],
            stomp_counter: ram[ADDR_STOMP_COUNTER],
            // A core with no memory map can't reach $7D9C, so cards never
            // appear to move and the card-triggered StageComplete path just
            // never fires — a safe degrade, not a crash. `main` warns at
            // startup when that is the case.
            cards: read_byte(ADDR_CARDS, ram).unwrap_or(0),
        })
    }

    /// $0727 holds world - 1, so world 1 reads as 0.
    pub fn world_number(&self) -> u16 {
        self.world as u16 + 1
    }
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "world {}  level map({:02X},{:02X}) set {:02X}  lives {}  powerup {:02X}  cards {}",
            self.world_number(),
            self.map_x,
            self.map_y,
            self.object_set,
            self.lives,
            self.powerup,
            self.cards
        )
    }
}

/// What the watcher saw this frame, if anything.
pub enum Event {
    StageComplete,
    StageFailed,
}

pub struct Watcher {
    prev: Option<Snapshot>,
    /// Set once a stage event has fired for the *current attempt*, so a
    /// card-triggered StageComplete can't be followed by a StageFailed off
    /// the same level's ordinary $0014 exit a few seconds later. Cleared
    /// again when that exit completes — see the falling edge in `tick`.
    resolved: bool,
    /// Extra addresses to report on change, from `--watch`.
    watched: Vec<usize>,
    prev_watched: Vec<u8>,
}

impl Watcher {
    pub fn new(watched: Vec<usize>) -> Self {
        Watcher {
            prev: None,
            resolved: false,
            prev_watched: vec![0; watched.len()],
            watched,
        }
    }

    /// Called after a savestate load: the stage restarts, so nothing carried
    /// over from before the load is meaningful.
    pub fn reset(&mut self) {
        self.prev = None;
        self.resolved = false;
    }

    /// Reads RAM and prints anything that changed. Returns a stage event when
    /// one fires. `frame` counts `retro_run` calls since stage entry (ADR 0005:
    /// timing is emulated frames, never wall-clock).
    pub fn tick(&mut self, ram: &[u8], frame: u64) -> Option<Event> {
        let Some(cur) = Snapshot::read(ram) else {
            return None;
        };
        self.report_watched(ram, frame);

        let prev = match self.prev {
            Some(p) => p,
            None => {
                // First frame of a stage: state the baseline, then only deltas.
                println!("[ram] frame {frame}  {cur}");
                self.prev = Some(cur);
                return None;
            }
        };

        if (cur.world, cur.map_x, cur.map_y, cur.lives, cur.object_set, cur.cards)
            != (prev.world, prev.map_x, prev.map_y, prev.lives, prev.object_set, prev.cards)
        {
            println!("[ram] frame {frame}  {cur}");
        }

        if cur.powerup != prev.powerup {
            println!(
                "[event] frame {frame}  POWERUP CHANGE  ${ADDR_POWERUP:04X} {:02X} -> {:02X}",
                prev.powerup, cur.powerup
            );
        }

        // Rising from zero, not just "nonzero", so a still-running timer from a
        // prior hit doesn't retrigger the event every frame it's counting down.
        if prev.hit_timer == 0 && cur.hit_timer != 0 {
            println!("[event] frame {frame}  DAMAGE  ${ADDR_HIT_TIMER:04X} set to {:02X}", cur.hit_timer);
        }

        if prev.pipe_timer == 0 && cur.pipe_timer != 0 {
            println!("[event] frame {frame}  PIPE  ${ADDR_PIPE_TIMER:04X} set to {:02X}", cur.pipe_timer);
        }

        if cur.stomp_counter != prev.stomp_counter {
            println!(
                "[event] frame {frame}  ENEMY STOMPED  ${ADDR_STOMP_COUNTER:04X} {:02X} -> {:02X}",
                prev.stomp_counter, cur.stomp_counter
            );
        }

        // A card is an early, unambiguous win signal for the levels that
        // award one — it never fires on death, so there's no lives check to
        // make here the way $0014 used to need. `resolved` stops a card that
        // increases more than once (multiple cards in one bonus round, if
        // that happens) from reporting StageComplete twice.
        let mut event = None;
        if !self.resolved && cur.cards > prev.cards {
            println!(
                "[event] frame {frame}  CARD  ${ADDR_CARDS:04X} {} -> {}",
                prev.cards, cur.cards
            );
            event = Some(Event::StageComplete);
            self.resolved = true;
        }

        // Anything that wasn't already resolved by a card and still leaves
        // the level is a failure. This covers card-less levels (fortresses,
        // castles, airships) where $0014 is the only signal there is, as
        // well as ordinary deaths.
        if !self.resolved && prev.return_to_map == 0 && cur.return_to_map != 0 {
            println!(
                "[event] frame {frame}  RETURN TO MAP  ${ADDR_RETURN_TO_MAP:04X} set to {:02X}",
                cur.return_to_map
            );
            event = Some(Event::StageFailed);
            self.resolved = true;
        }

        // The flag clearing again means the hand-off to the map has finished,
        // so whatever attempt `resolved` was suppressing is over and the next
        // one gets a clean slate. Without this the flag latches for the whole
        // process: one card early on would silence every later death, which
        // is exactly the bug this replaced.
        if prev.return_to_map != 0 && cur.return_to_map == 0 {
            self.resolved = false;
        }

        self.prev = Some(cur);
        event
    }

    /// Print the current snapshot unconditionally (bound to F5).
    pub fn dump(&self, ram: &[u8], frame: u64) {
        match Snapshot::read(ram) {
            Some(s) => println!(
                "[ram] frame {frame}  {s}  return_to_map {:02X}  hit_timer {:02X}  pipe_timer {:02X}  stomp {:02X}",
                s.return_to_map, s.hit_timer, s.pipe_timer, s.stomp_counter
            ),
            None => println!("[ram] core exposes {} bytes of system RAM, expected 2048", ram.len()),
        }
        for &addr in &self.watched {
            match read_byte(addr, ram) {
                Some(b) => println!("        ${addr:04X} = {b:02X} ({b})"),
                None => println!("        ${addr:04X} = <unmapped>"),
            }
        }
    }

    fn report_watched(&mut self, ram: &[u8], frame: u64) {
        for (i, &addr) in self.watched.iter().enumerate() {
            let Some(b) = read_byte(addr, ram) else { continue };
            if b != self.prev_watched[i] {
                println!(
                    "[watch] frame {frame}  ${addr:04X} = {b:02X} ({b})  was {:02X}",
                    self.prev_watched[i]
                );
                self.prev_watched[i] = b;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A RAM buffer big enough that `read_byte`'s slice fallback reaches
    /// $7D9C. No memory map is published under `cargo test`, so the fallback
    /// is the only route and the card address is writable from a test.
    fn ram() -> Vec<u8> {
        vec![0u8; 0x8000]
    }

    /// Drives one frame and returns the event, if any.
    fn step(w: &mut Watcher, ram: &[u8], frame: u64) -> Option<Event> {
        w.tick(ram, frame)
    }

    #[test]
    fn first_tick_is_baseline_only() {
        let mut w = Watcher::new(Vec::new());
        assert!(step(&mut w, &ram(), 1).is_none());
    }

    #[test]
    fn return_to_map_rise_fails_the_stage() {
        let mut w = Watcher::new(Vec::new());
        let mut r = ram();
        step(&mut w, &r, 1);
        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(matches!(step(&mut w, &r, 2), Some(Event::StageFailed)));
    }

    #[test]
    fn card_completes_the_stage() {
        let mut w = Watcher::new(Vec::new());
        let mut r = ram();
        step(&mut w, &r, 1);
        r[ADDR_CARDS] = 1;
        assert!(matches!(step(&mut w, &r, 2), Some(Event::StageComplete)));
    }

    /// The card fires seconds before $0014 moves; that later rise is the same
    /// level's ordinary exit and must not be reported as a failure.
    #[test]
    fn exit_after_a_card_is_not_a_failure() {
        let mut w = Watcher::new(Vec::new());
        let mut r = ram();
        step(&mut w, &r, 1);
        r[ADDR_CARDS] = 1;
        assert!(matches!(step(&mut w, &r, 2), Some(Event::StageComplete)));
        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(step(&mut w, &r, 3).is_none());
    }

    /// Regression: `resolved` used to latch for the whole process, so one
    /// card early in a run silenced every later death.
    #[test]
    fn a_death_after_a_card_still_fails() {
        let mut w = Watcher::new(Vec::new());
        let mut r = ram();
        step(&mut w, &r, 1);

        // Attempt one: cleared by card, then the level exits to the map.
        r[ADDR_CARDS] = 1;
        assert!(matches!(step(&mut w, &r, 2), Some(Event::StageComplete)));
        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(step(&mut w, &r, 3).is_none());
        r[ADDR_RETURN_TO_MAP] = 0;
        assert!(step(&mut w, &r, 4).is_none());

        // Attempt two: a death, which must still be reported.
        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(matches!(step(&mut w, &r, 5), Some(Event::StageFailed)));
    }

    /// The same latch also silenced a second death after a first one.
    #[test]
    fn two_deaths_both_fail() {
        let mut w = Watcher::new(Vec::new());
        let mut r = ram();
        step(&mut w, &r, 1);

        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(matches!(step(&mut w, &r, 2), Some(Event::StageFailed)));
        r[ADDR_RETURN_TO_MAP] = 0;
        assert!(step(&mut w, &r, 3).is_none());

        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(matches!(step(&mut w, &r, 4), Some(Event::StageFailed)));
    }

    /// A flag that stays high must report once, not every frame it is set.
    #[test]
    fn held_return_to_map_fires_once() {
        let mut w = Watcher::new(Vec::new());
        let mut r = ram();
        step(&mut w, &r, 1);
        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(matches!(step(&mut w, &r, 2), Some(Event::StageFailed)));
        assert!(step(&mut w, &r, 3).is_none());
        assert!(step(&mut w, &r, 4).is_none());
    }

    #[test]
    fn reset_clears_the_attempt() {
        let mut w = Watcher::new(Vec::new());
        let mut r = ram();
        step(&mut w, &r, 1);
        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(matches!(step(&mut w, &r, 2), Some(Event::StageFailed)));

        // A savestate load restarts the stage with the flag still set; the
        // next rise has to be detectable from a fresh baseline.
        w.reset();
        assert!(step(&mut w, &r, 1).is_none());
        r[ADDR_RETURN_TO_MAP] = 0;
        step(&mut w, &r, 2);
        r[ADDR_RETURN_TO_MAP] = 1;
        assert!(matches!(step(&mut w, &r, 3), Some(Event::StageFailed)));
    }
}
