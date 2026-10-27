//! The watcher: a small interpreter over the rules a manifest declares.
//!
//! Both harnesses this was extracted from had a hand-written `tick` that read a
//! fixed struct out of RAM and then ran a list of `if prev.x != cur.x` arms.
//! Put side by side, the two `tick`s differ only in *which* field, *which* kind
//! of edge, and *what* it means — and that residue is small enough to be data.
//! So it is data: [`crate::manifest::Rule`] is one of those `if` arms, and this
//! module runs them.
//!
//! Three ideas do all the work, and each was learned the hard way in one of the
//! two originals:
//!
//! * **Edges, not levels.** A rule fires on a transition. Holding a flag at a
//!   value is one event, not one per frame.
//! * **An attempt ends once.** The first terminal event wins and the rest of
//!   the attempt is quiet, so a game that walks its own menus on the way back
//!   to the map reports once. `attempt.reset_on` re-opens it — which SMB3
//!   needs, and the absence of that re-opening was a real bug: a latch held for
//!   the whole process meant one card early in a run silenced every later
//!   death.
//! * **Markers time an event the game announces late.** DKC's fade starts
//!   before the overworld finishes loading, and ADR 0005 says the hand-off is
//!   not the player's time. A marker arms on the fade, expires if nothing comes
//!   of it, and a terminal rule is timed to it rather than to the frame it
//!   noticed.
//!
//! What this deliberately is *not* is a scripting language. There are no
//! expressions, no arithmetic, and no state a manifest can invent — a rule
//! reads one field's transition, optionally guarded by one other field's
//! current value. Everything the two games needed fits; anything that does not
//! is a sign the game wants a real address found, not a cleverer manifest.

use std::collections::BTreeMap;

use session_api::SessionEvent;

use crate::manifest::{Edge, Emit, Manifest, Marker, Rule, TestKind, WatchDef};
use crate::memory::{AddressSpace, Field};

/// A rule with its field names resolved to indices.
struct Compiled {
    rule: Rule,
    field: usize,
    guard: Option<(usize, TestKind)>,
    /// Index into `markers`, from `timed_to`.
    marker: Option<usize>,
    /// Index into that marker's captures, from `if_marked`.
    capture: Option<usize>,
    terminal: bool,
}

struct CompiledMarker {
    marker: Marker,
    field: usize,
    captures: Vec<(usize, TestKind)>,
}

#[derive(Clone, Default)]
struct MarkerState {
    armed_at: Option<u64>,
    /// Which of the marker's captures held at the moment it armed. Read then
    /// rather than when the verdict lands, because by that point the thing
    /// being captured — a pause menu — is long gone.
    captured: Vec<bool>,
}

pub struct Watcher {
    space: AddressSpace,
    fields: Vec<Field>,
    names: Vec<String>,
    notes: Vec<String>,
    labels: Vec<BTreeMap<u32, String>>,
    display: Vec<usize>,
    markers: Vec<CompiledMarker>,
    rules: Vec<Compiled>,
    reset_on: Vec<(usize, Edge)>,

    prev: Option<Vec<Option<u32>>>,
    ended: bool,
    marker_state: Vec<MarkerState>,
    events: Vec<SessionEvent>,

    /// Extra raw addresses to report on change, from `--watch`.
    watched: Vec<usize>,
    prev_watched: Vec<Option<u8>>,
    /// Whether to print the running commentary. Off in tests, and off for a
    /// caller that wants the events and not the console.
    pub log: bool,
}

impl Watcher {
    pub fn new(manifest: &Manifest, watched: Vec<usize>) -> Watcher {
        // Every name was proved to resolve while the manifest was validated,
        // which is why this can index rather than return an error.
        let index = |name: &str| -> usize {
            manifest
                .watches
                .iter()
                .position(|w| w.name == name)
                .expect("manifest validation proved every field name resolves")
        };

        let markers: Vec<CompiledMarker> = manifest
            .markers
            .iter()
            .map(|m| CompiledMarker {
                field: index(&m.field),
                captures: m.captures.iter().map(|(_, t)| (index(&t.field), t.kind)).collect(),
                marker: m.clone(),
            })
            .collect();

        let rules = manifest
            .rules
            .iter()
            .map(|r| {
                let marker =
                    r.timed_to.as_ref().map(|m| markers.iter().position(|x| &x.marker.name == m).unwrap());
                let capture = match (&r.if_marked, marker) {
                    (Some(c), Some(mi)) => {
                        Some(markers[mi].marker.captures.iter().position(|(n, _)| n == c).unwrap())
                    }
                    _ => None,
                };
                Compiled {
                    field: index(&r.field),
                    guard: r.guard.as_ref().map(|g| (index(&g.field), g.kind)),
                    marker,
                    capture,
                    terminal: matches!(r.emit, Emit::StageCleared | Emit::StageExited(_)),
                    rule: r.clone(),
                }
            })
            .collect();

        let marker_state = markers
            .iter()
            .map(|m| MarkerState { armed_at: None, captured: vec![false; m.captures.len()] })
            .collect();

        Watcher {
            space: manifest.libretro.space,
            fields: manifest
                .watches
                .iter()
                .map(|w: &WatchDef| Field { at: w.at, width: w.width })
                .collect(),
            names: manifest.watches.iter().map(|w| w.name.clone()).collect(),
            notes: manifest.watches.iter().map(|w| w.note.clone()).collect(),
            labels: manifest
                .watches
                .iter()
                .map(|w| manifest.labels.get(&w.name).cloned().unwrap_or_default())
                .collect(),
            display: manifest.display.iter().map(|n| index(n)).collect(),
            markers,
            rules,
            reset_on: manifest.reset_on.iter().map(|t| (index(&t.field), t.edge.clone())).collect(),
            prev: None,
            ended: false,
            marker_state,
            events: Vec::new(),
            prev_watched: vec![None; watched.len()],
            watched,
            log: true,
        }
    }

    /// Called when a stage is entered or the core is reset: nothing from before
    /// is meaningful, including which stage it was.
    pub fn reset(&mut self) {
        self.prev = None;
        self.ended = false;
        for m in &mut self.marker_state {
            m.armed_at = None;
            m.captured.iter_mut().for_each(|c| *c = false);
        }
        self.prev_watched.fill(None);
        self.events.clear();
    }

    /// Reads RAM and returns everything that fired this frame.
    ///
    /// `frame` counts emulated frames since stage entry (ADR 0005). A returned
    /// event's own frame is not always this one: a rule timed to a marker
    /// reports the frame the marker armed on, which is when the thing actually
    /// happened.
    pub fn tick(&mut self, ram: &[u8], frame: u64) -> &[SessionEvent] {
        self.events.clear();
        let cur: Vec<Option<u32>> = self.fields.iter().map(|f| f.read(&self.space, ram)).collect();
        self.report_watched(ram, frame);

        let Some(prev) = self.prev.take() else {
            if self.log {
                println!("[ram] frame {frame}  {}", self.display_line(&cur));
            }
            self.prev = Some(cur);
            return &self.events;
        };

        // An attempt re-opening is evaluated first, so a frame that both closes
        // one attempt and opens the next behaves the way the manifest reads.
        for i in 0..self.reset_on.len() {
            let (field, edge) = (&self.reset_on[i].0, &self.reset_on[i].1);
            if self.ended && edge_fires(edge, prev[*field], cur[*field]) {
                let name = self.names[*field].clone();
                self.ended = false;
                if self.log {
                    println!("[stage] attempt re-opened by {name} at frame {frame}");
                }
            }
        }

        self.expire_markers(frame);
        self.arm_markers(&prev, &cur, frame);

        let mut fired: Vec<usize> = Vec::new();
        for i in 0..self.rules.len() {
            // Deciding and acting are separated so the decision can read the
            // whole watcher and the action can write to it.
            let (at, consume) = {
                let r = &self.rules[i];
                let (before, after) = (prev[r.field], cur[r.field]);
                if !edge_fires(&r.rule.edge, before, after) {
                    continue;
                }
                if let Some((f, t)) = r.guard {
                    if !holds(t, cur[f]) {
                        continue;
                    }
                }
                if r.rule.unless.iter().any(|n| {
                    fired.iter().any(|&j| self.rules[j].rule.name.as_deref() == Some(n.as_str()))
                }) {
                    continue;
                }

                let mut at = frame;
                let mut consume = None;
                if let Some(mi) = r.marker {
                    let state = &self.marker_state[mi];
                    if let Some(ci) = r.capture {
                        if state.armed_at.is_none() || !state.captured[ci] {
                            continue;
                        }
                    }
                    // Without a fresh marker, this frame is the best answer
                    // there is, and is better than refusing to report the
                    // ending at all.
                    if let Some(armed) = state.armed_at {
                        at = armed;
                        if r.terminal {
                            consume = Some(mi);
                        }
                    }
                }

                if r.terminal && self.ended {
                    if self.log {
                        println!(
                            "[stage] {} moved again at frame {frame}, but the attempt already ended",
                            self.names[r.field]
                        );
                    }
                    continue;
                }
                (at, consume)
            };

            if let Some(mi) = consume {
                self.marker_state[mi].armed_at = None;
            }
            if self.rules[i].terminal {
                self.ended = true;
            }
            fired.push(i);
            self.fire(i, at, frame, prev[self.rules[i].field], cur[self.rules[i].field]);
        }

        if self.log && self.display.iter().any(|&i| prev[i] != cur[i]) {
            println!("[ram] frame {frame}  {}", self.display_line(&cur));
        }
        self.prev = Some(cur);
        &self.events
    }

    /// A marker nothing came of must not be left armed to mis-time a later,
    /// unrelated ending — and expiring it must not block a fresh one arriving
    /// on the same frame. That ordering was a real bug in the DKC harness, and
    /// the test that caught it survives as `a_fade_that_led_nowhere_...`.
    fn expire_markers(&mut self, frame: u64) {
        for i in 0..self.markers.len() {
            let Some(at) = self.marker_state[i].armed_at else { continue };
            if frame.saturating_sub(at) >= self.markers[i].marker.window_frames {
                self.marker_state[i].armed_at = None;
                if self.log {
                    println!(
                        "[stage] the {} at frame {at} led nowhere; discarding it",
                        self.markers[i].marker.name
                    );
                }
            }
        }
    }

    fn arm_markers(&mut self, prev: &[Option<u32>], cur: &[Option<u32>], frame: u64) {
        for i in 0..self.markers.len() {
            let m = &self.markers[i];
            if !edge_fires(&m.marker.edge, prev[m.field], cur[m.field]) {
                continue;
            }
            let captured: Vec<bool> =
                m.captures.iter().map(|(f, t)| holds(*t, cur[*f])).collect();
            if self.log {
                let held: Vec<&str> = m
                    .marker
                    .captures
                    .iter()
                    .zip(&captured)
                    .filter(|(_, c)| **c)
                    .map(|((n, _), _)| n.as_str())
                    .collect();
                let name = m.marker.name.to_uppercase();
                let suffix =
                    if held.is_empty() { String::new() } else { format!("  ({})", held.join(", ")) };
                println!("[event] frame {frame}  {name}{suffix}");
            }
            self.marker_state[i].armed_at = Some(frame);
            self.marker_state[i].captured = captured;
        }
    }

    /// Turn one fired rule into events, and say so on the console.
    ///
    /// A per-bit rule produces one event per newly set bit, which is how four
    /// KONG letters come out of one byte without four rules.
    fn fire(&mut self, i: usize, at: u64, frame: u64, before: Option<u32>, after: Option<u32>) {
        let (p, c) = match (before, after) {
            (Some(p), Some(c)) => (p, c),
            _ => return,
        };
        let hits: Vec<(u32, Option<String>)> = match self.rules[i].rule.edge {
            Edge::Increases => vec![(c - p, None)],
            Edge::BitsSet => newly_set(p, c)
                .into_iter()
                .map(|b| {
                    let label = self.rules[i]
                        .rule
                        .bit_labels
                        .get(b as usize)
                        .cloned()
                        .unwrap_or_else(|| b.to_string());
                    (1, Some(label))
                })
                .collect(),
            _ => vec![(1, None)],
        };

        for (count, label) in hits {
            let emit = self.rules[i].rule.emit.clone();
            let event = match &emit {
                Emit::StageCleared => Some(SessionEvent::StageCleared { frame: at }),
                Emit::StageExited(reason) => {
                    Some(SessionEvent::StageExited { frame: at, reason: *reason })
                }
                Emit::Died => Some(SessionEvent::Died { frame: at }),
                Emit::Damaged => Some(SessionEvent::Damaged { frame: at }),
                Emit::EnemyDefeated => Some(SessionEvent::EnemyDefeated { frame: at, count }),
                Emit::Item(item) => Some(SessionEvent::ItemCollected {
                    frame: at,
                    // A per-bit item names one item per bit, so `kong_letter`
                    // becomes `kong_letter:K`.
                    item: match &label {
                        Some(l) => format!("{item}:{l}"),
                        None => item.clone(),
                    },
                    count,
                }),
                Emit::Count(counter) => Some(SessionEvent::Counted {
                    frame: at,
                    counter: match &label {
                        Some(l) => format!("{counter}:{l}"),
                        None => counter.clone(),
                    },
                    count,
                }),
                Emit::Objective(name) => {
                    Some(SessionEvent::ObjectiveReached { frame: at, objective: name.clone() })
                }
                Emit::Note => None,
            };
            if let Some(e) = event {
                self.events.push(e);
            }
            if self.log {
                self.report(i, &emit, &label, (at, frame), (p, c));
            }
        }
    }

    /// `frames` is (the frame the event is reported *at*, the frame it was
    /// noticed on); those differ when a marker timed it.
    fn report(
        &self,
        i: usize,
        emit: &Emit,
        label: &Option<String>,
        frames: (u64, u64),
        values: (u32, u32),
    ) {
        let ((at, frame), (p, c)) = (frames, values);
        let field = self.rules[i].field;
        let what = match emit {
            Emit::StageCleared => "STAGE CLEARED".to_string(),
            Emit::StageExited(reason) => format!("STAGE EXITED ({reason})"),
            Emit::Died => "DEATH".to_string(),
            Emit::Damaged => "DAMAGE".to_string(),
            Emit::EnemyDefeated => "ENEMY DEFEATED".to_string(),
            Emit::Item(item) => match label {
                Some(l) => format!("ITEM {item}:{l}"),
                None => format!("ITEM {item}"),
            },
            Emit::Count(counter) => match label {
                Some(l) => format!("COUNT {counter}:{l}"),
                None => format!("COUNT {counter}"),
            },
            Emit::Objective(name) => format!("OBJECTIVE {name}"),
            Emit::Note => "NOTE".to_string(),
        };
        let movement = format!("{} {p} -> {}", self.names[field], self.labelled(field, c));
        let extra = match &self.rules[i].rule.log {
            Some(text) => format!("  {text}"),
            None => String::new(),
        };
        let timing = if at != frame { format!("  (timed to frame {at})") } else { String::new() };
        println!("[event] frame {frame}  {what}  {movement}{extra}{timing}");
    }

    fn labelled(&self, i: usize, v: u32) -> String {
        match self.labels[i].get(&v) {
            Some(name) => format!("{v} ({name})"),
            None => v.to_string(),
        }
    }

    fn display_line(&self, cur: &[Option<u32>]) -> String {
        self.display
            .iter()
            .map(|&i| {
                let v = match cur[i] {
                    Some(v) => self.labelled(i, v),
                    None => "-".to_string(),
                };
                format!("{} {}", self.names[i], v)
            })
            .collect::<Vec<_>>()
            .join("  ")
    }

    /// Every field and every `--watch` address, printed unconditionally (F5).
    pub fn dump(&self, ram: &[u8], frame: u64) {
        println!("[ram] frame {frame}  dump");
        for (i, f) in self.fields.iter().enumerate() {
            let shown = f.show(&self.space);
            let name = &self.names[i];
            match f.read(&self.space, ram) {
                Some(v) => println!(
                    "        {name:<14} {shown:<18} = {:<16} {}",
                    self.labelled(i, v),
                    self.notes[i]
                ),
                None => {
                    println!("        {name:<14} {shown:<18} = {:<16} {}", "-", self.notes[i])
                }
            }
        }
        for &addr in &self.watched {
            let at = self.space.show(addr);
            match self.space.read_u8(addr, ram) {
                Some(b) => println!("        {at} = {b:02X} ({b})"),
                None => println!("        {at} = <unmapped>"),
            }
        }
    }

    /// Fields whose address is set but which the core cannot resolve.
    ///
    /// This is the general form of a warning the NES harness wrote by hand:
    /// SMB3's card counter is in cartridge WRAM, and a core that publishes no
    /// memory map cannot reach it, so the completion detector silently never
    /// fires. Anything unreadable is worth saying once, at startup, in every
    /// game.
    pub fn unreadable(&self, ram: &[u8]) -> Vec<String> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.at.is_some() && f.read(&self.space, ram).is_none())
            .map(|(i, f)| format!("{} at {}", self.names[i], f.show(&self.space)))
            .collect()
    }

    /// Fields with no address at all, and so no detector.
    pub fn unset(&self) -> Vec<&str> {
        self.fields
            .iter()
            .enumerate()
            .filter(|(_, f)| f.at.is_none())
            .map(|(i, _)| self.names[i].as_str())
            .collect()
    }

    fn report_watched(&mut self, ram: &[u8], frame: u64) {
        if !self.log {
            return;
        }
        for i in 0..self.watched.len() {
            let addr = self.watched[i];
            let cur = self.space.read_u8(addr, ram);
            let Some(b) = cur else { continue };
            let at = self.space.show(addr);
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

/// Whether `edge` fired between `prev` and `cur`. `None` for either value — an
/// unset or unresolvable address — is never a fire, which is what keeps a
/// detector honest when nobody has found its address yet.
fn edge_fires(edge: &Edge, prev: Option<u32>, cur: Option<u32>) -> bool {
    let (Some(p), Some(c)) = (prev, cur) else { return false };
    match *edge {
        Edge::Increases => c > p,
        Edge::Decreases => c < p,
        Edge::Changes => c != p,
        Edge::Enters(v) => p != v && c == v,
        Edge::Leaves(v) => p == v && c != v,
        Edge::Reaches(v) => p < v && c >= v,
        Edge::BitSet(b) => p & (1 << b) == 0 && c & (1 << b) != 0,
        Edge::BitCleared(b) => p & (1 << b) != 0 && c & (1 << b) == 0,
        Edge::BitsSet => c & !p != 0,
    }
}

/// Every bit newly set between two values, low bit first.
fn newly_set(prev: u32, cur: u32) -> Vec<u32> {
    let gained = cur & !prev;
    (0..32).filter(|b| gained & (1 << b) != 0).collect()
}

fn holds(test: TestKind, value: Option<u32>) -> bool {
    let Some(v) = value else { return false };
    match test {
        TestKind::Eq(x) => v == x,
        TestKind::Ne(x) => v != x,
        TestKind::Ge(x) => v >= x,
        TestKind::Le(x) => v <= x,
        TestKind::BitSet(b) => v & (1 << b) != 0,
        TestKind::BitCleared(b) => v & (1 << b) == 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_field_never_fires() {
        assert!(!edge_fires(&Edge::Changes, None, Some(1)));
        assert!(!edge_fires(&Edge::Changes, Some(1), None));
    }

    /// `leaves 0` is how "rose from zero" is spelled, and it is an edge: a
    /// timer still counting down from an earlier hit is not a second hit.
    #[test]
    fn leaves_zero_is_how_rising_from_zero_is_spelled() {
        assert!(edge_fires(&Edge::Leaves(0), Some(0), Some(40)));
        assert!(!edge_fires(&Edge::Leaves(0), Some(40), Some(39)));
    }

    #[test]
    fn enters_is_an_edge_so_holding_a_value_fires_once() {
        assert!(edge_fires(&Edge::Enters(0x82), Some(0), Some(0x82)));
        assert!(!edge_fires(&Edge::Enters(0x82), Some(0x82), Some(0x82)));
    }

    /// `reaches` is a threshold crossed upward, so a count that jumps past the
    /// value still fires — the case `enters` misses — and one already past it
    /// does not fire again.
    #[test]
    fn reaches_fires_on_crossing_however_far_the_jump() {
        assert!(edge_fires(&Edge::Reaches(20), Some(19), Some(20)));
        assert!(edge_fires(&Edge::Reaches(20), Some(18), Some(23)));
        assert!(!edge_fires(&Edge::Reaches(20), Some(20), Some(21)));
        assert!(!edge_fires(&Edge::Reaches(20), Some(21), Some(19)));
        assert!(!edge_fires(&Edge::Enters(20), Some(18), Some(23)));
    }

    #[test]
    fn bit_edges_are_edges() {
        assert!(edge_fires(&Edge::BitCleared(0), Some(0b01), Some(0b00)));
        assert!(!edge_fires(&Edge::BitCleared(0), Some(0b00), Some(0b00)));
        assert!(edge_fires(&Edge::BitSet(2), Some(0b001), Some(0b101)));
    }

    #[test]
    fn newly_set_lists_only_the_new_bits() {
        assert_eq!(newly_set(0b0001, 0b0101), vec![2]);
        assert_eq!(newly_set(0b0000, 0b1010), vec![1, 3]);
        assert!(newly_set(0b1111, 0b1111).is_empty());
    }

    #[test]
    fn guards_read_the_current_value() {
        assert!(holds(TestKind::Ne(0), Some(9)));
        assert!(!holds(TestKind::Ne(0), Some(0)));
        assert!(holds(TestKind::BitSet(6), Some(0b0100_0000)));
        assert!(!holds(TestKind::BitSet(6), None));
    }

    #[test]
    fn ge_and_le_include_their_bound() {
        assert!(holds(TestKind::Ge(20), Some(20)));
        assert!(holds(TestKind::Ge(20), Some(21)));
        assert!(!holds(TestKind::Ge(20), Some(19)));
        assert!(holds(TestKind::Le(5), Some(5)));
        assert!(!holds(TestKind::Le(5), Some(6)));
        assert!(!holds(TestKind::Le(5), None));
    }
}
