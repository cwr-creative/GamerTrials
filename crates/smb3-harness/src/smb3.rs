//! SMB3 RAM watching. Deliberately game-specific — Phase 1 has one game, and
//! ADR 0012 says the interface gets extracted once DKC exists to abstract
//! against, not before.
//!
//! Addresses are into the NES 2 KB internal RAM (`RETRO_MEMORY_SYSTEM_RAM`,
//! $0000-$07FF). Cartridge WRAM at $6000-$7FFF is a different libretro region
//! and nothing here needs it yet.
//!
//! Provenance matters:
//!
//!   $0736 lives            verified (playtested against this harness)
//!   $0727 world - 1        verified (playtested against this harness)
//!   $0014 return-to-map    verified (playtested against this harness)
//!   $0078 / $0079          verified (playtested against this harness)
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

/// The return-to-map flag has to have been clear this long before a rise counts
/// as leaving a level. Without it, map-screen chatter reads as a stage end.
const MIN_FRAMES_IN_LEVEL: u64 = 120;

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
}

impl Snapshot {
    pub fn read(ram: &[u8]) -> Option<Snapshot> {
        // Every address above is inside the 2 KB internal RAM. A core handing
        // back something smaller is not the NES core we think it is.
        if ram.len() < 0x0800 {
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
            "world {}  level map({:02X},{:02X}) set {:02X}  lives {}  powerup {:02X}",
            self.world_number(),
            self.map_x,
            self.map_y,
            self.object_set,
            self.lives,
            self.powerup
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
    /// Consecutive frames the return-to-map flag has read zero.
    zero_run: u64,
    /// Extra addresses to report on change, from `--watch`.
    watched: Vec<usize>,
    prev_watched: Vec<u8>,
}

impl Watcher {
    pub fn new(watched: Vec<usize>) -> Self {
        Watcher {
            prev: None,
            zero_run: 0,
            prev_watched: vec![0; watched.len()],
            watched,
        }
    }

    /// Called after a savestate load: the stage restarts, so nothing carried
    /// over from before the load is meaningful.
    pub fn reset(&mut self) {
        self.prev = None;
        self.zero_run = 0;
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
                self.zero_run = if cur.return_to_map == 0 { 1 } else { 0 };
                return None;
            }
        };

        if (cur.world, cur.map_x, cur.map_y, cur.lives, cur.object_set)
            != (prev.world, prev.map_x, prev.map_y, prev.lives, prev.object_set)
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

        let rose = prev.return_to_map == 0 && cur.return_to_map != 0;
        let event = if rose && self.zero_run >= MIN_FRAMES_IN_LEVEL {
            // Leaving a level covers both winning and dying. A life lost is the
            // one signal that separates them without another unverified address.
            if cur.lives < prev.lives {
                Some(Event::StageFailed)
            } else {
                Some(Event::StageComplete)
            }
        } else {
            None
        };

        self.zero_run = if cur.return_to_map == 0 {
            self.zero_run + 1
        } else {
            0
        };
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
            if let Some(b) = ram.get(addr) {
                println!("        ${addr:04X} = {b:02X} ({b})");
            }
        }
    }

    fn report_watched(&mut self, ram: &[u8], frame: u64) {
        for (i, &addr) in self.watched.iter().enumerate() {
            let Some(&b) = ram.get(addr) else { continue };
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
