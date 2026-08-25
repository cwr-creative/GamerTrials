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
//! Almost every address here now has a second, independent source: the
//! RetroAchievements code notes for game 337, transcribed in
//! `RA-CODE-NOTES.md`. Those are notes written by achievement authors against
//! running code, and RA's SNES address space is WRAM one-for-one, so an RA
//! address *is* the offset form below. Where a note and an older source agree,
//! the address is about as confirmed as it gets without a playtest; where they
//! disagree, the note wins, because the older sources were wikis that could
//! not be reached at all.
//!
//!   $7E0527 in stage       RA: "00=In a Stage 01=In Overworld". The spine of
//!                          stage completion — see `Watcher::tick`.
//!   $7E003E level id       confirmed twice: the RainbowZ Editor transcribes
//!                          the game's own bound lookup as `LDA $3E`, and RA
//!                          calls it "Room ID (last exit used)". `levels.rs`
//!                          maps its 230 values to names. Used for naming
//!                          only now, not for the verdict.
//!   $7E0040 room id        RA: "Last Room ID inside a level (else 0)". A
//!                          second, independent in-a-stage indicator, kept as
//!                          a cross-check on $0527.
//!   $7E0575 lives          RA: "[2 Byte] Lives". Note that $0577, which every
//!                          older map quotes and this harness used to watch,
//!                          is "[2 Byte] Lives (Display)" — the copy the HUD
//!                          draws from. The real counter is the one that moves
//!                          on the frame of death.
//!   $7E057B bananas        RA: "Bananas / Mini Animal Tokens (Up to 999)",
//!                          agreeing with the older map. Shared with the mini
//!                          animal token count inside an animal bonus.
//!   $7E057F KONG letters   RA: "KONG letters flag". Unmarked in the notes,
//!                          which in that set means 8-bit — so this is read as
//!                          a byte, not the 16 bits the older map implied.
//!   $7E051B fade type      confirmed twice: `Lui37/dkc1-hack` src/defines.asm
//!                          and RA's "82 = Screen is Fading-Out". Playtested:
//!                          a death produces no fade, a bonus barrel produces
//!                          the same 0x82 the exit does. Timing only.
//!   $7E0579 state flags    RA: "b0=2 Kongs, b1=Invulnerable?, b3=Inside
//!                          Barrel/Bouncing?, b6=Pause, b7=Launched from
//!                          barrel". The dkc1-hack agreed on bit 6. Bit 0 is
//!                          what the damage detector now watches.
//!   $7E16D1 hit timer      RA: "[2 Byte] Timer: Donkey flashes (when taking
//!                          hit)". Diddy's is $16D3; the invulnerability
//!                          timers are $16D5 and $16D7.
//!   $7E0516 animal buddy   RA: "Riding Buddy? No=0, Rambi=9, Expresso=0a,
//!                          Winky=0b, Enguarde=0c".
//!   $7E1E17 enemy kills    RA: "Enemies stomped in a row". A combo counter,
//!                          not a running total — it resets. Good enough to
//!                          see a stomp happen, not to count a stage's worth.
//!   $7E005E player X       RA: "[2 Byte] Character's X coord", and $0060 for
//!   $7E0060 player Y       Y. These follow whichever Kong is active.
//!   $7E0FC3 DK X           RA: "[16-Bits] DK's Position X", and $0FF7 for Y.
//!   $7E0FF7 DK Y           Diddy's are $0FC5 and $0FF9. Carried alongside the
//!                          pair above because the two disagree about what
//!                          "the player's position" means and only a playtest
//!                          can settle which one progress should be measured
//!                          from.
//!   $7E1929 accept inputs  dkc1-hack defines.asm. The one address here with
//!                          no RA note, so the one still resting on a single
//!                          source.
//!
//! `$0B19` / `$0BC1`, the "Object 1 (DK) X/Y" pair this harness used to watch,
//! have *no* RA note at all. They are still reachable with `--addr`, but they
//! are no longer the default, and PROGRESS-HANDOFF.md is written against them,
//! so that document is now the least trustworthy thing in this crate.
//!
//! # What still has no address
//!
//! A dedicated Kong counter. Damage instead reads bit 0 of the state flags,
//! which RA calls "2 Kongs" — losing the partner clears it. `kong_count`
//! remains as an overridable field in case a real counter turns up.
//!
//! A running enemy-kill total. `$1E17` counts a stomp *streak*. If a real
//! total is wanted, the better lead is the per-object table RA documents at
//! `$7E102F`-`$7E1049`: one flag per object slot, `01 = Object _N Defeated`,
//! parallel to the object ids at `$7E0D4B`-`$7E0D61`. Counting set flags there
//! would be a real kill count, and it is the sprite table this harness was
//! told it could not have.
//!
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
    in_stage,      Some(0x0527), U8,  "RA notes: 00 = in a stage, 01 = overworld";
    level_id,      Some(0x003E), U16, "RA notes + RainbowZ: room id (last exit used)";
    room_id,       Some(0x0040), U16, "RA notes: last room id inside a level, else 0";
    lives,         Some(0x0575), U16, "RA notes: lives ($0577 is the display copy)";
    bananas,       Some(0x057B), U16, "RA notes: bananas / mini animal tokens, up to 999";
    kong_letters,  Some(0x057F), U8,  "RA notes: KONG letters flag";
    fade_type,     Some(0x051B), U8,  "RA notes + dkc1-hack: 0x82 = screen fading out";
    state_flags,   Some(0x0579), U8,  "RA notes: b0 = 2 Kongs, b6 = paused";
    hit_timer,     Some(0x16D1), U16, "RA notes: DK flash timer, set on taking a hit";
    animal,        Some(0x0516), U8,  "RA notes: riding buddy; 9 Rambi A Expresso B Winky C Enguarde";
    enemy_kills,   Some(0x1E17), U8,  "RA notes: enemies stomped in a row (a streak, not a total)";
    player_x,      Some(0x005E), U16, "RA notes: character's X coord";
    player_y,      Some(0x0060), U16, "RA notes: character's Y coord";
    dk_x,          Some(0x0FC3), U16, "RA notes: DK's position X (Diddy $0FC5)";
    dk_y,          Some(0x0FF7), U16, "RA notes: DK's position Y (Diddy $0FF9)";
    accept_inputs, Some(0x1929), U16, "dkc1-hack defines.asm; no RA note, single source";
    kong_count,    None,         U16, "UNKNOWN - damage reads state_flags b0 instead";
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
            "{}  level {}  room {}  lives {}  bananas {}  KONG {}  {}  fade {}  at ({},{})",
            match self.in_stage {
                Some(IN_STAGE) => "in-stage",
                Some(_) => "overworld",
                None => "where?",
            },
            v(self.level_id),
            v(self.room_id),
            v(self.lives),
            v(self.bananas),
            match self.kong_letters {
                Some(b) => kong_letters(b),
                None => "-".into(),
            },
            match self.state_flags {
                Some(st) if st & STATE_TWO_KONGS != 0 => "2 kongs".to_string(),
                Some(_) => "1 kong".to_string(),
                None => "? kongs".to_string(),
            },
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
    /// The player left the stage from the pause menu. Reaching the exit and
    /// abandoning the attempt both put the overworld on screen; the pause flag
    /// at the moment the fade started is what tells them apart.
    Quit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Damage,
    Death,
    EnemyKilled(u16),
    ItemGained(Item),
    /// A transition out of the *room* has started. Reported on its own
    /// because it is the frame a clear is *timed* to, but it is not a verdict:
    /// a bonus barrel raises it exactly as the exit does. `in_stage` says
    /// whether the room being left was the last one.
    Fade { at: u64 },
    StageComplete { at: u64 },
    StageFailed { at: u64, reason: FailReason },
}

/// The fade-to-black speed the game sets at `fade_type` on any scene change
/// that takes the player out of the current *room*.
///
/// A death causes no fade at all, so this can never fire from one. It is not a
/// verdict on its own — a bonus barrel sets the same value the exit does — and
/// it is no longer asked to be one. All it supplies now is the frame: the
/// stage ended when the screen started to go dark, not when the overworld
/// finished loading (ADR 0005).
pub const STAGE_COMPLETE_FADE: u16 = 0x82;

/// The value of `in_stage` ($7E0527) while the player is inside a stage.
/// Anything else means the overworld map.
pub const IN_STAGE: u16 = 0x00;

/// `state_flags` bit 0: both Kongs are present. Losing the partner clears it,
/// which is what the damage detector watches.
pub const STATE_TWO_KONGS: u16 = 1 << 0;

/// `state_flags` bit 6: the pause menu is up.
pub const STATE_PAUSED: u16 = 1 << 6;

/// How long a fade stays good for as the timing of a stage end.
///
/// The fade starts the transition; `in_stage` flips when the overworld is
/// actually up, a little later. That gap is the whole reason the fade is worth
/// keeping — it is the honest frame the stage ended — but a fade with nothing
/// behind it means something else happened, and it must not be left armed to
/// mis-time a later, unrelated exit. Two seconds at 60 Hz is far longer than
/// the hand-off.
pub const DEFAULT_TRANSITION_WINDOW_FRAMES: u64 = 120;

pub struct Watcher {
    addrs: Addrs,
    prev: Option<Snapshot>,
    /// Set once a death has failed this attempt, so one death does not report
    /// twice. Deliberately does *not* gate the fade counter below: a latch
    /// over the fade is what made the bonus-room behaviour invisible, because
    /// the first fade silenced every one after it.
    failed: bool,
    /// Set once this attempt has produced a verdict, so the overworld
    /// arriving cannot report twice.
    ended: bool,
    /// A fade whose destination is not known yet, and the frame it happened
    /// on. That frame is what a stage end is timed to.
    pending_fade: Option<u64>,
    /// Whether the pause menu was up when `pending_fade` was armed. A player
    /// who quits from the menu gets the same fade and the same overworld as
    /// one who reached the exit; this is the only thing separating them.
    faded_while_paused: bool,
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
            ended: false,
            pending_fade: None,
            faded_while_paused: false,
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
        self.ended = false;
        self.pending_fade = None;
        self.faded_while_paused = false;
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
                // Stage identity still comes from the entry point, not from
                // RAM. What RAM says here is only where the player is, and the
                // level name is context for reading the log.
                if let Some(c) = cur.level_id {
                    match levels::family(c) {
                        Some(fam) => println!("[stage] entered at {} — in `{fam}`", levels::show(c)),
                        None => println!("[stage] entered at {}", levels::show(c)),
                    }
                }
                match cur.in_stage {
                    Some(IN_STAGE) => {}
                    Some(v) => println!(
                        "[stage] in_stage reads {v:02X}, not {IN_STAGE:02X}: this state is not inside a stage"
                    ),
                    None => println!(
                        "[stage] no in_stage address, so there is no stage-complete detector"
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
        // Losing the partner Kong while the lead one survives: a hit that did
        // not cost a life. RA documents bit 0 of $7E0579 as "2 Kongs", so the
        // hit is that bit falling. A death already reported this frame
        // suppresses it, since dying with one Kong clears the bit too.
        if !died {
            if let (Some(p), Some(c)) = (prev.state_flags, cur.state_flags) {
                if p & STATE_TWO_KONGS != 0 && c & STATE_TWO_KONGS == 0 {
                    println!("[event] frame {frame}  DAMAGE  lost the partner Kong");
                    events.push(Event::Damage);
                }
            }
            // Kept as an override for a real Kong counter, if one turns up.
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
                println!("[event] frame {frame}  ENEMY KILLED  x{n} (streak {c})");
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

        // ---- leaving the stage -------------------------------------------
        // $7E0527 is the game's own answer to "is the player inside a stage?"
        // — RA's code notes call it "00=In a Stage 01=In Overworld". Every way
        // of *staying* in a level leaves it at 00: a bonus room, the return
        // from one, a checkpoint, a death and the reload it causes. Only
        // actually leaving flips it.
        //
        // That replaces the previous rule, which paired the fade with the
        // level code at $7E003E and asked whether the new code was in the same
        // level "family" as the entry code. It needed a 230-entry table, an
        // explicit list of shared animal rooms, and a per-attempt notion of
        // which family the stage belonged to — all of it reconstructing, from
        // the outside, a fact the game already stores in one byte.
        //
        // The fade at $7E051B stays, but only for timing. It rises a little
        // before the overworld is up, and ADR 0005 says the hand-off is not
        // the player's time.
        // Expire a stale fade before looking for a new one. A fade nothing
        // explained must not be left armed — the next exit would get timed to
        // a transition it had nothing to do with — and it must not block a
        // fresh fade arriving on this same frame either.
        if let Some(at) = self.pending_fade {
            if frame.saturating_sub(at) >= self.transition_window {
                println!(
                    "[stage] the fade at frame {at} led nowhere out of the stage; discarding it"
                );
                self.pending_fade = None;
                self.faded_while_paused = false;
            }
        }

        if let (Some(p), Some(c)) = (prev.fade_type, cur.fade_type) {
            if c == STAGE_COMPLETE_FADE && p != STAGE_COMPLETE_FADE && self.pending_fade.is_none() {
                println!("[event] frame {frame}  FADE  {p:02X} -> {c:02X}");
                events.push(Event::Fade { at: frame });
                self.pending_fade = Some(frame);
                // Read now rather than when the overworld arrives: by then the
                // menu is long gone.
                self.faded_while_paused =
                    cur.state_flags.is_some_and(|st| st & STATE_PAUSED != 0);
            }
        }

        if let (Some(p), Some(c)) = (prev.in_stage, cur.in_stage) {
            if p == IN_STAGE && c != IN_STAGE {
                // Timed to the fade when there is a fresh one, because that is
                // when the stage actually ended. Without one — which should
                // not happen, since leaving always fades — this frame is the
                // best answer available and is better than no verdict.
                let at = self.pending_fade.take().unwrap_or(frame);
                let quit = self.faded_while_paused;
                self.faded_while_paused = false;

                println!("[stage] left the stage: in_stage {p:02X} -> {c:02X}");
                if self.ended {
                    // Nothing to say twice.
                } else if self.failed {
                    // The attempt was already lost; this is the game-over
                    // sequence walking back to the map, not a clear.
                    self.ended = true;
                    println!("[stage] the attempt had already failed — not a clear");
                } else if quit {
                    self.ended = true;
                    println!("[stage] the fade began while paused — scoring this as a quit");
                    events.push(Event::StageFailed { at, reason: FailReason::Quit });
                    self.failed = true;
                } else {
                    self.ended = true;
                    events.push(Event::StageComplete { at });
                }
            }
        }

        // The level code no longer decides anything, but it still says where
        // the player went, which is most of what makes the log readable.
        if let (Some(pc), Some(cc)) = (prev.level_id, cur.level_id) {
            if pc != cc {
                println!("[stage] level {} -> {}", levels::show(pc), levels::show(cc));
            }
        }

        // Anything that moved and is worth a running log line, printed once
        // per change rather than per frame. Position is excluded: it changes
        // every frame the player moves.
        let interesting = |s: &Snapshot| {
            (s.in_stage, s.level_id, s.lives, s.bananas, s.kong_letters, s.state_flags, s.fade_type)
        };
        if interesting(&cur) != interesting(&prev) {
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

    // Real level codes. They no longer decide anything, but the log prints
    // them and a bonus room still has to not be a clear, so the tests keep
    // walking a real route.
    const JUNGLE_START: u16 = 0x16; // Jungle Hijinx (level start)
    const JUNGLE_BONUS_1: u16 = 0x06; // Jungle Hijinx - Bonus 1
    const JUNGLE_FROM_BONUS_1: u16 = 0x08; // Jungle Hijinx (from Bonus 1)
    const JUNGLE_FROM_SAVE: u16 = 0x25; // Jungle Hijinx (from Save)
    const REPTILE_START: u16 = 0x01; // Reptile Rumble (level start) — another stage
    const WINKY_BONUS: u16 = 0xD3; // shared animal room

    /// Starts an attempt inside a stage and returns a watcher past its
    /// baseline tick. `in_stage` reads IN_STAGE, which is what a savestate
    /// taken inside a level looks like.
    fn enter_stage(code: u16) -> (Watcher, Vec<u8>) {
        let mut w = watcher();
        let a = Addrs::default();
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, code);
        put(&mut r, a.in_stage, IN_STAGE);
        put(&mut r, a.state_flags, STATE_TWO_KONGS);
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

    /// The room changing under the player, a few frames after a fade. Inside
    /// one stage this is a bonus room or a checkpoint; it is not an exit.
    fn land_on(w: &mut Watcher, ram: &mut [u8], code: u16, at: u64) -> Vec<Event> {
        let a = Addrs::default();
        put(ram, a.level_id, code);
        w.tick(ram, at)
    }

    /// The overworld map coming up: the stage is over. This is the only thing
    /// that ends an attempt now.
    fn to_overworld(w: &mut Watcher, ram: &mut [u8], at: u64) -> Vec<Event> {
        let a = Addrs::default();
        put(ram, a.in_stage, 1);
        w.tick(ram, at)
    }

    /// Holds the pause menu open, which is what separates a quit from a clear.
    fn pause(ram: &mut [u8], on: bool) {
        let a = Addrs::default();
        let mut st = STATE_TWO_KONGS;
        if on {
            st |= STATE_PAUSED;
        }
        put(ram, a.state_flags, st);
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

    /// A death does not put the overworld on screen — DKC drops the player
    /// back into the level. So the reload that follows one is not an exit, and
    /// the attempt is failed rather than cleared.
    #[test]
    fn a_death_and_its_level_reload_are_never_a_clear() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.lives, 4);
        let ev = w.tick(&r, 2);
        assert!(ev.contains(&Event::Death));
        assert_eq!(failed(&ev), Some((2, FailReason::Death)));
        assert_eq!(completed(&ev), None);

        // The reload moves the room code without leaving the stage.
        let ev = land_on(&mut w, &mut r, JUNGLE_START, 60);
        assert_eq!(completed(&ev), None);
        assert_eq!(completed(&idle(&mut w, &r, 61, 400)), None);
    }

    /// The plain clear: a fade for the timing, the overworld for the verdict.
    /// The reported frame is the fade, not the later map load — the hand-off
    /// is not part of the player's time (ADR 0005).
    #[test]
    fn reaching_the_overworld_completes_the_stage_timed_to_the_fade() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);

        assert_eq!(faded(&fade(&mut w, &mut r, 900)), vec![900]);
        let ev = to_overworld(&mut w, &mut r, 930);
        assert_eq!(completed(&ev), Some(900));
    }

    /// The case that broke fade counting, and the case the family table was
    /// built for. A bonus room raises the same fade the exit does, but the
    /// player is still inside a stage, so nothing scores and the attempt stays
    /// open for the real exit.
    #[test]
    fn a_bonus_room_is_not_a_clear_and_the_real_exit_still_is() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);

        fade(&mut w, &mut r, 100);
        let ev = land_on(&mut w, &mut r, JUNGLE_BONUS_1, 130);
        assert_eq!(completed(&ev), None, "a bonus room is not a clear");

        fade(&mut w, &mut r, 400);
        let ev = land_on(&mut w, &mut r, JUNGLE_FROM_BONUS_1, 430);
        assert_eq!(completed(&ev), None, "returning from a bonus is not one either");

        fade(&mut w, &mut r, 900);
        assert_eq!(completed(&to_overworld(&mut w, &mut r, 930)), Some(900));
    }

    /// The property the whole design exists for: two routes through one stage,
    /// same verdict and same time.
    #[test]
    fn the_verdict_does_not_depend_on_the_route() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        fade(&mut w, &mut r, 900);
        let direct = completed(&to_overworld(&mut w, &mut r, 930));

        let (mut w, mut r) = enter_stage(JUNGLE_START);
        fade(&mut w, &mut r, 100);
        land_on(&mut w, &mut r, JUNGLE_BONUS_1, 130);
        fade(&mut w, &mut r, 300);
        land_on(&mut w, &mut r, JUNGLE_FROM_BONUS_1, 330);
        fade(&mut w, &mut r, 500);
        land_on(&mut w, &mut r, WINKY_BONUS, 530);
        fade(&mut w, &mut r, 700);
        land_on(&mut w, &mut r, JUNGLE_FROM_SAVE, 730);
        fade(&mut w, &mut r, 900);
        let scenic = completed(&to_overworld(&mut w, &mut r, 930));

        assert_eq!(direct, Some(900));
        assert_eq!(scenic, direct, "the route must not change the verdict");
    }

    /// The shared animal rooms used to need naming explicitly, because they
    /// belong to no level's family. They need nothing now: the player is
    /// inside a stage the whole time they are in one.
    #[test]
    fn a_shared_animal_room_is_not_a_clear() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        fade(&mut w, &mut r, 200);
        let ev = land_on(&mut w, &mut r, WINKY_BONUS, 230);
        assert_eq!(completed(&ev), None, "{} is inside the stage", levels::show(WINKY_BONUS));

        fade(&mut w, &mut r, 800);
        assert_eq!(completed(&to_overworld(&mut w, &mut r, 830)), Some(800));
    }

    /// Quitting from the pause menu reaches the overworld exactly as the exit
    /// does. The pause flag at the moment of the fade is the only difference,
    /// and it must not be read later — by the time the map is up the menu is
    /// gone.
    #[test]
    fn leaving_from_the_pause_menu_is_a_quit_not_a_clear() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);

        pause(&mut r, true);
        w.tick(&r, 500);
        fade(&mut w, &mut r, 520);
        pause(&mut r, false);

        let ev = to_overworld(&mut w, &mut r, 560);
        assert_eq!(completed(&ev), None, "a menu exit is not a clear");
        assert_eq!(failed(&ev), Some((520, FailReason::Quit)));
    }

    /// A fade that led nowhere must not be left armed, or the real exit —
    /// whenever it comes — gets timed to a transition it had nothing to do
    /// with.
    #[test]
    fn a_fade_that_led_nowhere_is_discarded_after_the_window() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        fade(&mut w, &mut r, 100);
        idle(&mut w, &r, 101, 100 + DEFAULT_TRANSITION_WINDOW_FRAMES + 1);

        // No fresh fade, so the exit is timed to itself rather than to the
        // stale one. Still a clear: reaching the map is the verdict.
        let ev = to_overworld(&mut w, &mut r, 5000);
        assert_eq!(completed(&ev), Some(5000), "the stale fade must not time this");
    }

    /// The fade is an edge, so holding 0x82 for many frames arms once.
    #[test]
    fn a_fade_held_at_the_value_arms_once() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.fade_type, STAGE_COMPLETE_FADE);
        assert_eq!(faded(&w.tick(&r, 2)), vec![2]);
        assert!(faded(&idle(&mut w, &r, 3, 60)).is_empty(), "held at 0x82 is one fade");
    }

    /// Reaching the map twice reports once. The second crossing is the game
    /// walking its own menus, not a second clear.
    #[test]
    fn the_stage_only_ends_once() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        let a = Addrs::default();
        fade(&mut w, &mut r, 900);
        assert_eq!(completed(&to_overworld(&mut w, &mut r, 930)), Some(900));

        put(&mut r, a.in_stage, IN_STAGE);
        w.tick(&r, 940);
        assert_eq!(completed(&to_overworld(&mut w, &mut r, 950)), None);
    }

    /// With no `in_stage` address there is no completion detector at all,
    /// rather than a guess from the level code.
    #[test]
    fn without_in_stage_no_clear_is_ever_claimed() {
        let mut a = Addrs::default();
        a.set("in_stage", None, None).unwrap();
        let mut w = Watcher::new(a, Vec::new(), DEFAULT_TRANSITION_WINDOW_FRAMES);
        let mut r = wram();
        put(&mut r, a.lives, 5);
        put(&mut r, a.level_id, JUNGLE_START);
        w.tick(&r, 1);

        fade(&mut w, &mut r, 100);
        let ev = land_on(&mut w, &mut r, REPTILE_START, 130);
        assert_eq!(completed(&ev), None);
        assert_eq!(completed(&idle(&mut w, &r, 131, 600)), None);
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

    /// Losing the partner Kong is bit 0 of the state flags falling. This is
    /// the detector that used to be off for want of an address.
    #[test]
    fn losing_the_partner_kong_is_damage() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.state_flags, 0);
        assert!(w.tick(&r, 2).contains(&Event::Damage));

        // And it is an edge: staying at one Kong is not damage every frame.
        assert!(!idle(&mut w, &r, 3, 60).contains(&Event::Damage));
    }

    /// The hit timer is the other half, and it is the one that fires when a
    /// solo Kong is hit and there is no partner to lose.
    #[test]
    fn a_hit_timer_rising_from_zero_is_damage() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.hit_timer, 40);
        assert!(w.tick(&r, 2).contains(&Event::Damage));
        // Counting down from that hit is not a second one.
        put(&mut r, a.hit_timer, 39);
        assert!(!w.tick(&r, 3).contains(&Event::Damage));
    }

    /// An override still turns a detector off without a rebuild.
    #[test]
    fn an_override_can_silence_a_detector() {
        let mut a = Addrs::default();
        a.set("state_flags", None, None).unwrap();
        a.set("hit_timer", None, None).unwrap();
        let mut w = Watcher::new(a, Vec::new(), DEFAULT_TRANSITION_WINDOW_FRAMES);
        let mut r = wram();
        put(&mut r, Addrs::default().state_flags, STATE_TWO_KONGS);
        w.tick(&r, 1);
        put(&mut r, Addrs::default().state_flags, 0);
        assert!(!w.tick(&r, 2).contains(&Event::Damage));
    }

    /// Dying with one Kong clears the two-Kong bit as well; that is one
    /// event, not damage plus a death.
    #[test]
    fn a_death_suppresses_the_damage_it_also_causes() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.lives, 4);
        put(&mut r, a.state_flags, 0);
        let ev = w.tick(&r, 2);
        assert!(ev.contains(&Event::Death));
        assert!(!ev.contains(&Event::Damage));
    }

    /// $1E17 is a stomp *streak*, so it rises per stomp and drops back to
    /// zero when the chain breaks. The rise is the event; the reset is not.
    #[test]
    fn a_rising_stomp_streak_reports_enemy_kills() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        let a = Addrs::default();

        put(&mut r, a.enemy_kills, 1);
        assert!(w.tick(&r, 2).contains(&Event::EnemyKilled(1)));
        put(&mut r, a.enemy_kills, 3);
        assert!(w.tick(&r, 3).contains(&Event::EnemyKilled(2)));
        put(&mut r, a.enemy_kills, 0);
        let ev = w.tick(&r, 4);
        assert!(!ev.iter().any(|e| matches!(e, Event::EnemyKilled(_))));
    }

    /// A savestate load reopens the attempt: the second run reports its own
    /// clear rather than being suppressed by the first one's `ended` flag.
    #[test]
    fn a_reset_starts_a_fresh_attempt() {
        let (mut w, mut r) = enter_stage(JUNGLE_START);
        fade(&mut w, &mut r, 900);
        assert_eq!(completed(&to_overworld(&mut w, &mut r, 930)), Some(900));

        w.reset();
        let a = Addrs::default();
        put(&mut r, a.in_stage, IN_STAGE);
        put(&mut r, a.level_id, JUNGLE_START);
        assert!(w.tick(&r, 1).is_empty());

        fade(&mut w, &mut r, 500);
        assert_eq!(completed(&to_overworld(&mut w, &mut r, 530)), Some(500));
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
