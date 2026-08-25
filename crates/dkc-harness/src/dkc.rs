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
//!   $7E003E level id       confirmed: the RainbowZ Editor transcribes the
//!                          game's own bound lookup as `LDA $3E` followed by
//!                          `levelCode << 1`, so this is read off working code
//!                          rather than a wiki. `levels.rs` maps its 230 values
//!                          to names, which is what makes a stage boundary
//!                          classifiable.
//!   $7E0577 lives          reported: widely-cited DKC RAM map, attributed to
//!                          TCRF / DKC Atlas. Both of those pages currently
//!                          serve unrelated content, so this could not be
//!                          confirmed at the source. Treat as a lead.
//!   $7E057B bananas        reported, same source and same caveat
//!   $7E057F KONG letters   reported, same source and same caveat
//!   $7E051B fade type      source: `Lui37/dkc1-hack` src/defines.asm, an
//!                          assembling speedrun-practice hack — so these three
//!                          are load-bearing in working code rather than
//!                          transcribed from a wiki. This is the fade-to-black
//!                          speed the game sets on a scene change, and it is
//!                          set to STAGE_COMPLETE_FADE (0x82) on the way out of
//!                          a stage. Playtested: a death produces no fade at
//!                          all, but entering a bonus room produces the same
//!                          0x82 as clearing the stage does. So the value says
//!                          "the stage is being left", not "the stage was
//!                          cleared" — the level code that follows says which,
//!                          see `Watcher::tick`.
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

use crate::levels;

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
    fade_type,     Some(0x051B), U8,  "dkc1-hack defines.asm; 0x82 = stage complete";
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
            "level {}  lives {}  bananas {}  KONG {}  kongs {}  fade {}  at ({},{})",
            v(self.level_id),
            v(self.lives),
            v(self.bananas),
            match self.kong_letters {
                Some(b) => kong_letters(b),
                None => "-".into(),
            },
            v(self.kong_count),
            match self.fade_type {
                Some(f) => format!("{f:02X}"),
                None => "-".into(),
            },
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
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Damage,
    Death,
    EnemyKilled(u16),
    ItemGained(Item),
    /// A transition out of the stage has started. Reported on its own because
    /// it is the frame a clear is *timed* to, but it is not a verdict: the
    /// level code that follows says whether this was the exit or a bonus room.
    Fade { at: u64 },
    StageComplete { at: u64 },
    StageFailed { at: u64, reason: FailReason },
}

/// The fade-to-black speed the game sets at `fade_type` on the scene change
/// that takes the player out of a stage.
///
/// A death causes no fade at all, so this can never fire from one — which is
/// what makes it a good *timing* signal and why no grace window is needed. It
/// is not a verdict on its own, though: entering a bonus barrel sets the same
/// value as reaching the exit. The fade says *when* the stage was left; the
/// level code it lands on says *whether* it was left for good.
pub const STAGE_COMPLETE_FADE: u16 = 0x82;

/// How long after a fade to keep waiting for the level code to catch up.
///
/// The fade starts the transition and the level code changes when the next
/// room loads, a little later. That gap is the whole reason the fade is worth
/// having — it is the honest frame the stage ended — but a fade with no level
/// change behind it means something else happened, and the harness should stop
/// holding it open. Two seconds at 60 Hz is far longer than the hand-off.
pub const DEFAULT_TRANSITION_WINDOW_FRAMES: u64 = 120;

pub struct Watcher {
    addrs: Addrs,
    prev: Option<Snapshot>,
    /// Set once a death has failed this attempt, so one death does not report
    /// twice. Deliberately does *not* gate the fade counter below: a latch
    /// over the fade is what made the bonus-room behaviour invisible, because
    /// the first fade silenced every one after it.
    failed: bool,
    /// The stage this attempt is in, as a level *family* — every entrance,
    /// checkpoint and bonus room of one level answers the same name. Captured
    /// on the first tick after a state load rather than hardcoded, for the
    /// same reason the stage label comes from the savestate: stage identity
    /// belongs to the entry point.
    entry_family: Option<&'static str>,
    /// A fade seen with no level change behind it yet, and the frame it
    /// happened on. That frame is what a clear is timed to.
    pending_fade: Option<u64>,
    /// How long to hold `pending_fade` open waiting for the level code.
    transition_window: u64,
    /// Extra addresses to report on change, from `--watch`.
    watched: Vec<usize>,
    prev_watched: Vec<Option<u8>>,
}

impl Watcher {
    pub fn new(addrs: Addrs, watched: Vec<usize>, transition_window: u64) -> Self {
        Watcher {
            addrs,
            prev: None,
            failed: false,
            entry_family: None,
            pending_fade: None,
            transition_window,
            prev_watched: vec![None; watched.len()],
            watched,
        }
    }

    /// Called after a savestate load or a core reset: the stage restarts, so
    /// nothing from before is meaningful — including which stage it was.
    pub fn reset(&mut self) {
        self.prev = None;
        self.failed = false;
        self.entry_family = None;
        self.pending_fade = None;
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
                // only. The level code read here is what "this stage" means
                // for the rest of the attempt.
                println!("[ram] frame {frame}  {cur}");
                self.entry_family = cur.level_id.and_then(levels::family);
                match (cur.level_id, self.entry_family) {
                    (Some(c), Some(fam)) => {
                        println!("[stage] in {} — stage is `{fam}`", levels::show(c))
                    }
                    (Some(c), None) => println!(
                        "[stage] {} is not in the level table, so leaving it cannot be \
                         detected; no stage-complete detector this attempt",
                        levels::show(c)
                    ),
                    (None, _) => println!(
                        "[stage] no level_id address, so no stage-complete detector"
                    ),
                }
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
        if died && !self.failed {
            events.push(Event::StageFailed { at: frame, reason: FailReason::Death });
            self.failed = true;
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

        // ---- leaving the stage ---------------------------------------------
        // $7E051B is the fade-to-black speed, set to STAGE_COMPLETE_FADE on
        // the scene change that takes the player out of a stage. A death
        // produces no fade, so this edge can never fire from one — no grace
        // window, no ordering to race, verdict on the frame it is seen.
        //
        // What it cannot do is say what kind of transition it is — a bonus
        // barrel raises it exactly as the exit does.
        //
        // The level code at $7E003E is the *classification*. It changes when
        // the next room loads, a little after the fade, and where it lands
        // says whether the stage was really left: a bonus room, a checkpoint
        // or a return all stay inside the stage's level family, and anything
        // else is outside it.
        //
        // Pairing them gives a verdict that does not care about the route —
        // the point of the exercise, since the previous fade-counting scheme
        // silently depended on whether a player took the bonus barrels.
        if let (Some(p), Some(c)) = (prev.fade_type, cur.fade_type) {
            if c == STAGE_COMPLETE_FADE && p != STAGE_COMPLETE_FADE && self.pending_fade.is_none() {
                println!(
                    "[event] frame {frame}  FADE  {p:02X} -> {c:02X}  — leaving the stage, \
                     waiting for the level code to say where to"
                );
                events.push(Event::Fade { at: frame });
                self.pending_fade = Some(frame);
            }
        }

        if let (Some(entry), Some(pc), Some(cc)) = (self.entry_family, prev.level_id, cur.level_id) {
            if pc != cc {
                // A side room is somewhere the player comes back from. A
                // level's own bonus rooms share its family and are carried by
                // the family check alone; the shared animal-token rooms belong
                // to no level, which is what `is_side_room` is really for.
                let stayed = levels::family(cc) == Some(entry) || levels::is_side_room(cc);
                println!("[stage] level {} -> {}", levels::show(pc), levels::show(cc));

                match (stayed, self.pending_fade.take()) {
                    (true, _) => println!("[stage] still inside `{entry}` — attempt continues"),
                    // Timed to the fade, not to this frame: the fade is when
                    // the stage actually ended, and the level code catching up
                    // is hand-off (ADR 0005).
                    (false, Some(at)) if !self.failed => {
                        events.push(Event::StageComplete { at })
                    }
                    // Left the stage with no fade behind it. A death reload
                    // looks like this, and so would anything else that moves
                    // the level code without a transition — neither is a
                    // clear, and refusing to guess is the conservative answer.
                    (false, None) => println!(
                        "[stage] left `{entry}` with no fade — not scoring this as a clear"
                    ),
                    (false, Some(_)) => {}
                }
            }
        }

        // A fade no level change ever explained. Letting it stand would mean
        // the next level change, whenever it came, got timed to a transition
        // it had nothing to do with.
        if let Some(at) = self.pending_fade {
            if frame.saturating_sub(at) >= self.transition_window {
                println!(
                    "[stage] the fade at frame {at} was not followed by a level change; \
                     discarding it"
                );
                self.pending_fade = None;
            }
        }

        // Anything that moved and is worth a running log line, printed once
        // per change rather than per frame. Position is excluded: it changes
        // every frame the player moves.
        if (cur.level_id, cur.lives, cur.bananas, cur.kong_letters, cur.kong_count, cur.fade_type)
            != (prev.level_id, prev.lives, prev.bananas, prev.kong_letters, prev.kong_count, prev.fade_type)
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

    fn watcher() -> Watcher {
        Watcher::new(Addrs::default(), Vec::new(), DEFAULT_TRANSITION_WINDOW_FRAMES)
    }

    // Real level codes, so the tests exercise the same table the harness does.
    const JUNGLE_START: u16 = 0x16; // Jungle Hijinx (level start)
    const JUNGLE_BONUS_1: u16 = 0x06; // Jungle Hijinx - Bonus 1
    const JUNGLE_FROM_BONUS_1: u16 = 0x08; // Jungle Hijinx (from Bonus 1)
    const JUNGLE_FROM_SAVE: u16 = 0x25; // Jungle Hijinx (from Save)
    const REPTILE_START: u16 = 0x01; // Reptile Rumble (level start) — another stage
    const WINKY_BONUS: u16 = 0xD3; // shared animal room, in no level's family

    /// Starts an attempt in `code` and returns a watcher past its baseline
    /// tick, which is where `entry_family` is captured.
    fn in_stage(code: u16) -> (Watcher, Vec<u8>) {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, code);
        w.tick(&r, 1);
        (w, r)
    }

    /// One rising edge to `STAGE_COMPLETE_FADE` and back down, the way a real
    /// transition does. Timing only — on its own this decides nothing.
    fn fade(w: &mut Watcher, ram: &mut [u8], at: u64) -> Vec<Event> {
        let a = Addrs::default();
        put(ram, a.fade_type, STAGE_COMPLETE_FADE);
        let ev = w.tick(ram, at);
        put(ram, a.fade_type, 0);
        ev
    }

    /// The level code catching up a few frames after the fade, which is what
    /// the real game does and what the verdict is actually read from.
    fn land_on(w: &mut Watcher, ram: &mut [u8], code: u16, at: u64) -> Vec<Event> {
        let a = Addrs::default();
        put(ram, a.level_id, code);
        w.tick(ram, at)
    }

    fn faded(events: &[Event]) -> Vec<u64> {
        events
            .iter()
            .filter_map(|e| match e {
                Event::Fade { at } => Some(*at),
                _ => None,
            })
            .collect()
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

    /// Runs a range of quiet frames, for confirming nothing fires again.
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

    /// A death produces no fade, so the timing half never arms — and the
    /// level reload it causes then arrives with nothing behind it. That is the
    /// case the old level-id signal could score as a clear.
    #[test]
    fn a_death_and_its_level_reload_are_never_a_clear() {
        let (mut w, mut r) = in_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.lives, 4);
        let ev = w.tick(&r, 2);
        assert!(ev.contains(&Event::Death));
        assert_eq!(failed(&ev), Some((2, FailReason::Death)));
        assert_eq!(completed(&ev), None);

        // The reload drops us somewhere outside the stage, with no fade.
        let ev = land_on(&mut w, &mut r, REPTILE_START, 60);
        assert_eq!(completed(&ev), None, "a level change with no fade is not a clear");
        assert_eq!(completed(&idle(&mut w, &r, 61, 400)), None);
    }

    /// The plain clear: fade for the timing, a level code outside the stage
    /// family for the verdict. The reported frame is the fade, not the later
    /// level change — the hand-off is not part of the player's time.
    #[test]
    fn leaving_the_stage_after_a_fade_completes_it_timed_to_the_fade() {
        let (mut w, mut r) = in_stage(JUNGLE_START);

        assert_eq!(faded(&fade(&mut w, &mut r, 900)), vec![900]);
        let ev = land_on(&mut w, &mut r, REPTILE_START, 930);
        assert_eq!(completed(&ev), Some(900));
    }

    /// The case that broke fade counting. A bonus room raises the same fade as
    /// the exit, but its level code is inside the stage family, so it is not a
    /// clear — and crucially the attempt stays open for the real exit later.
    #[test]
    fn a_bonus_room_is_not_a_clear_and_the_real_exit_still_is() {
        let (mut w, mut r) = in_stage(JUNGLE_START);

        // Into Bonus 1.
        fade(&mut w, &mut r, 100);
        let ev = land_on(&mut w, &mut r, JUNGLE_BONUS_1, 130);
        assert_eq!(completed(&ev), None, "a bonus room is not a clear");

        // Back out of it.
        fade(&mut w, &mut r, 400);
        let ev = land_on(&mut w, &mut r, JUNGLE_FROM_BONUS_1, 430);
        assert_eq!(completed(&ev), None, "returning from a bonus is not a clear either");

        // The actual exit.
        fade(&mut w, &mut r, 900);
        let ev = land_on(&mut w, &mut r, REPTILE_START, 930);
        assert_eq!(completed(&ev), Some(900));
    }

    /// The whole point of the rewrite: the verdict must not depend on how many
    /// bonus rooms the player entered. Two routes through one stage, same
    /// answer.
    #[test]
    fn the_verdict_does_not_depend_on_the_route() {
        // Skipped the bonus entirely.
        let (mut w, mut r) = in_stage(JUNGLE_START);
        fade(&mut w, &mut r, 900);
        let direct = completed(&land_on(&mut w, &mut r, REPTILE_START, 930));

        // Took two bonus rooms on the way.
        let (mut w, mut r) = in_stage(JUNGLE_START);
        fade(&mut w, &mut r, 100);
        land_on(&mut w, &mut r, JUNGLE_BONUS_1, 130);
        fade(&mut w, &mut r, 300);
        land_on(&mut w, &mut r, JUNGLE_FROM_BONUS_1, 330);
        fade(&mut w, &mut r, 500);
        land_on(&mut w, &mut r, JUNGLE_BONUS_1, 530);
        fade(&mut w, &mut r, 700);
        land_on(&mut w, &mut r, JUNGLE_FROM_BONUS_1, 730);
        fade(&mut w, &mut r, 900);
        let scenic = completed(&land_on(&mut w, &mut r, REPTILE_START, 930));

        assert_eq!(direct, Some(900));
        assert_eq!(scenic, direct, "the route must not change the verdict");
    }

    /// A checkpoint re-entry is another way to move the level code without
    /// leaving the stage.
    #[test]
    fn a_checkpoint_code_stays_inside_the_stage() {
        let (mut w, mut r) = in_stage(JUNGLE_START);
        fade(&mut w, &mut r, 200);
        let ev = land_on(&mut w, &mut r, JUNGLE_FROM_SAVE, 230);
        assert_eq!(completed(&ev), None);
    }

    /// The shared animal rooms belong to no level's family, so family alone
    /// would call them a clear. `is_side_room` is what stops that.
    #[test]
    fn a_shared_animal_room_is_not_a_clear() {
        let (mut w, mut r) = in_stage(JUNGLE_START);
        fade(&mut w, &mut r, 200);
        let ev = land_on(&mut w, &mut r, WINKY_BONUS, 230);
        assert_eq!(completed(&ev), None, "{} is a side room", levels::show(WINKY_BONUS));

        // And the stage can still be cleared afterwards.
        fade(&mut w, &mut r, 800);
        assert_eq!(completed(&land_on(&mut w, &mut r, REPTILE_START, 830)), Some(800));
    }

    /// A fade that nothing follows must not be left armed, or the next level
    /// change — whenever it comes — gets timed to a transition it had nothing
    /// to do with.
    #[test]
    fn an_unexplained_fade_is_discarded_after_the_window() {
        let (mut w, mut r) = in_stage(JUNGLE_START);
        fade(&mut w, &mut r, 100);
        idle(&mut w, &r, 101, 100 + DEFAULT_TRANSITION_WINDOW_FRAMES + 1);

        let ev = land_on(&mut w, &mut r, REPTILE_START, 5000);
        assert_eq!(completed(&ev), None, "the stale fade must not time this");
    }

    /// The fade is an edge, so holding 0x82 for many frames arms once.
    #[test]
    fn a_fade_held_at_the_value_arms_once() {
        let (mut w, mut r) = in_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.fade_type, STAGE_COMPLETE_FADE);
        assert_eq!(faded(&w.tick(&r, 2)), vec![2]);
        assert!(faded(&idle(&mut w, &r, 3, 60)).is_empty(), "held at 0x82 is one fade");
    }

    /// A fade to some other value is a different kind of scene change and is
    /// not the signal at all.
    #[test]
    fn a_fade_to_another_value_does_not_arm() {
        let (mut w, mut r) = in_stage(JUNGLE_START);
        let a = Addrs::default();
        put(&mut r, a.fade_type, 0x04);
        assert!(faded(&w.tick(&r, 2)).is_empty());
        assert_eq!(completed(&land_on(&mut w, &mut r, REPTILE_START, 30)), None);
    }

    /// Starting in a level code the table does not cover means the stage
    /// cannot be delimited, so no clear is claimed rather than a wrong one.
    #[test]
    fn an_unknown_entry_code_disables_the_completion_detector() {
        let (mut w, mut r) = in_stage(999);
        fade(&mut w, &mut r, 100);
        assert_eq!(completed(&land_on(&mut w, &mut r, REPTILE_START, 130)), None);
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
        let mut w = Watcher::new(a, Vec::new(), DEFAULT_TRANSITION_WINDOW_FRAMES);
        let mut r = wram();
        r[0x08A5] = 2;
        w.tick(&r, 1);
        r[0x08A5] = 1;
        assert!(w.tick(&r, 2).is_empty(), "no detector without an address");

        a.set("kong_count", Some(0x7E_08A5), Some(Width::U8)).unwrap();
        let mut w = Watcher::new(a, Vec::new(), DEFAULT_TRANSITION_WINDOW_FRAMES);
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
        let mut w = Watcher::new(a, Vec::new(), DEFAULT_TRANSITION_WINDOW_FRAMES);
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
        let mut w = Watcher::new(a, Vec::new(), DEFAULT_TRANSITION_WINDOW_FRAMES);
        let mut r = wram();
        w.tick(&r, 1);
        r[0x1000] = 2;
        assert!(w.tick(&r, 2).contains(&Event::EnemyKilled(2)));
    }

    /// A savestate load reopens the attempt: a fresh rise to
    /// STAGE_COMPLETE_FADE reports its own result rather than being suppressed
    /// by the previous attempt's `resolved` flag.
    #[test]
    fn a_reset_starts_a_fresh_attempt() {
        let (mut w, mut r) = in_stage(JUNGLE_START);
        fade(&mut w, &mut r, 900);
        assert_eq!(completed(&land_on(&mut w, &mut r, REPTILE_START, 930)), Some(900));

        // A savestate load drops us back into the stage. The entry family has
        // to be re-read from RAM rather than remembered from before the load —
        // otherwise the second attempt is measured against the wrong stage.
        w.reset();
        let a = Addrs::default();
        put(&mut r, a.level_id, JUNGLE_START);
        assert!(w.tick(&r, 1).is_empty());

        fade(&mut w, &mut r, 500);
        assert_eq!(completed(&land_on(&mut w, &mut r, REPTILE_START, 530)), Some(500));
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
