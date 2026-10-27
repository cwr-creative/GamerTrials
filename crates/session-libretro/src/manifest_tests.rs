//! What the extraction has to not have broken.
//!
//! Every behavioural test the two harnesses had, re-run against the manifests
//! this crate actually ships — not against a fixture written to suit the new
//! code. If `manifests/dkc.toml` stops expressing "a bonus room is not
//! a clear", these fail, which is the only way a manifest-driven watcher can be
//! held to the same standard as the hand-written one it replaced.
//!
//! Two of the originals did not survive, and both for the same good reason. The
//! DKC harness had tests over a 230-entry level-name table and the "family" a
//! code belonged to; that table decided the verdict once, and by the end it
//! decided nothing — `in_stage` did. It is a label table in the manifest now,
//! spot-checked below for transcription rather than for behaviour.

use std::path::Path;

use session_api::{ChallengeId, ChallengeKind, EntryPoint, ExitReason, SessionEvent, StageId};

use crate::manifest::{Controller, Manifest};
use crate::memory::{Endian, Prefer, Width};
use crate::watch::Watcher;

const SMB3: &str = include_str!("../../../manifests/smb3.toml");
const DKC: &str = include_str!("../../../manifests/dkc.toml");
const DOOM: &str = include_str!("../../../manifests/doom.toml");

fn smb3() -> Manifest {
    Manifest::parse(SMB3, Path::new("manifests/smb3.toml")).expect("the shipped SMB3 manifest")
}

fn dkc() -> Manifest {
    Manifest::parse(DKC, Path::new("manifests/dkc.toml")).expect("the shipped DKC manifest")
}

fn doom() -> Manifest {
    Manifest::parse(DOOM, Path::new("manifests/doom.toml")).expect("the shipped DOOM manifest")
}

/// A game plus a RAM buffer to write into, driven a frame at a time.
struct Sim {
    manifest: Manifest,
    watcher: Watcher,
    ram: Vec<u8>,
}

impl Sim {
    fn new(manifest: Manifest, ram_bytes: usize) -> Sim {
        let mut watcher = Watcher::new(&manifest, Vec::new());
        watcher.log = false;
        Sim { manifest, watcher, ram: vec![0u8; ram_bytes] }
    }

    fn put(&mut self, field: &str, value: u32) {
        let w = self.manifest.watch(field).unwrap_or_else(|| panic!("no [watch.{field}]"));
        let at = w.at.expect("test wrote to a field with no address");
        let off = self.manifest.libretro.space.offset(at).expect("inside the slice");
        // The inverse of `AddressSpace::read`, so a test writes a field the
        // same way the watcher will read it back at any width.
        let n = w.width.bytes();
        for i in 0..n {
            let shift = match self.manifest.libretro.space.endian {
                Endian::Little => i,
                Endian::Big => n - 1 - i,
            };
            self.ram[off + i] = ((value >> (8 * shift)) & 0xFF) as u8;
        }
    }

    fn tick(&mut self, frame: u64) -> Vec<SessionEvent> {
        self.watcher.tick(&self.ram, frame).to_vec()
    }

    /// Quiet frames, for confirming nothing fires again.
    fn idle(&mut self, from: u64, to: u64) -> Vec<SessionEvent> {
        let mut out = Vec::new();
        for f in from..=to {
            out.extend(self.tick(f));
        }
        out
    }
}

fn cleared(events: &[SessionEvent]) -> Option<u64> {
    events.iter().find_map(|e| match e {
        SessionEvent::StageCleared { frame } => Some(*frame),
        _ => None,
    })
}

fn exited(events: &[SessionEvent]) -> Option<(u64, ExitReason)> {
    events.iter().find_map(|e| match e {
        SessionEvent::StageExited { frame, reason } => Some((*frame, *reason)),
        _ => None,
    })
}

fn items(events: &[SessionEvent]) -> Vec<(String, u32)> {
    events
        .iter()
        .filter_map(|e| match e {
            SessionEvent::ItemCollected { item, count, .. } => Some((item.clone(), *count)),
            _ => None,
        })
        .collect()
}

// ============================================================ every manifest

#[test]
fn every_shipped_manifest_loads() {
    let s = smb3();
    assert_eq!(s.id.0, "smb3");
    // Four vanilla stages, plus the two that run on the hand-authored hack
    // (ADR 0020).
    assert_eq!(s.stages.len(), 6);
    assert_eq!(s.roms.len(), 2);
    assert_eq!(s.base_rom().id, "base");
    assert!(s.stages.iter().filter(|st| st.rom == "hack1").count() == 2);
    let d = dkc();
    assert_eq!(d.id.0, "dkc");
    assert_eq!(d.stages.len(), 6);
    let m = doom();
    assert_eq!(m.id.0, "doom");
    assert_eq!(m.stages.len(), 2);
}

/// The two SMB3 custom challenges, pinned against `docs/games/smb3.md`.
///
/// Every number here was read out of a disassembly and argued for in that
/// document, and none of it can be re-derived from the manifest — so this test
/// is the link between the two. The duplicated `0x72`s are the one to watch: they
/// are the weighting, and a pool that quietly became a set would change what the
/// challenge is without changing a line of Rust.
#[test]
fn the_smb3_custom_challenges_carry_the_tables_the_docs_record() {
    let m = smb3();
    let hack = m.rom("hack1").expect("the hack is declared");
    assert_eq!(hack.from.as_deref(), Some("base"));
    assert_eq!(
        hack.sha1.as_deref(),
        Some("11f4851c7b4e8413432eed30b0a3760e5dcac638")
    );
    // A hack is a patch, never a file (ADR 0020).
    assert!(hack.path.is_none());
    assert!(hack.patch.is_some());

    // Sun Storm: three slots, the last of them fixed so the roll is one to three
    // suns rather than zero to three.
    for id in ["sun-storm-survival", "sun-storm-time-trial"] {
        let t = m.patch_table(&ChallengeId(id.into())).expect("generated");
        let offsets: Vec<usize> = t.slots.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, vec![0xC6BD, 0xC6C0, 0xC6C3], "{id}");
        assert_eq!(
            t.slots[2].values,
            content_gen::ValueSource::Pool(vec![0xAF]),
            "{id}: slot C is always a sun"
        );
    }

    // Bro Roulette: four independent slots over a multiset, so the roll is a coin
    // flip and then an even choice among four bros.
    for id in ["bro-roulette-time-trial", "bro-roulette-survival"] {
        let t = m.patch_table(&ChallengeId(id.into())).expect("generated");
        let offsets: Vec<usize> = t.slots.iter().map(|s| s.offset).collect();
        assert_eq!(offsets, vec![0xC6CB, 0xC6CE, 0xC6D1, 0xC6D4], "{id}");
        for slot in &t.slots {
            match &slot.values {
                content_gen::ValueSource::Pool(p) => {
                    assert_eq!(p.iter().filter(|v| **v == 0x72).count(), 4, "{id}");
                    assert_eq!(p.len(), 8, "{id}");
                }
                other => panic!("{id}: {other:?}"),
            }
        }
        assert!(t.groups.is_empty(), "{id}: this is not a permutation");
    }

    // Both stages run on the hack, and neither vanilla stage does.
    assert_eq!(m.stage("sun-storm").unwrap().rom, "hack1");
    assert_eq!(m.stage("1-1").unwrap().rom, "base");
    // A hack has its own name, and it is what a player is shown.
    assert_eq!(m.content_title(m.stage("sun-storm").unwrap()), "SMB3: Gamer Trials");
}

/// What a DOS game actually needed, pinned so a later refactor cannot quietly
/// undo it. The point of ADR 0018 was that this list is short and lives
/// entirely in the runner half of the schema.
#[test]
fn the_dos_game_differs_from_the_consoles_only_in_its_runner_block() {
    let m = doom();
    assert_eq!(m.console, "dos");
    // No pad. A DOS game is played on the keys the game itself defines.
    assert_eq!(m.libretro.controller, Controller::Keyboard);
    // No system-RAM slice to route through, so the map is the only way.
    assert_eq!(m.libretro.space.prefer, Prefer::MemoryMap);
    assert_eq!(m.libretro.system_ram_bytes, None);
    // C ints.
    assert_eq!(m.libretro.default_width, Width::U32);
    assert!(m.watches.iter().all(|w| w.width == Width::U32));
    // A window for the discovery tools, since there is no slice to hand them.
    let scan = m.libretro.scan.expect("doom declares a scan window");
    assert!(
        m.watches.iter().flat_map(|w| w.at).all(|a| a >= scan.at && a < scan.at + scan.size),
        "every watched address should be inside the scan window a contributor is pointed at"
    );
}

/// The cycles setting decides how many emulated frames a piece of play takes,
/// so two players on different values have incomparable scores (invariant 4).
/// It is part of the game's identity, and a manifest that stopped pinning it
/// would still load and still be wrong — hence a test rather than a comment.
#[test]
fn the_dos_game_pins_the_options_that_decide_what_a_frame_means() {
    let m = doom();
    assert_eq!(m.libretro.options.get("dosbox_pure_cycles").map(String::as_str), Some("77000"));
    assert_eq!(m.libretro.options.get("dosbox_pure_force60fps").map(String::as_str), Some("false"));
    // ...and the declared framerate is VGA's own refresh, which is what the
    // core reports once the second of those stops forcing 60.
    assert_eq!(m.framerate.0, 70.0866);
}

/// A `named` entry is how a state gets captured in the first place (ADR 0002),
/// so the two spellings are a pipeline rather than alternatives. Doom declares
/// one stage of each kind for the same place.
#[test]
fn the_dos_game_declares_both_an_authoring_entry_and_a_racing_one() {
    let m = doom();
    let named = m.stages.iter().filter(|s| matches!(s.entry, EntryPoint::Named(_))).count();
    let state = m.stages.iter().filter(|s| matches!(s.entry, EntryPoint::State(_))).count();
    assert_eq!((named, state), (1, 1));
    // A named entry cannot be reached without knowing when the boot has
    // finished, and the loader has no way to guess it.
    assert!(m.libretro.boot_frames > 0);
    // Every challenge races on the state, because a cold boot is ~960 frames
    // and a restore is ~60 — which is what a death in a time trial costs.
    for c in &m.challenges {
        let stage = m.stage(&c.stage.0).expect("validation proved the stage exists");
        assert!(
            matches!(stage.entry, EntryPoint::State(_)),
            "challenge {} races on a cold boot",
            c.id.0
        );
    }
}

/// A console manifest declares none of the DOS machinery, and picks up no
/// surprising defaults from it.
#[test]
fn the_console_manifests_are_unaffected_by_the_dos_fields() {
    for m in [smb3(), dkc()] {
        assert!(matches!(m.libretro.controller, Controller::Pad(_)));
        assert_eq!(m.libretro.scan, None);
        assert_eq!(m.libretro.boot_frames, 0);
        assert!(m.libretro.options.is_empty());
    }
}

/// A forced-scrolling level cannot be a time trial: everyone finishes it in the
/// same number of frames. That is not declared anywhere any more — it is
/// expressed by the time trial simply not existing (ADR 0017), which is the
/// same fact with one source of truth instead of two.
#[test]
fn forced_scrolling_stages_carry_no_time_trial() {
    for (m, stage) in [(smb3(), "1-4"), (dkc(), "mine-cart-carnage")] {
        let id = StageId(stage.to_string());
        let kinds: Vec<ChallengeKind> = m.challenges_on(&id).map(|c| c.kind).collect();
        assert!(!kinds.is_empty(), "{stage} has no challenges at all");
        assert!(
            kinds.iter().all(|k| *k == ChallengeKind::Survival),
            "{stage} should offer survival only, got {kinds:?}"
        );
    }
}

/// Every stage that can host both kinds does, and every challenge points at a
/// stage that exists. The loader proves the second; this pins the first, which
/// is content rather than schema and so is easy to half-finish.
#[test]
fn every_shipped_stage_can_be_dealt() {
    for m in [smb3(), dkc()] {
        for s in &m.stages {
            assert!(
                m.challenges_on(&s.id).next().is_some(),
                "{}: stage `{}` carries no challenge, so it can never be dealt",
                m.id,
                s.id
            );
        }
    }
}

/// Several challenges on one stage is the normal case. Both manifests carry an
/// example, because a schema that only ever sees one per stage is a schema
/// whose second case is untested.
#[test]
fn a_stage_can_carry_more_than_one_challenge() {
    let d = dkc();
    let mine = StageId("mine-cart-carnage".to_string());
    let on: Vec<&str> = d.challenges_on(&mine).map(|c| c.id.0.as_str()).collect();
    assert_eq!(on, vec!["mine-cart-carnage-survival", "mine-cart-carnage-45s"]);

    // The second one is the only criterion that ends a run without the player
    // doing anything, and it is authored in seconds and stored in frames.
    let c = d.challenge("mine-cart-carnage-45s").unwrap();
    assert_eq!(c.kind, ChallengeKind::Survival);
    assert_eq!(c.complete_when.survive_frames, Some(d.framerate.frames(45.0)));
    assert!(!c.complete_when.cleared);
}

/// Criteria other than "reach the end", in both directions: a count of a fact,
/// and a conjunction of several. Both are shipped rather than only documented.
#[test]
fn shipped_challenges_use_criteria_other_than_cleared() {
    let stomps = smb3().challenge("1-1-stomp-rush").unwrap().complete_when.clone();
    assert_eq!(stomps.counts.get(session_api::ENEMY_DEFEATED), Some(&5));
    assert!(!stomps.cleared, "the clock stops on the fifth stomp, not at the flagpole");

    // All four letters *and* the exit. The per-bit rule is what makes
    // `kong_letter:K` the name, and the loader refuses any other spelling.
    let kong = dkc().challenge("jungle-hijinx-kong").unwrap().complete_when.clone();
    assert!(kong.cleared);
    assert_eq!(
        kong.counts.keys().cloned().collect::<Vec<_>>(),
        ["kong_letter:G", "kong_letter:K", "kong_letter:N", "kong_letter:O"]
    );
    assert!(kong.counts.values().all(|&n| n == 1));
}

/// The spec that crosses into `challenge-core`. It names a stage and never the
/// state file behind it: no adapter vocabulary crosses the boundary
/// (invariant 3).
#[test]
fn a_challenge_spec_carries_no_adapter_vocabulary() {
    let d = dkc();
    let spec = d.challenge_spec(d.challenge("jungle-hijinx-time-trial").unwrap());
    assert_eq!(spec.game.0, "dkc");
    assert_eq!(spec.stage.0, "jungle-hijinx");
    assert_eq!(spec.kind, ChallengeKind::TimeTrial);
    assert_eq!(spec.name, "Jungle Hijinx");
    assert!(spec.complete_when.cleared);
}

/// Spot-checks against the RainbowZ Editor's table, which is indexed by the
/// value at $7E003E. These indices are the whole reason the table exists, so a
/// transcription slip has to fail loudly.
#[test]
fn the_dkc_level_table_survived_the_move_into_the_manifest() {
    let d = dkc();
    let l = &d.labels["level_id"];
    assert_eq!(l.len(), 230);
    assert_eq!(l[&0x16], "Jungle Hijinx (level start)");
    assert_eq!(l[&0x06], "Jungle Hijinx - Bonus 1");
    assert_eq!(l[&0x08], "Jungle Hijinx (from Bonus 1)");
    assert_eq!(l[&0x25], "Jungle Hijinx (from Save)");
    assert_eq!(l[&0x01], "Reptile Rumble (level start)");
    assert_eq!(l.get(&230), None);
}

// ==================================================================== SMB3
//
// The NES buffer is 0x8000 rather than the real 2 KB so that a test can write
// the card counter at $7D9C, which on hardware is cartridge WRAM reached
// through the core's memory map. No map is published under `cargo test`, so
// the slice is the only route and a longer slice is what makes the address
// writable at all. This is what the original harness's tests did too.
const NES_TEST_RAM: usize = 0x8000;

fn mario() -> Sim {
    Sim::new(smb3(), NES_TEST_RAM)
}

#[test]
fn first_tick_is_baseline_only() {
    let mut g = mario();
    assert!(g.tick(1).is_empty());
}

#[test]
fn return_to_map_with_no_card_exits_the_stage() {
    let mut g = mario();
    g.tick(1);
    g.put("return_to_map", 1);
    // Honestly `unknown`: $0014 alone cannot tell a death from a card-less
    // level cleared normally, and saying so is the point.
    assert_eq!(exited(&g.tick(2)), Some((2, ExitReason::Unknown)));
}

#[test]
fn a_card_clears_the_stage() {
    let mut g = mario();
    g.tick(1);
    g.put("cards", 1);
    assert_eq!(cleared(&g.tick(2)), Some(2));
}

/// The card fires seconds before $0014 moves; that later rise is the same
/// level's ordinary exit and must not be reported as a second ending.
#[test]
fn the_exit_after_a_card_is_not_a_second_ending() {
    let mut g = mario();
    g.tick(1);
    g.put("cards", 1);
    assert_eq!(cleared(&g.tick(2)), Some(2));
    g.put("return_to_map", 1);
    assert!(g.tick(3).is_empty());
}

/// Regression, and the reason `attempt.reset_on` exists at all: the
/// once-per-attempt latch used to hold for the whole process, so one card
/// early in a run silenced every later death.
#[test]
fn a_death_after_a_card_still_exits() {
    let mut g = mario();
    g.tick(1);

    g.put("cards", 1);
    assert_eq!(cleared(&g.tick(2)), Some(2));
    g.put("return_to_map", 1);
    assert!(g.tick(3).is_empty());
    // The flag clearing is the hand-off to the map finishing, which re-opens
    // the attempt.
    g.put("return_to_map", 0);
    assert!(g.tick(4).is_empty());

    g.put("return_to_map", 1);
    assert_eq!(exited(&g.tick(5)), Some((5, ExitReason::Unknown)));
}

#[test]
fn two_deaths_both_report() {
    let mut g = mario();
    g.tick(1);
    g.put("return_to_map", 1);
    assert!(exited(&g.tick(2)).is_some());
    g.put("return_to_map", 0);
    assert!(g.tick(3).is_empty());
    g.put("return_to_map", 1);
    assert!(exited(&g.tick(4)).is_some());
}

/// A flag that stays high reports once, not every frame it is set.
#[test]
fn a_held_return_to_map_fires_once() {
    let mut g = mario();
    g.tick(1);
    g.put("return_to_map", 1);
    assert!(exited(&g.tick(2)).is_some());
    assert!(g.tick(3).is_empty());
    assert!(g.tick(4).is_empty());
}

#[test]
fn a_reset_clears_the_attempt() {
    let mut g = mario();
    g.tick(1);
    g.put("return_to_map", 1);
    assert!(exited(&g.tick(2)).is_some());

    // A state load restarts the stage with the flag still set; the next rise
    // has to be detectable from a fresh baseline.
    g.watcher.reset();
    assert!(g.tick(1).is_empty());
    g.put("return_to_map", 0);
    g.tick(2);
    g.put("return_to_map", 1);
    assert!(exited(&g.tick(3)).is_some());
}

/// Two detectors SMB3 had the addresses for but never emitted events from,
/// because the harness only printed them. Expressing them as rules made them
/// real without anyone finding a new address.
#[test]
fn smb3_now_reports_damage_and_stomps_as_events() {
    let mut g = mario();
    g.tick(1);
    g.put("hit_timer", 40);
    assert!(g.tick(2).contains(&SessionEvent::Damaged { frame: 2 }));
    // Counting down from that hit is not a second one.
    g.put("hit_timer", 39);
    assert!(!g.tick(3).contains(&SessionEvent::Damaged { frame: 3 }));

    g.put("stomp_counter", 2);
    assert!(g.tick(4).contains(&SessionEvent::EnemyDefeated { frame: 4, count: 2 }));
}

/// One award of points is one count, however many points it was and whether
/// or not the low byte wrapped. Bro Roulette's criteria count kills this way.
#[test]
fn smb3_counts_each_award_of_points_once() {
    let award = |frame| SessionEvent::Counted { frame, counter: "score_award".into(), count: 1 };
    let mut g = mario();
    g.put("score_lo", 0xF0);
    g.tick(1);
    // 100 points is ten in the byte, and still one award.
    g.put("score_lo", 0xFA);
    assert!(g.tick(2).contains(&award(2)));
    // 200 more carries into the middle byte, so the low byte falls.
    g.put("score_lo", 0x0E);
    g.put("score_mid", 0x01);
    assert!(g.tick(3).contains(&award(3)));
    // Nothing awarded, nothing counted.
    assert!(!g.tick(4).iter().any(|e| matches!(e, SessionEvent::Counted { .. })));

    let m = smb3();
    for id in ["bro-roulette-time-trial", "bro-roulette-survival"] {
        let c = &m.challenge(id).unwrap().complete_when;
        assert_eq!(c.counts.get("score_award"), Some(&4), "{id}");
        assert_eq!(c.counts.get(session_api::ENEMY_DEFEATED), None, "{id}: not stomps");
    }
}

// ===================================================================== DKC

const SNES_WRAM: usize = 0x2_0000;

// Real level codes. They no longer decide anything, but a bonus room still has
// to not be a clear, so the tests walk a route a player would take.
const JUNGLE_START: u32 = 0x16;
const JUNGLE_BONUS_1: u32 = 0x06;
const JUNGLE_FROM_BONUS_1: u32 = 0x08;
const JUNGLE_FROM_SAVE: u32 = 0x25;
const WINKY_BONUS: u32 = 0xD3;
const FADE_OUT: u32 = 0x82;
const TWO_KONGS: u32 = 1 << 0;
const PAUSED: u32 = 1 << 6;
const WINDOW: u64 = 120;

/// An attempt inside a stage, past its baseline tick. `in_stage` reads 0,
/// which is what a state taken inside a level looks like.
fn kong() -> Sim {
    let mut g = Sim::new(dkc(), SNES_WRAM);
    g.put("lives", 5);
    g.put("level_id", JUNGLE_START);
    g.put("in_stage", 0);
    g.put("state_flags", TWO_KONGS);
    g.tick(1);
    g
}

/// One rising edge to the fade value and back down, the way a real transition
/// does. Timing only — on its own this decides nothing.
fn fade(g: &mut Sim, at: u64) -> Vec<SessionEvent> {
    g.put("fade_type", FADE_OUT);
    let ev = g.tick(at);
    g.put("fade_type", 0);
    ev
}

/// The room changing under the player. Inside one stage this is a bonus room
/// or a checkpoint; it is not an exit.
fn land_on(g: &mut Sim, code: u32, at: u64) -> Vec<SessionEvent> {
    g.put("level_id", code);
    g.tick(at)
}

/// The overworld map coming up: the stage is over.
fn to_overworld(g: &mut Sim, at: u64) -> Vec<SessionEvent> {
    g.put("in_stage", 1);
    g.tick(at)
}

#[test]
fn losing_a_life_is_a_death_and_ends_the_stage() {
    let mut g = kong();
    g.put("lives", 4);
    let ev = g.tick(2);
    assert!(ev.contains(&SessionEvent::Died { frame: 2 }));
    assert_eq!(exited(&ev), Some((2, ExitReason::Death)));
}

/// A death does not put the overworld on screen — DKC drops the player back
/// into the level — so the reload that follows one is not an exit, and the
/// attempt is failed rather than cleared.
#[test]
fn a_death_and_its_level_reload_are_never_a_clear() {
    let mut g = kong();
    g.put("lives", 4);
    let ev = g.tick(2);
    assert_eq!(exited(&ev), Some((2, ExitReason::Death)));
    assert_eq!(cleared(&ev), None);

    let ev = land_on(&mut g, JUNGLE_START, 60);
    assert_eq!(cleared(&ev), None);
    assert_eq!(cleared(&g.idle(61, 400)), None);
}

/// The plain clear: a fade for the timing, the overworld for the verdict. The
/// reported frame is the fade, not the later map load — the hand-off is not
/// part of the player's time (ADR 0005).
#[test]
fn reaching_the_overworld_clears_the_stage_timed_to_the_fade() {
    let mut g = kong();
    fade(&mut g, 900);
    assert_eq!(cleared(&to_overworld(&mut g, 930)), Some(900));
}

/// The case that broke fade counting, and the case the old family table was
/// built for. A bonus room raises the same fade the exit does, but the player
/// is still inside a stage, so nothing scores and the attempt stays open.
#[test]
fn a_bonus_room_is_not_a_clear_and_the_real_exit_still_is() {
    let mut g = kong();
    fade(&mut g, 100);
    assert_eq!(cleared(&land_on(&mut g, JUNGLE_BONUS_1, 130)), None);

    fade(&mut g, 400);
    assert_eq!(cleared(&land_on(&mut g, JUNGLE_FROM_BONUS_1, 430)), None);

    fade(&mut g, 900);
    assert_eq!(cleared(&to_overworld(&mut g, 930)), Some(900));
}

/// The property the whole design exists for: two routes through one stage,
/// same verdict and same time.
#[test]
fn the_verdict_does_not_depend_on_the_route() {
    let mut g = kong();
    fade(&mut g, 900);
    let direct = cleared(&to_overworld(&mut g, 930));

    let mut g = kong();
    fade(&mut g, 100);
    land_on(&mut g, JUNGLE_BONUS_1, 130);
    fade(&mut g, 300);
    land_on(&mut g, JUNGLE_FROM_BONUS_1, 330);
    fade(&mut g, 500);
    land_on(&mut g, WINKY_BONUS, 530);
    fade(&mut g, 700);
    land_on(&mut g, JUNGLE_FROM_SAVE, 730);
    fade(&mut g, 900);
    let scenic = cleared(&to_overworld(&mut g, 930));

    assert_eq!(direct, Some(900));
    assert_eq!(scenic, direct, "the route must not change the verdict");
}

/// The shared animal rooms used to need naming explicitly, because they belong
/// to no level's family. They need nothing now.
#[test]
fn a_shared_animal_room_is_not_a_clear() {
    let mut g = kong();
    fade(&mut g, 200);
    assert_eq!(cleared(&land_on(&mut g, WINKY_BONUS, 230)), None);
    fade(&mut g, 800);
    assert_eq!(cleared(&to_overworld(&mut g, 830)), Some(800));
}

/// Quitting from the pause menu reaches the overworld exactly as the exit
/// does. The pause flag at the moment of the fade is the only difference, and
/// it must not be read later — by the time the map is up the menu is gone.
#[test]
fn leaving_from_the_pause_menu_is_a_quit_not_a_clear() {
    let mut g = kong();
    g.put("state_flags", TWO_KONGS | PAUSED);
    g.tick(500);
    fade(&mut g, 520);
    g.put("state_flags", TWO_KONGS);

    let ev = to_overworld(&mut g, 560);
    assert_eq!(cleared(&ev), None, "a menu exit is not a clear");
    assert_eq!(exited(&ev), Some((520, ExitReason::Quit)));
}

/// A fade that led nowhere must not be left armed, or the real exit — whenever
/// it comes — gets timed to a transition it had nothing to do with.
#[test]
fn a_fade_that_led_nowhere_is_discarded_after_the_window() {
    let mut g = kong();
    fade(&mut g, 100);
    g.idle(101, 100 + WINDOW + 1);

    // No fresh fade, so the exit is timed to itself. Still a clear: reaching
    // the map is the verdict.
    assert_eq!(cleared(&to_overworld(&mut g, 5000)), Some(5000));
}

/// The fade is an edge, so holding the value for many frames arms once — if it
/// re-armed every frame, the clear below would be timed to 100 rather than 2.
#[test]
fn a_fade_held_at_the_value_arms_once() {
    let mut g = kong();
    g.put("fade_type", FADE_OUT);
    g.idle(2, 100);
    assert_eq!(cleared(&to_overworld(&mut g, 101)), Some(2));
}

/// Reaching the map twice reports once. The second crossing is the game
/// walking its own menus, not a second clear.
#[test]
fn the_stage_only_ends_once() {
    let mut g = kong();
    fade(&mut g, 900);
    assert_eq!(cleared(&to_overworld(&mut g, 930)), Some(900));

    g.put("in_stage", 0);
    g.tick(940);
    assert_eq!(cleared(&to_overworld(&mut g, 950)), None);
}

/// With no `in_stage` address there is no completion detector at all, rather
/// than a guess from the level code. Turning one off is what `--addr
/// in_stage=off` does, and it must not turn into a wrong answer.
#[test]
fn without_in_stage_no_clear_is_ever_claimed() {
    let mut m = dkc();
    m.set_addr("in_stage", None, None).unwrap();
    let mut g = Sim::new(m, SNES_WRAM);
    g.put("lives", 5);
    g.put("level_id", JUNGLE_START);
    g.tick(1);

    fade(&mut g, 100);
    assert_eq!(cleared(&land_on(&mut g, 0x01, 130)), None);
    assert_eq!(cleared(&g.idle(131, 600)), None);
}

#[test]
fn bananas_and_letters_report_as_items() {
    let mut g = kong();
    g.put("bananas", 3);
    assert_eq!(items(&g.tick(2)), vec![("banana".to_string(), 3)]);

    // Bit 0 then bit 2: K, then N. Only the newly set bit reports.
    g.put("kong_letters", 0b0001);
    assert_eq!(items(&g.tick(3)), vec![("kong_letter:K".to_string(), 1)]);
    g.put("kong_letters", 0b0101);
    assert_eq!(items(&g.tick(4)), vec![("kong_letter:N".to_string(), 1)]);
}

#[test]
fn gaining_a_life_is_an_item_not_a_death() {
    let mut g = kong();
    g.put("lives", 6);
    let ev = g.tick(2);
    assert_eq!(items(&ev), vec![("extra_life".to_string(), 1)]);
    assert!(!ev.contains(&SessionEvent::Died { frame: 2 }));
}

/// Losing the partner Kong is bit 0 of the state flags falling — the detector
/// that spent a week switched off for want of an address.
#[test]
fn losing_the_partner_kong_is_damage() {
    let mut g = kong();
    g.put("state_flags", 0);
    assert!(g.tick(2).contains(&SessionEvent::Damaged { frame: 2 }));
    // And it is an edge: staying at one Kong is not damage every frame.
    assert!(!g.idle(3, 60).iter().any(|e| matches!(e, SessionEvent::Damaged { .. })));
}

/// The hit timer is the other half, and the one that fires when a solo Kong is
/// hit and there is no partner to lose.
#[test]
fn a_hit_timer_rising_from_zero_is_damage() {
    let mut g = kong();
    g.put("hit_timer", 40);
    assert!(g.tick(2).contains(&SessionEvent::Damaged { frame: 2 }));
    g.put("hit_timer", 39);
    assert!(!g.tick(3).contains(&SessionEvent::Damaged { frame: 3 }));
}

/// Dying with one Kong clears the two-Kong bit as well; that is one event, not
/// damage plus a death. `unless = ["death"]` in the manifest is what says so.
#[test]
fn a_death_suppresses_the_damage_it_also_causes() {
    let mut g = kong();
    g.put("lives", 4);
    g.put("state_flags", 0);
    let ev = g.tick(2);
    assert!(ev.contains(&SessionEvent::Died { frame: 2 }));
    assert!(!ev.contains(&SessionEvent::Damaged { frame: 2 }));
}

/// $1E17 is a stomp *streak*, so it rises per stomp and drops back to zero
/// when the chain breaks. The rise is the event; the reset is not.
#[test]
fn a_rising_stomp_streak_reports_enemy_kills() {
    let mut g = kong();
    g.put("enemy_kills", 1);
    assert!(g.tick(2).contains(&SessionEvent::EnemyDefeated { frame: 2, count: 1 }));
    g.put("enemy_kills", 3);
    assert!(g.tick(3).contains(&SessionEvent::EnemyDefeated { frame: 3, count: 2 }));
    g.put("enemy_kills", 0);
    assert!(!g.tick(4).iter().any(|e| matches!(e, SessionEvent::EnemyDefeated { .. })));
}

/// An override still turns a detector off without a rebuild.
#[test]
fn an_override_can_silence_a_detector() {
    let mut m = dkc();
    m.set_addr("state_flags", None, None).unwrap();
    m.set_addr("hit_timer", None, None).unwrap();
    let mut g = Sim::new(m, SNES_WRAM);
    // Written through a manifest that still knows the address, so the bytes in
    // RAM are the same; only the detector is gone.
    let full = dkc();
    let at = full.watch("state_flags").unwrap().at.unwrap();
    g.ram[at] = TWO_KONGS as u8;
    g.tick(1);
    g.ram[at] = 0;
    assert!(!g.tick(2).iter().any(|e| matches!(e, SessionEvent::Damaged { .. })));
}

/// A state load reopens the attempt: the second run reports its own clear
/// rather than being suppressed by the first one's ending.
#[test]
fn a_reset_starts_a_fresh_attempt() {
    let mut g = kong();
    fade(&mut g, 900);
    assert_eq!(cleared(&to_overworld(&mut g, 930)), Some(900));

    g.watcher.reset();
    g.put("in_stage", 0);
    g.put("level_id", JUNGLE_START);
    assert!(g.tick(1).is_empty());

    fade(&mut g, 500);
    assert_eq!(cleared(&to_overworld(&mut g, 530)), Some(500));
}

/// Mounting a buddy is a pickup; dismounting one is not. The guard in the
/// manifest is the whole of that distinction.
#[test]
fn an_animal_buddy_is_a_pickup_only_on_the_way_in() {
    let mut g = kong();
    g.put("animal", 0x09);
    assert_eq!(items(&g.tick(2)), vec![("animal_buddy".to_string(), 1)]);
    g.put("animal", 0x00);
    assert!(items(&g.tick(3)).is_empty());
}
