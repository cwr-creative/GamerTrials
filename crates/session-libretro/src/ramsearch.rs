//! Two ways to find an address that nobody has written down.
//!
//! The NES got away without this. SMB3's RAM map is thoroughly documented, its
//! RAM is 2 KB, and `--watch 0x0090-0x00ff` over a range that small prints
//! something a human can read. DKC has 128 KB of work RAM and a RAM map that is
//! mostly missing, so the same approach prints tens of thousands of lines a
//! second and finds nothing. Both tools below work on any console's system-RAM
//! slice; which one you need depends on how big it is and how well documented.
//!
//! So there are two tools here, and they answer different questions.
//!
//! [`Search`] is the classic cheat-search: snapshot RAM, play, then filter the
//! candidate set by how each byte moved — unchanged, changed, increased,
//! decreased. Repeat until few candidates remain. This is for **values that
//! persist**: lives, bananas, a Kong count, an enemy-kill counter if one
//! exists. It needs several rounds and a value you can push in a known
//! direction on demand.
//!
//! [`Probe`] answers a different question: *what changed just now?* It records,
//! for every byte, the frame it last changed and the frame it changed before
//! that. Ask it for a report and it lists bytes that were **stable for a long
//! while and then moved in the last moment** — which is exactly the shape of a
//! one-shot event. Take a hit, hit the key, and the address the game wrote
//! when the Kong was lost is in a list of a dozen rather than of thousands.
//! This is the tool for damage, item pickups, and stage clear: things that
//! happen once and cannot be nudged up and down at will.
//!
//! Both work on the system-RAM slice as offsets. Everything they print goes
//! through the manifest's address space first, so it comes out in the same
//! spelling the manifest uses and pastes straight into `--addr` or `--watch`.

use crate::memory::{AddressSpace, Width};

/// How a candidate must have moved to survive a filter step. Compared against
/// the snapshot taken at the previous step, which is the usual cheat-search
/// meaning of "changed".
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Filter {
    Unchanged,
    Changed,
    Increased,
    Decreased,
}

impl Filter {
    pub fn label(self) -> &'static str {
        match self {
            Filter::Unchanged => "unchanged",
            Filter::Changed => "changed",
            Filter::Increased => "increased",
            Filter::Decreased => "decreased",
        }
    }
}

/// Cheat-engine-style narrowing over the whole of work RAM.
pub struct Search {
    /// RAM as it was at the last snapshot or filter step.
    base: Vec<u8>,
    /// Offsets still in the running. Held as `u32` because 128 K of `usize`
    /// is a megabyte of pointers for no reason.
    candidates: Vec<u32>,
    width: Width,
    started: bool,
    /// Filter steps applied since the last snapshot, for the log line.
    steps: u32,
}

impl Search {
    pub fn new(width: Width) -> Self {
        Search {
            base: Vec::new(),
            candidates: Vec::new(),
            width,
            started: false,
            steps: 0,
        }
    }

    /// Reads one candidate in the search's width. `None` past the end of the
    /// window, which only a multi-byte read near the last byte can hit.
    ///
    /// Little-endian, unconditionally: this is a search over raw bytes rather
    /// than over a manifest's declared fields, and a candidate's byte order is
    /// not known until it turns out to be the address you were looking for.
    fn value(&self, ram: &[u8], off: usize) -> Option<u32> {
        let mut v = 0u32;
        for i in 0..self.width.bytes() {
            v |= u32::from(*ram.get(off + i)?) << (8 * i);
        }
        Some(v)
    }

    /// Start over: every address is a candidate again, and this frame's RAM
    /// becomes the baseline the next filter compares against.
    pub fn snapshot(&mut self, ram: &[u8]) {
        self.base.clear();
        self.base.extend_from_slice(ram);
        // A multi-byte candidate needs its high bytes to exist, so the last
        // few offsets are excluded rather than silently reading past the end.
        let last = ram.len().saturating_sub(self.width.bytes() - 1);
        self.candidates = (0..last as u32).collect();
        self.started = true;
        self.steps = 0;
        println!(
            "[search] snapshot taken at {} width; {} candidates",
            self.width,
            self.candidates.len()
        );
    }

    /// Narrow the candidate set, then re-baseline on this frame so the next
    /// filter compares against here rather than against the original
    /// snapshot.
    pub fn filter(&mut self, ram: &[u8], f: Filter) {
        if !self.started {
            println!("[search] no search running — take a snapshot first");
            return;
        }
        let before = self.candidates.len();
        let mut base = std::mem::take(&mut self.base);
        let mut kept = std::mem::take(&mut self.candidates);
        kept.retain(|&off| {
            let off = off as usize;
            let (Some(was), Some(now)) = (self.value(&base, off), self.value(ram, off)) else {
                return false;
            };
            match f {
                Filter::Unchanged => now == was,
                Filter::Changed => now != was,
                Filter::Increased => now > was,
                Filter::Decreased => now < was,
            }
        });
        self.candidates = kept;
        base.clear();
        base.extend_from_slice(ram);
        self.base = base;
        self.steps += 1;
        println!(
            "[search] step {}: {} -> {} candidates ({})",
            self.steps,
            before,
            self.candidates.len(),
            f.label()
        );
    }

    /// Print what is left. Capped, because a search that has not narrowed yet
    /// still holds every address in RAM.
    pub fn list(&self, space: &AddressSpace, ram: &[u8], limit: usize) {
        if !self.started {
            println!("[search] no search running — take a snapshot first");
            return;
        }
        println!(
            "[search] {} candidate(s) after {} step(s):",
            self.candidates.len(),
            self.steps
        );
        for &off in self.candidates.iter().take(limit) {
            let off = off as usize;
            let at = space.show(off + space.base);
            match self.value(ram, off) {
                Some(v) => println!("        {at} = {v:5} ({v:04X})"),
                None => println!("        {at} = <out of range>"),
            }
        }
        if self.candidates.len() > limit {
            println!(
                "        ... {} more; narrow further before listing",
                self.candidates.len() - limit
            );
        }
    }
}

/// Per-byte change history, for answering "what did the game write when that
/// happened?" after the fact.
///
/// Cost is four arrays over 128 KB — two of `u64` timestamps and two of bytes,
/// about 2.3 MB — plus a full compare every frame. At 128 K bytes and 60 Hz
/// that is a few tens of microseconds per frame in a release build, which is
/// noise next to running the emulator itself.
pub struct Probe {
    prev: Vec<u8>,
    /// The value each byte held before its most recent change, so a report can
    /// show what it moved *from* rather than only where it landed.
    value_before: Vec<u8>,
    last_change: Vec<u64>,
    prior_change: Vec<u64>,
    armed: bool,
}

impl Probe {
    pub fn new() -> Self {
        Probe {
            prev: Vec::new(),
            value_before: Vec::new(),
            last_change: Vec::new(),
            prior_change: Vec::new(),
            armed: false,
        }
    }

    /// Re-arm after a savestate load: RAM has been replaced wholesale, and
    /// every byte would otherwise look like it changed at once.
    pub fn reset(&mut self) {
        self.armed = false;
    }

    pub fn tick(&mut self, ram: &[u8], frame: u64) {
        if !self.armed || self.prev.len() != ram.len() {
            // Arming records the current RAM as the baseline and backdates
            // every timestamp to now. A byte that changes later then reports a
            // stable interval measured from the moment of arming, which is the
            // honest answer — we know nothing about what it did before that.
            self.prev.clear();
            self.prev.extend_from_slice(ram);
            self.value_before.clear();
            self.value_before.extend_from_slice(ram);
            self.last_change = vec![frame; ram.len()];
            self.prior_change = vec![frame; ram.len()];
            self.armed = true;
            return;
        }
        // Five parallel arrays walked in lockstep. Zipping them keeps the
        // bounds checks out of the hot path, which matters: this runs over
        // 128 KB every frame.
        let rows = self
            .prev
            .iter_mut()
            .zip(self.value_before.iter_mut())
            .zip(self.last_change.iter_mut())
            .zip(self.prior_change.iter_mut())
            .zip(ram.iter());
        for ((((prev, value_before), last), prior), &now) in rows {
            if now != *prev {
                *value_before = *prev;
                *prior = *last;
                *last = frame;
                *prev = now;
            }
        }
    }

    /// Report bytes that moved within the last `recent` frames after holding
    /// still for at least `stable` before that.
    ///
    /// The two windows are what does the work. `recent` says "around when I
    /// pressed the key"; `stable` throws out everything that is always busy —
    /// positions, animation timers, the RNG, the audio driver's scratch space
    /// — because those never hold still for a second at a time.
    pub fn report(
        &self,
        space: &AddressSpace,
        ram: &[u8],
        frame: u64,
        recent: u64,
        stable: u64,
        limit: usize,
    ) {
        if !self.armed {
            println!("[probe] not armed yet");
            return;
        }
        println!(
            "[probe] frame {frame}: bytes that changed in the last {recent} frame(s) \
             after holding still for {stable}+"
        );
        let mut hits = 0usize;
        let mut shown = 0usize;
        for i in 0..self.last_change.len() {
            let last = self.last_change[i];
            if frame.saturating_sub(last) > recent {
                continue;
            }
            if last.saturating_sub(self.prior_change[i]) < stable {
                continue;
            }
            hits += 1;
            if shown >= limit {
                continue;
            }
            shown += 1;
            let was = self.value_before[i];
            let now = ram.get(i).copied().unwrap_or(was);
            println!(
                "        {}  {was:02X} -> {now:02X}  at frame {last} (still for {})",
                space.show(i + space.base),
                last.saturating_sub(self.prior_change[i])
            );
        }
        if hits == 0 {
            println!("        nothing — widen --probe-recent, or narrow --probe-stable");
        } else if hits > shown {
            println!("        ... {} more", hits - shown);
        }
    }
}

/// Accessors the console never needs: the harness reports through `println!`,
/// so these exist only so the narrowing logic can be asserted on rather than
/// eyeballed.
#[cfg(test)]
impl Search {
    fn is_started(&self) -> bool {
        self.started
    }

    fn remaining(&self) -> usize {
        self.candidates.len()
    }
}

#[cfg(test)]
impl Probe {
    /// How many bytes `report` would list, without the printing. Exists so
    /// the window logic can be tested rather than eyeballed in a console.
    fn qualifying(&self, _ram: &[u8], frame: u64, recent: u64, stable: u64) -> usize {
        if !self.armed {
            return 0;
        }
        (0..self.last_change.len())
            .filter(|&i| {
                let last = self.last_change[i];
                frame.saturating_sub(last) <= recent
                    && last.saturating_sub(self.prior_change[i]) >= stable
            })
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ram(n: usize) -> Vec<u8> {
        vec![0u8; n]
    }

    #[test]
    fn a_search_narrows_to_the_byte_that_moved_the_right_way() {
        let mut s = Search::new(Width::U8);
        let mut r = ram(64);
        r[10] = 5;
        r[20] = 5;
        s.snapshot(&r);
        assert_eq!(s.remaining(), 64);

        // One goes up, one goes down. "Increased" keeps only the first.
        r[10] = 6;
        r[20] = 4;
        s.filter(&r, Filter::Increased);
        assert_eq!(s.remaining(), 1);

        // A second round that nothing satisfies empties the set rather than
        // quietly keeping stale candidates.
        s.filter(&r, Filter::Increased);
        assert_eq!(s.remaining(), 0);
    }

    #[test]
    fn unchanged_keeps_only_still_bytes() {
        let mut s = Search::new(Width::U8);
        let mut r = ram(8);
        s.snapshot(&r);
        r[3] = 1;
        s.filter(&r, Filter::Unchanged);
        assert_eq!(s.remaining(), 7);
    }

    /// Each filter re-baselines, so "changed" means changed since the previous
    /// step and not since the original snapshot.
    #[test]
    fn filters_compare_against_the_previous_step() {
        let mut s = Search::new(Width::U8);
        let mut r = ram(8);
        s.snapshot(&r);
        r[1] = 9;
        s.filter(&r, Filter::Changed);
        assert_eq!(s.remaining(), 1);
        // $01 stays at 9, so relative to the last step it is now unchanged.
        s.filter(&r, Filter::Changed);
        assert_eq!(s.remaining(), 0);
    }

    #[test]
    fn a_sixteen_bit_search_reads_little_endian_pairs() {
        let mut s = Search::new(Width::U16);
        let mut r = ram(8);
        r[4] = 0xFF;
        r[5] = 0x00;
        s.snapshot(&r);
        // 0x00FF -> 0x0100 is an increase only if the pair is read as one
        // 16-bit value; the low byte alone went down.
        r[4] = 0x00;
        r[5] = 0x01;
        s.filter(&r, Filter::Increased);
        assert!(s.remaining() >= 1);

        // The same pair read a byte at a time says the opposite: $04 went
        // from FF down to 00.
        let mut narrow = Search::new(Width::U8);
        r[4] = 0xFF;
        r[5] = 0x00;
        narrow.snapshot(&r);
        r[4] = 0x00;
        r[5] = 0x01;
        narrow.filter(&r, Filter::Increased);
        assert_eq!(narrow.remaining(), 1);
    }

    /// The last offset has no high byte, so a 16-bit search must not offer it.
    #[test]
    fn a_sixteen_bit_search_excludes_the_final_offset() {
        let mut s = Search::new(Width::U16);
        s.snapshot(&ram(8));
        assert_eq!(s.remaining(), 7);
    }

    #[test]
    fn filtering_before_a_snapshot_is_a_no_op() {
        let mut s = Search::new(Width::U8);
        s.filter(&ram(8), Filter::Changed);
        assert!(!s.is_started());
        assert_eq!(s.remaining(), 0);
    }

    /// Arming is not a change. Every byte holds a value at that moment, and
    /// reporting all of them would drown the first real report.
    #[test]
    fn arming_does_not_count_as_a_change() {
        let mut p = Probe::new();
        let mut r = ram(16);
        r[2] = 7;
        p.tick(&r, 100);
        // Nothing has moved since arming, so the stable interval is zero and
        // no byte qualifies however wide the recent window is.
        assert_eq!(p.qualifying(&r, 100, 1000, 1), 0);
    }

    #[test]
    fn a_byte_that_holds_still_then_moves_is_reported() {
        let mut p = Probe::new();
        let mut r = ram(16);
        p.tick(&r, 0);
        for f in 1..500 {
            p.tick(&r, f);
        }
        r[5] = 1;
        p.tick(&r, 500);
        assert_eq!(p.qualifying(&r, 500, 10, 100), 1);
    }

    /// A byte that changes every frame — a position, a timer, the RNG — is
    /// exactly what the stable window exists to throw out.
    #[test]
    fn a_constantly_busy_byte_is_filtered_out() {
        let mut p = Probe::new();
        let mut r = ram(16);
        p.tick(&r, 0);
        for f in 1..500 {
            r[9] = (f % 251) as u8;
            p.tick(&r, f);
        }
        assert_eq!(p.qualifying(&r, 499, 10, 100), 0);
    }

    /// A change from long before the key press is not what was asked about.
    #[test]
    fn a_stale_change_falls_outside_the_recent_window() {
        let mut p = Probe::new();
        let mut r = ram(16);
        p.tick(&r, 0);
        for f in 1..200 {
            p.tick(&r, f);
        }
        r[5] = 1;
        p.tick(&r, 200);
        for f in 201..600 {
            p.tick(&r, f);
        }
        assert_eq!(p.qualifying(&r, 599, 30, 100), 0);
    }

    #[test]
    fn a_reset_re_arms_against_the_replaced_ram() {
        let mut p = Probe::new();
        let mut r = ram(16);
        p.tick(&r, 0);
        for f in 1..300 {
            p.tick(&r, f);
        }
        // A savestate load rewrites RAM wholesale; without re-arming, every
        // byte that differs would look like a one-shot event.
        p.reset();
        r.iter_mut().for_each(|b| *b = 0xAA);
        p.tick(&r, 0);
        assert_eq!(p.qualifying(&r, 0, 1000, 1), 0);
    }
}
