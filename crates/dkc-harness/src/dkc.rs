//! Donkey Kong Country RAM watching. Deliberately game-specific, and
//! deliberately not shared with `smb3-harness` — Phase 2 wants two harnesses
//! written independently so the abstraction can be read off the diff rather
//! than guessed at (ADR 0012).
//!
//! # Reaching the bytes
//!
//! The SNES keeps 128 KB of work RAM at $7E:0000-$7F:FFFF, and snes9x hands
//! all of it over as `RETRO_MEMORY_SYSTEM_RAM`. That is the whole story for
//! every address below, which is the opposite of the NES harness's situation:
//! there, the interesting byte was in cartridge WRAM and only the core's
//! published memory map could reach it. Here the slice is authoritative and
//! the memory map is the fallback, used only for addresses outside WRAM.
//!
//! Addresses may be written either way round. `0x0577` is a WRAM offset;
//! `0x7E0577` is the same byte named as a full SNES address. `normalise`
//! folds the second form into the first, so `--watch` and `--addr` accept
//! whichever form a RAM map happens to use.
//!
//! # Provenance
//!
//! Not one address here has been playtested. That is the point of this
//! commit: the harness exists so the addresses can be *checked*, and every
//! one of them is a constant with a documented source and an
//! `--addr <name>=<addr>` override, so a wrong guess costs a flag rather than
//! a rebuild.
//!
//!   $7E003E level id       documented: Data Crystal DKC RAM map,
//!                          "Current level"
//!   $7E0577 lives          reported: widely-cited DKC RAM map, attributed to
//!                          TCRF / DKC Atlas. Both of those pages currently
//!                          serve unrelated content, so this could not be
//!                          confirmed at the source. Treat as a lead.
//!   $7E057B bananas        reported, same source and same caveat
//!   $7E057F KONG letters   reported, same source and same caveat
//!   $7E051B fade type      source: `Lui37/dkc1-hack` src/defines.asm, an
//!                          assembling speedrun-practice hack — so these three
//!                          are load-bearing in working code rather than
//!                          transcribed from a wiki
//!   $7E0579 pause flags    same source. Its bit 6 is the paused flag; the
//!                          hack tests it with BIT/BVS.
//!   $7E1929 accept inputs  same source. Nonzero once the level hands control
//!                          to the player, which is a plausible "stage really
//!                          started" gate.
//!   $7E0B19 player X       documented: Data Crystal, "Object 1 (DK) X"
//!   $7E0BC1 player Y       documented: Data Crystal, "Object 1 (DK) Y"
//!
//! $0577 / $057B / $057F sit four bytes apart, which reads like a stats block
//! with one entry per field (or per save slot — DKC has two contest players).
//! Sweeping `--watch 0x0570-0x05a0` is the first thing to try if any of the
//! three is wrong.
//!
//! # What has no address at all
//!
//! Taking damage, losing a Kong, picking up an animal buddy, and killing an
//! enemy have no published address that survived checking. They are declared
//! below with `at: None`, which means their detector is simply off; startup
//! says which detectors are live. Finding them is a playtest job, and
//! `ramsearch.rs` is the tool for it — `Search` for anything that behaves like
//! a counter, `Probe` for one-shot events like a hit.
//!
//! DKC may not *have* an enemy-kill counter; it has no score. If a session of
//! `Probe` reports on a stomp turns up nothing stable, the honest answer is
//! that enemy kills need the sprite table, which is a bigger job than this
//! phase.

use std::fmt;

/// SNES work RAM: $7E:0000-$7F:FFFF.
pub const WRAM_SIZE: usize = 0x2_0000;
/// Bank address WRAM starts at, for the `$7Exxxx` spelling of an offset.
pub const WRAM_BASE: usize = 0x7E_0000;

/// Folds a full SNES WRAM address into a plain offset. `None` for anything
/// outside WRAM, which `read_byte` then hands to the core's memory map.
pub fn normalise(addr: usize) -> Option<usize> {
    if addr < WRAM_SIZE {
        Some(addr)
    } else if (WRAM_BASE..WRAM_BASE + WRAM_SIZE).contains(&addr) {
        Some(addr - WRAM_BASE)
    } else {
        None
    }
}

/// Reads one byte. Prefers the WRAM slice, which covers every address this
/// harness watches; falls back to the core's published memory map so an
/// address outside WRAM (cartridge SRAM, ROM) still resolves when a core
/// published one. `None` when neither route covers `addr`.
pub fn read_byte(addr: usize, ram: &[u8]) -> Option<u8> {
    match normalise(addr) {
        Some(off) => ram.get(off).copied(),
        None => crate::libretro::read_addr(addr),
    }
}

/// Renders an address the way every other line of output does: as a full SNES
/// address, so anything printed can be pasted straight back into `--addr` or
/// `--watch`. An address outside work RAM is shown as given.
pub fn show_addr(addr: usize) -> String {
    match normalise(addr) {
        Some(off) => format!("${:06X}", off + WRAM_BASE),
        None => format!("${addr:06X}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    U8,
    U16,
}

impl Width {
    pub fn parse(s: &str) -> Result<Width, String> {
        match s {
            "u8" | "8" => Ok(Width::U8),
            "u16" | "16" => Ok(Width::U16),
            other => Err(format!("width must be u8 or u16, got `{other}`")),
        }
    }
}

impl fmt::Display for Width {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Width::U8 => write!(f, "u8"),
            Width::U16 => write!(f, "u16"),
        }
    }
}

/// One watched quantity: where it lives, and how wide it is. `at: None` means
/// the address is not known, and every detector built on it stays quiet.
#[derive(Clone, Copy)]
pub struct Field {
    pub at: Option<usize>,
    pub width: Width,
}

impl Field {
    /// The SNES is little-endian, and the 65816 runs with a 16-bit
    /// accumulator through most of DKC's game logic, so `U16` is the safer
    /// default for anything that looks like a count. If a value turns out to
    /// be a single byte with a noisy neighbour, `--addr name=ADDR:u8` narrows
    /// it without a rebuild.
    pub fn read(&self, ram: &[u8]) -> Option<u16> {
        let at = self.at?;
        match self.width {
            Width::U8 => read_byte(at, ram).map(u16::from),
            Width::U16 => {
                let lo = read_byte(at, ram)?;
                let hi = read_byte(at + 1, ram)?;
                Some(u16::from(lo) | (u16::from(hi) << 8))
            }
        }
    }
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.at {
            Some(a) => write!(f, "${:06X} {}", a + WRAM_BASE, self.width),
            None => write!(f, "unset"),
        }
    }
}

/// Declares the address table once and derives everything that has to stay in
/// step with it: the overridable field set, the per-frame snapshot, and the
/// startup listing. Adding a watched quantity is one line here.
macro_rules! ram_fields {
    ($( $name:ident, $default:expr, $width:ident, $note:literal ; )*) => {
        #[derive(Clone, Copy)]
        pub struct Addrs { $( pub $name: Field, )* }

        impl Default for Addrs {
            fn default() -> Self {
                Addrs { $( $name: Field { at: $default, width: Width::$width }, )* }
            }
        }

        impl Addrs {
            /// Applies one `--addr name=ADDR[:width]` override. `ADDR` may be
            /// a WRAM offset or a full `$7Exxxx` address; `off` unsets the
            /// field, which turns its detector off.
            pub fn set(&mut self, name: &str, at: Option<usize>, width: Option<Width>)
                -> Result<(), String>
            {
                let at = match at {
                    Some(a) => Some(normalise(a).ok_or_else(|| format!(
                        "${a:06X} is outside SNES work RAM ($7E0000-$7FFFFF)"
                    ))?),
                    None => None,
                };
                match name {
                    $( stringify!($name) => {
                        self.$name.at = at;
                        if let Some(w) = width { self.$name.width = w; }
                        Ok(())
                    } )*
                    other => Err(format!("unknown address name `{other}`; try --list-addrs")),
                }
            }

            /// Name, current setting, and where the default came from.
            pub fn list(&self) -> Vec<(&'static str, Field, &'static str)> {
                vec![ $( (stringify!($name), self.$name, $note), )* ]
            }
        }

        /// Every watched quantity as of one frame. `None` for a field whose
        /// address is unset, or whose address the core could not resolve.
        #[derive(Clone, Copy, PartialEq, Eq)]
        pub struct Snapshot { $( pub $name: Option<u16>, )* }

        impl Snapshot {
            pub fn read(ram: &[u8], a: &Addrs) -> Snapshot {
                Snapshot { $( $name: a.$name.read(ram), )* }
            }
        }
    };
}

ram_fields! {
    level_id,      Some(0x003E), U16, "Data Crystal: current level";
    lives,         Some(0x0577), U16, "reported RAM map, unconfirmed at source";
    bananas,       Some(0x057B), U16, "reported RAM map, unconfirmed at source";
    kong_letters,  Some(0x057F), U16, "reported RAM map, unconfirmed at source";
    fade_type,     Some(0x051B), U16, "dkc1-hack defines.asm";
    pause_flags,   Some(0x0579), U16, "dkc1-hack defines.asm, bit 6 = paused";
    accept_inputs, Some(0x1929), U16, "dkc1-hack defines.asm";
    player_x,      Some(0x0B19), U16, "Data Crystal: object 1 (DK) X";
    player_y,      Some(0x0BC1), U16, "Data Crystal: object 1 (DK) Y";
    kong_count,    None,         U16, "UNKNOWN - find with Probe on a hit";
    hit_timer,     None,         U8,  "UNKNOWN - find with Probe on a hit";
    enemy_kills,   None,         U16, "UNKNOWN - DKC may have no such counter";
    animal,        None,         U16, "UNKNOWN - find with Probe on a pickup";
}

impl fmt::Display for Snapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fn v(x: Option<u16>) -> String {
            match x {
                Some(n) => n.to_string(),
                None => "-".into(),
            }
        }
        write!(
            f,
            "level {}  lives {}  bananas {}  KONG {}  kongs {}  at ({},{})",
            v(self.level_id),
            v(self.lives),
            v(self.bananas),
            match self.kong_letters {
                Some(b) => kong_letters(b),
                None => "-".into(),
            },
            v(self.kong_count),
            v(self.player_x),
            v(self.player_y),
        )
    }
}

/// Renders the KONG bitfield as the letters held, e.g. `K-N-`. Which bit is
/// which letter is a guess: bit 0 is assumed to be K. If playtesting shows the
/// order reversed, this function is the only thing to change.
fn kong_letters(bits: u16) -> String {
    const LETTERS: [char; 4] = ['K', 'O', 'N', 'G'];
    LETTERS
        .iter()
        .enumerate()
        .map(|(i, c)| if bits & (1 << i) != 0 { *c } else { '-' })
        .collect()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Bananas(u16),
    KongLetter(char),
    ExtraLife,
    AnimalBuddy(u16),
}

impl fmt::Display for Item {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Item::Bananas(n) => write!(f, "{n} banana(s)"),
            Item::KongLetter(c) => write!(f, "letter {c}"),
            Item::ExtraLife => write!(f, "extra life"),
            Item::AnimalBuddy(v) => write!(f, "animal buddy {v:04X}"),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FailReason {
    /// A life was lost.
    Death,
    /// The level ended and no death explained it — but no death detector was
    /// running either, so a clear cannot be claimed.
    LeftStage,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Damage,
    Death,
    EnemyKilled(u16),
    ItemGained(Item),
    /// `at` is the frame the stage actually ended, which is not always the
    /// frame the event is emitted — see `result_grace`.
    StageComplete { at: u64 },
    StageFailed { at: u64, reason: FailReason },
}

/// Default for `Watcher::result_grace`, two seconds at 60 Hz.
pub const DEFAULT_RESULT_GRACE_FRAMES: u64 = 120;

pub struct Watcher {
    addrs: Addrs,
    prev: Option<Snapshot>,
    /// Level id at stage entry. The stage-boundary logic is relative to this,
    /// so it is captured on the first tick after a state load rather than
    /// hardcoded — the same reason the NES harness takes its stage label from
    /// the entry point rather than from RAM.
    entry_level: Option<u16>,
    /// Set once this attempt has produced a StageComplete or StageFailed, so
    /// the level unload that follows a death does not fire a second event.
    resolved: bool,
    /// A level change that has not been ruled a clear yet, and the frame it
    /// happened on. See `result_grace`.
    pending_end: Option<u64>,
    /// How long to wait after the level unloads before calling it a clear.
    ///
    /// A death and the unload it causes are not simultaneous, and with every
    /// address here unverified we do not know which lands first. If the life
    /// counter drops *before* the level id moves, `resolved` alone is enough.
    /// If it drops *after*, an immediate verdict would score a death as a
    /// clear — the one failure mode ADR 0003 calls the worst available.
    ///
    /// So a clear is held for this many frames and cancelled by a death that
    /// arrives inside the window. The frame reported is still the frame the
    /// level ended, so the grace costs latency in the console and nothing in
    /// the measurement (ADR 0005).
    result_grace: u64,
    /// Extra addresses to report on change, from `--watch`.
    watched: Vec<usize>,
    prev_watched: Vec<Option<u8>>,
}

impl Watcher {
    pub fn new(addrs: Addrs, watched: Vec<usize>, result_grace: u64) -> Self {
        Watcher {
            addrs,
            prev: None,
            entry_level: None,
            resolved: false,
            pending_end: None,
            result_grace,
            prev_watched: vec![None; watched.len()],
            watched,
        }
    }

    /// Called after a savestate load or a core reset: the stage restarts, so
    /// nothing from before is meaningful — including the entry level id.
    pub fn reset(&mut self) {
        self.prev = None;
        self.entry_level = None;
        self.resolved = false;
        self.pending_end = None;
        self.prev_watched.fill(None);
    }

    /// Reads RAM, prints what changed, and returns everything that fired this
    /// frame. `frame` counts `retro_run` calls since stage entry (ADR 0005:
    /// timing is measured in emulated frames, never wall-clock).
    pub fn tick(&mut self, ram: &[u8], frame: u64) -> Vec<Event> {
        let cur = Snapshot::read(ram, &self.addrs);
        self.report_watched(ram, frame);

        let prev = match self.prev {
            Some(p) => p,
            None => {
                // First frame of an attempt: state the baseline, then deltas
                // only. The level id read here is what "left the stage" means
                // for the rest of the attempt.
                println!("[ram] frame {frame}  {cur}");
                self.entry_level = cur.level_id;
                self.prev = Some(cur);
                return Vec::new();
            }
        };

        let mut events = Vec::new();

        // ---- death -------------------------------------------------------
        // A life lost is the one signal that means the same thing everywhere
        // in DKC: pit, water, enemy contact with a single Kong. It fires at
        // the moment of death rather than when the map reloads, which is what
        // makes it worth using over the level unload — the same argument the
        // NES harness makes for the card.
        let died = match (prev.lives, cur.lives) {
            (Some(p), Some(c)) if c < p => {
                println!("[event] frame {frame}  DEATH  lives {p} -> {c}");
                events.push(Event::Death);
                true
            }
            _ => false,
        };
        if died && !self.resolved {
            // A death cancels a clear that was still being held: the level
            // change a moment ago was this death on its way back to the map.
            // The failure is timed to the death itself unless the level had
            // already gone, in which case that boundary is the honest frame.
            let at = self.pending_end.take().unwrap_or(frame);
            events.push(Event::StageFailed { at, reason: FailReason::Death });
            self.resolved = true;
        }

        // ---- damage ------------------------------------------------------
        // Losing the lead Kong while the other is still alive: a hit that did
        // not cost a life. Both detectors are off until playtesting finds an
        // address. A death already reported this frame suppresses them, since
        // dying with one Kong drops the count to zero too.
        if !died {
            if let (Some(p), Some(c)) = (prev.kong_count, cur.kong_count) {
                if c < p {
                    println!("[event] frame {frame}  DAMAGE  kongs {p} -> {c}");
                    events.push(Event::Damage);
                }
            }
            // Rising from zero rather than merely nonzero, so a timer still
            // counting down from an earlier hit does not retrigger every frame.
            if let (Some(0), Some(c)) = (prev.hit_timer, cur.hit_timer) {
                if c != 0 {
                    println!("[event] frame {frame}  DAMAGE  hit timer set to {c}");
                    events.push(Event::Damage);
                }
            }
        }

        // ---- enemy kills -------------------------------------------------
        if let (Some(p), Some(c)) = (prev.enemy_kills, cur.enemy_kills) {
            if c > p {
                let n = c - p;
                println!("[event] frame {frame}  ENEMY KILLED  x{n} (total {c})");
                events.push(Event::EnemyKilled(n));
            }
        }

        // ---- items -------------------------------------------------------
        if let (Some(p), Some(c)) = (prev.bananas, cur.bananas) {
            if c > p {
                let n = c - p;
                println!("[event] frame {frame}  ITEM  {n} banana(s) ({p} -> {c})");
                events.push(Event::ItemGained(Item::Bananas(n)));
            }
            // A drop is the hundredth banana rolling over into a life, not a
            // loss; the extra life below reports that.
        }
        if let (Some(p), Some(c)) = (prev.kong_letters, cur.kong_letters) {
            let gained = c & !p;
            if gained != 0 {
                for (i, letter) in ['K', 'O', 'N', 'G'].into_iter().enumerate() {
                    if gained & (1 << i) != 0 {
                        println!("[event] frame {frame}  ITEM  KONG letter {letter}");
                        events.push(Event::ItemGained(Item::KongLetter(letter)));
                    }
                }
            }
        }
        if let (Some(p), Some(c)) = (prev.lives, cur.lives) {
            if c > p {
                println!("[event] frame {frame}  ITEM  extra life ({p} -> {c})");
                events.push(Event::ItemGained(Item::ExtraLife));
            }
        }
        if let (Some(p), Some(c)) = (prev.animal, cur.animal) {
            if c != p && c != 0 {
                println!("[event] frame {frame}  ITEM  animal buddy {p:04X} -> {c:04X}");
                events.push(Event::ItemGained(Item::AnimalBuddy(c)));
            }
        }

        // ---- stage boundary ----------------------------------------------
        // The level id leaving its entry value is the only end-of-stage signal
        // there is so far, and it is a late one: it moves when the level
        // unloads, not when the exit is touched. That is the same gap the NES
        // harness had before the card was found, and the same fix applies —
        // find the earlier signal. Until then a clear is timed to the unload,
        // which puts several seconds of hand-off inside the reported time.
        //
        // It is also not specific to *finishing*. A bonus barrel changes the
        // level id too, so a bonus room currently looks exactly like a clear.
        // The re-entry branch below is what notices that after the fact.
        if let (Some(entry), Some(c)) = (self.entry_level, cur.level_id) {
            let left = c != entry;
            let was_inside = prev.level_id == Some(entry);

            if left && was_inside && !self.resolved && self.pending_end.is_none() {
                println!("[event] frame {frame}  LEVEL ID  {entry:04X} -> {c:04X}");
                self.pending_end = Some(frame);
            }

            // Back inside the entry level: the player re-entered from the map,
            // or came back out of a bonus room. Either way the next death or
            // clear belongs to a fresh attempt. Without this the first result
            // latches for the whole session — the bug the NES harness had to
            // fix twice.
            if !left && prev.level_id != Some(entry) {
                if self.resolved {
                    println!(
                        "[stage] back in level {entry:04X} — if that result came from a \
                         bonus barrel rather than the exit, it was a false positive"
                    );
                }
                println!("[stage] attempt reset at frame {frame}");
                self.resolved = false;
                self.pending_end = None;
            }
        }

        // A held clear that no death arrived to cancel. Emitted late, but
        // timed to the frame the stage actually ended.
        if let Some(at) = self.pending_end {
            if !self.resolved && frame.saturating_sub(at) >= self.result_grace {
                if self.addrs.lives.at.is_none() {
                    // No death detector was running, so "no death was seen"
                    // proves nothing. Refuse to claim a clear.
                    events.push(Event::StageFailed { at, reason: FailReason::LeftStage });
                } else {
                    events.push(Event::StageComplete { at });
                }
                self.resolved = true;
                self.pending_end = None;
            }
        }

        // Anything that moved and is worth a running log line, printed once
        // per change rather than per frame. Position is excluded: it changes
        // every frame the player moves.
        if (cur.level_id, cur.lives, cur.bananas, cur.kong_letters, cur.kong_count)
            != (prev.level_id, prev.lives, prev.bananas, prev.kong_letters, prev.kong_count)
        {
            println!("[ram] frame {frame}  {cur}");
        }

        self.prev = Some(cur);
        events
    }

    /// Print every field and every `--watch` address unconditionally (F5).
    pub fn dump(&self, ram: &[u8], frame: u64) {
        println!("[ram] frame {frame}  dump");
        println!("        {}", Snapshot::read(ram, &self.addrs));
        for (name, field, note) in self.addrs.list() {
            let shown = format!("{field}");
            match field.read(ram) {
                Some(v) => println!("        {name:<14} {shown:<18} = {v:5} ({v:04X})  {note}"),
                None => println!("        {name:<14} {shown:<18} =     -         {note}"),
            }
        }
        for &addr in &self.watched {
            let at = show_addr(addr);
            match read_byte(addr, ram) {
                Some(b) => println!("        {at} = {b:02X} ({b})"),
                None => println!("        {at} = <unmapped>"),
            }
        }
    }

    fn report_watched(&mut self, ram: &[u8], frame: u64) {
        for (i, &addr) in self.watched.iter().enumerate() {
            let cur = read_byte(addr, ram);
            let Some(b) = cur else { continue };
            let at = show_addr(addr);
            match self.prev_watched[i] {
                // The first read after a reset states the baseline rather than
                // reporting a change against a zero that was never there.
                None => println!("[watch] frame {frame}  {at} = {b:02X} ({b})  baseline"),
                Some(was) if was != b => {
                    println!("[watch] frame {frame}  {at} = {b:02X} ({b})  was {was:02X}")
                }
                Some(_) => continue,
            }
            self.prev_watched[i] = cur;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A whole 128 KB of WRAM, which is what snes9x hands over. `read_byte`
    /// prefers this slice, so no memory map and no core is needed.
    fn wram() -> Vec<u8> {
        vec![0u8; WRAM_SIZE]
    }

    /// Most tests are about *which* verdict fires, not about the grace window,
    /// so they run with no grace and get their answer on the same tick.
    fn watcher() -> Watcher {
        Watcher::new(Addrs::default(), Vec::new(), 0)
    }

    /// Writes a value at a field's address, in that field's width.
    fn put(ram: &mut [u8], field: Field, v: u16) {
        let at = field.at.expect("test wrote to an unset field");
        match field.width {
            Width::U8 => ram[at] = v as u8,
            Width::U16 => {
                ram[at] = v as u8;
                ram[at + 1] = (v >> 8) as u8;
            }
        }
    }

    fn completed(events: &[Event]) -> Option<u64> {
        events.iter().find_map(|e| match e {
            Event::StageComplete { at } => Some(*at),
            _ => None,
        })
    }

    fn failed(events: &[Event]) -> Option<(u64, FailReason)> {
        events.iter().find_map(|e| match e {
            Event::StageFailed { at, reason } => Some((*at, *reason)),
            _ => None,
        })
    }

    /// Runs quiet frames so a grace window can expire.
    fn idle(w: &mut Watcher, ram: &[u8], from: u64, to: u64) -> Vec<Event> {
        let mut out = Vec::new();
        for f in from..=to {
            out.extend(w.tick(ram, f));
        }
        out
    }

    #[test]
    fn full_snes_addresses_fold_onto_wram_offsets() {
        assert_eq!(normalise(0x7E_0577), Some(0x0577));
        assert_eq!(normalise(0x0577), Some(0x0577));
        // The upper half of WRAM is $7F:0000 and up.
        assert_eq!(normalise(0x7F_0000), Some(0x1_0000));
        // Cartridge space is not WRAM; the memory map answers for it, not us.
        assert_eq!(normalise(0x70_0000), None);
    }

    #[test]
    fn sixteen_bit_fields_read_little_endian() {
        let mut r = wram();
        r[0x0577] = 0x34;
        r[0x0578] = 0x12;
        let wide = Field { at: Some(0x0577), width: Width::U16 };
        assert_eq!(wide.read(&r), Some(0x1234));
        let narrow = Field { at: Some(0x0577), width: Width::U8 };
        assert_eq!(narrow.read(&r), Some(0x34));
    }

    #[test]
    fn an_unset_field_reads_as_none() {
        let f = Field { at: None, width: Width::U16 };
        assert_eq!(f.read(&wram()), None);
    }

    #[test]
    fn first_tick_is_baseline_only() {
        let mut w = watcher();
        assert!(w.tick(&wram(), 1).is_empty());
    }

    #[test]
    fn losing_a_life_is_a_death_and_fails_the_stage() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        w.tick(&r, 1);
        put(&mut r, a.lives, 4);
        let ev = w.tick(&r, 2);
        assert!(ev.contains(&Event::Death));
        assert_eq!(failed(&ev), Some((2, FailReason::Death)));
    }

    /// The level unloads after the death that caused it. That unload must not
    /// produce a second verdict.
    #[test]
    fn the_level_unload_after_a_death_is_not_a_clear() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);

        put(&mut r, a.lives, 4);
        assert!(w.tick(&r, 2).contains(&Event::Death));

        put(&mut r, a.level_id, 0x0000);
        let ev = idle(&mut w, &r, 3, 300);
        assert_eq!(completed(&ev), None);
        assert_eq!(failed(&ev), None);
    }

    /// The other order. If the game moves the level id first and only then
    /// decrements lives, an immediate verdict would score the death as a win —
    /// which is why a clear is held rather than emitted on the spot.
    #[test]
    fn a_death_just_after_the_level_unloads_still_fails() {
        let a = Addrs::default();
        let mut w = Watcher::new(a, Vec::new(), 120);
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);

        put(&mut r, a.level_id, 0x0000);
        let ev = idle(&mut w, &r, 2, 40);
        assert_eq!(completed(&ev), None, "a clear inside the grace window is not final");

        put(&mut r, a.lives, 4);
        let ev = w.tick(&r, 41);
        // Timed to the boundary, not to the frame the life counter caught up.
        assert_eq!(failed(&ev), Some((2, FailReason::Death)));

        // And the held clear is gone for good, not merely postponed.
        let ev = idle(&mut w, &r, 42, 400);
        assert_eq!(completed(&ev), None);
    }

    /// Once the window passes with no death, the clear stands — and it is
    /// reported against the frame the stage ended, not the frame the verdict
    /// was finally emitted. ADR 0005: the number that travels is a frame count
    /// from stage entry.
    #[test]
    fn a_held_clear_is_timed_to_the_boundary_not_to_the_verdict() {
        let a = Addrs::default();
        let mut w = Watcher::new(a, Vec::new(), 120);
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);

        put(&mut r, a.level_id, 0x0000);
        let ev = idle(&mut w, &r, 2, 121);
        assert_eq!(completed(&ev), None);
        let ev = idle(&mut w, &r, 122, 122);
        assert_eq!(completed(&ev), Some(2));
    }

    #[test]
    fn leaving_the_level_with_no_death_completes_the_stage() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);
        put(&mut r, a.level_id, 0x0000);
        assert_eq!(completed(&w.tick(&r, 500)), Some(500));
    }

    /// Regression guard borrowed from the NES harness: a result must not latch
    /// for the whole session. Re-entering the level starts over.
    #[test]
    fn a_second_attempt_reports_its_own_result() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);

        put(&mut r, a.level_id, 0x0000);
        assert_eq!(completed(&w.tick(&r, 2)), Some(2));

        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 3);
        put(&mut r, a.lives, 4);
        assert_eq!(failed(&w.tick(&r, 4)), Some((4, FailReason::Death)));
    }

    /// A bonus barrel moves the level id as surely as the exit does, so the
    /// trip into a bonus room currently reads as a clear. Coming back out has
    /// to reopen the attempt — otherwise the death that follows is never
    /// reported at all.
    #[test]
    fn returning_from_a_bonus_room_reopens_the_attempt() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);

        put(&mut r, a.level_id, 0x0042);
        assert_eq!(completed(&w.tick(&r, 60)), Some(60));

        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 400);
        put(&mut r, a.lives, 4);
        assert_eq!(failed(&w.tick(&r, 500)), Some((500, FailReason::Death)));
    }

    #[test]
    fn a_held_level_change_fires_once() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);
        put(&mut r, a.level_id, 0x0000);
        assert_eq!(completed(&w.tick(&r, 2)), Some(2));
        assert!(w.tick(&r, 3).is_empty());
        assert!(w.tick(&r, 4).is_empty());
    }

    /// With no lives address configured there is no death detector, so an
    /// unexplained level change cannot be claimed as a clear.
    #[test]
    fn without_a_death_detector_a_level_change_is_not_claimed_as_a_clear() {
        let mut a = Addrs::default();
        a.set("lives", None, None).unwrap();
        let mut w = Watcher::new(a, Vec::new(), 0);
        let mut r = wram();
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);
        put(&mut r, a.level_id, 0x0000);
        assert_eq!(failed(&w.tick(&r, 2)), Some((2, FailReason::LeftStage)));
    }

    #[test]
    fn bananas_and_letters_report_as_items() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        w.tick(&r, 1);

        put(&mut r, a.bananas, 3);
        assert!(w.tick(&r, 2).contains(&Event::ItemGained(Item::Bananas(3))));

        // Bit 0 then bit 2: K, then N. Only the newly set bit reports.
        put(&mut r, a.kong_letters, 0b0001);
        assert!(w.tick(&r, 3).contains(&Event::ItemGained(Item::KongLetter('K'))));
        put(&mut r, a.kong_letters, 0b0101);
        let ev = w.tick(&r, 4);
        assert!(ev.contains(&Event::ItemGained(Item::KongLetter('N'))));
        assert!(!ev.contains(&Event::ItemGained(Item::KongLetter('K'))));
    }

    #[test]
    fn gaining_a_life_is_an_item_not_a_death() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 4);
        w.tick(&r, 1);
        put(&mut r, a.lives, 5);
        let ev = w.tick(&r, 2);
        assert!(ev.contains(&Event::ItemGained(Item::ExtraLife)));
        assert!(!ev.contains(&Event::Death));
    }

    /// Damage detectors stay off until an address is found, and turning one on
    /// is a flag rather than a rebuild.
    #[test]
    fn damage_reports_once_an_address_is_supplied() {
        let mut a = Addrs::default();
        let mut w = Watcher::new(a, Vec::new(), 0);
        let mut r = wram();
        r[0x08A5] = 2;
        w.tick(&r, 1);
        r[0x08A5] = 1;
        assert!(w.tick(&r, 2).is_empty(), "no detector without an address");

        a.set("kong_count", Some(0x7E_08A5), Some(Width::U8)).unwrap();
        let mut w = Watcher::new(a, Vec::new(), 0);
        r[0x08A5] = 2;
        w.tick(&r, 1);
        r[0x08A5] = 1;
        assert!(w.tick(&r, 2).contains(&Event::Damage));
    }

    /// Dying with one Kong drops the count to zero as well; that is one event,
    /// not damage plus a death.
    #[test]
    fn a_death_suppresses_the_damage_it_also_causes() {
        let mut a = Addrs::default();
        a.set("kong_count", Some(0x08A5), Some(Width::U8)).unwrap();
        let mut w = Watcher::new(a, Vec::new(), 0);
        let mut r = wram();
        put(&mut r, a.lives, 5);
        r[0x08A5] = 1;
        w.tick(&r, 1);

        put(&mut r, a.lives, 4);
        r[0x08A5] = 0;
        let ev = w.tick(&r, 2);
        assert!(ev.contains(&Event::Death));
        assert!(!ev.contains(&Event::Damage));
    }

    #[test]
    fn enemy_kills_report_once_a_counter_is_supplied() {
        let mut a = Addrs::default();
        a.set("enemy_kills", Some(0x1000), Some(Width::U16)).unwrap();
        let mut w = Watcher::new(a, Vec::new(), 0);
        let mut r = wram();
        w.tick(&r, 1);
        r[0x1000] = 2;
        assert!(w.tick(&r, 2).contains(&Event::EnemyKilled(2)));
    }

    #[test]
    fn a_reset_starts_a_fresh_attempt() {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);
        put(&mut r, a.level_id, 0x0000);
        assert_eq!(completed(&w.tick(&r, 2)), Some(2));

        // A savestate load drops us back into the level; the entry level has
        // to be re-read rather than remembered from before the load.
        w.reset();
        put(&mut r, a.level_id, 0x0007);
        assert!(w.tick(&r, 1).is_empty());
        put(&mut r, a.level_id, 0x0000);
        assert_eq!(completed(&w.tick(&r, 2)), Some(2));
    }

    /// A state load lands mid-grace often enough to be worth pinning: the held
    /// clear belongs to the attempt that was thrown away.
    #[test]
    fn a_reset_discards_a_held_clear() {
        let a = Addrs::default();
        let mut w = Watcher::new(a, Vec::new(), 120);
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, 0x0007);
        w.tick(&r, 1);
        put(&mut r, a.level_id, 0x0000);
        w.tick(&r, 2);

        w.reset();
        put(&mut r, a.level_id, 0x0007);
        let ev = idle(&mut w, &r, 1, 400);
        assert_eq!(completed(&ev), None);
    }

    #[test]
    fn unknown_address_names_are_rejected() {
        let mut a = Addrs::default();
        assert!(a.set("bananas", Some(0x1234), None).is_ok());
        assert!(a.set("banannas", Some(0x1234), None).is_err());
        // Outside WRAM entirely: the override is refused rather than silently
        // pointing a detector at nothing.
        assert!(a.set("bananas", Some(0x70_0000), None).is_err());
    }

    /// Output has to paste back into a flag, so a WRAM offset and its full
    /// address print identically however the user typed it.
    #[test]
    fn addresses_print_in_one_canonical_form() {
        assert_eq!(show_addr(0x0577), "$7E0577");
        assert_eq!(show_addr(0x7E_0577), "$7E0577");
        assert_eq!(show_addr(0x1_0000), "$7F0000");
        // Outside work RAM there is nothing to canonicalise to.
        assert_eq!(show_addr(0x70_0000), "$700000");
    }

    #[test]
    fn kong_letter_rendering_is_stable() {
        assert_eq!(kong_letters(0b0000), "----");
        assert_eq!(kong_letters(0b0101), "K-N-");
        assert_eq!(kong_letters(0b1111), "KONG");
    }
}
