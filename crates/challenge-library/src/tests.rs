use std::path::Path;

use super::*;

/// No `[runner]`, no `[watch]`, no `[[rom]]` and no entry state: the whole of
/// what a server can see of a game. A manifest this crate parses cannot be
/// loaded by `session-libretro`, and that is the point of the fixtures here.
const MINIMAL: &str = r#"
schema_version = 3
[game]
id = "t"
title = "Test"
console = "nes"
framerate = 60.0
"#;

/// A stage, and a rule that can clear it. `field` and `edge` are the runner's
/// and are present only because a rule cannot be written without them.
const CLEARABLE: &str = r#"
[[rule]]
field = "lives"
edge = "increases"
emit = "stage_cleared"
[[stage]]
id = "s"
"#;

fn with_challenge(body: &str) -> Result<Library, String> {
    Library::parse(&format!("{MINIMAL}{CLEARABLE}\n[[challenge]]\nstage = \"s\"\n{body}"))
}

#[test]
fn a_future_schema_version_is_refused_at_the_version() {
    let text = MINIMAL.replace("schema_version = 3", "schema_version = 4");
    assert!(Library::parse(&text).unwrap_err().contains("schema_version"));
}

/// The version is read before the rest, so a manifest whose other tables have
/// changed shape is still refused for the reason that matters.
#[test]
fn the_version_is_checked_before_the_shape() {
    let text = "schema_version = 1\n[game]\nid = \"t\"\nendurance = true\n";
    assert!(Library::parse(text).unwrap_err().contains("schema_version"));
}

/// The runner half is someone else's: it is neither required nor read.
#[test]
fn the_runner_half_is_ignored() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}rom = \"base\"\nentry = {{ state = \"a.state\" }}\n\
         [runner]\nkind = \"anything\"\n[watch.lives]\nat = 0x0736\n\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert_eq!(lib.challenges.len(), 1);
}

/// What this crate owns, it is strict about: a mistyped criterion is a
/// challenge that cannot be won, silently.
#[test]
fn an_unknown_key_in_a_challenge_is_an_error() {
    let e = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleard = true }\n",
    );
    assert!(e.is_err());
    let e = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleared = true }\nnotes = \"x\"\n",
    );
    assert!(e.is_err());
}

#[test]
fn an_implausible_framerate_is_refused() {
    let text = MINIMAL.replace("framerate = 60.0", "framerate = 0.0");
    assert!(Library::parse(&text).unwrap_err().contains("framerate"));
}

#[test]
fn durations_authored_in_seconds_are_stored_as_frames() {
    let lib = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { survive_seconds = 30 }\n",
    )
    .unwrap();
    assert_eq!(lib.challenge("c").unwrap().complete_when.survive_frames, Some(1800));
}

/// "Survive 30 seconds fastest" is not something anyone can win, so the
/// criterion belongs to survival alone.
#[test]
fn survive_seconds_on_a_time_trial_is_refused() {
    let e = with_challenge(
        "id = \"c\"\nkind = \"time_trial\"\ncomplete_when = { survive_seconds = 30 }\n",
    )
    .unwrap_err();
    assert!(e.contains("survival criterion"), "{e}");
}

#[test]
fn a_challenge_defaults_its_name_to_the_stages() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}name = \"The Stage\"\n\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert_eq!(lib.challenge("c").unwrap().name, "The Stage");
    assert_eq!(lib.challenge("c").unwrap().kind, ChallengeKind::Survival);
}

/// Several challenges on one stage is the normal case, not an edge case.
#[test]
fn one_stage_can_carry_several_challenges() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}\
         [[challenge]]\nid = \"tt\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ cleared = true }}\n\
         [[challenge]]\nid = \"sv\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    let s = StageId("s".into());
    let on: Vec<&str> = lib.challenges_on(&s).map(|c| c.id.0.as_str()).collect();
    assert_eq!(on, vec!["tt", "sv"]);
}

#[test]
fn a_challenge_naming_an_undeclared_stage_is_refused() {
    let e = with_challenge("id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleared = true }\n");
    assert!(e.is_ok(), "the control case must pass: {e:?}");

    let text = format!(
        "{MINIMAL}{CLEARABLE}\
         [[challenge]]\nid = \"c\"\nstage = \"nope\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n"
    );
    assert!(Library::parse(&text).unwrap_err().contains("is not declared"));
}

#[test]
fn two_challenges_cannot_share_an_id() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ cleared = true }}\n"
    );
    assert!(Library::parse(&text).unwrap_err().contains("declared twice"));
}

#[test]
fn two_stages_cannot_share_an_id() {
    let text = format!("{MINIMAL}{CLEARABLE}[[stage]]\nid = \"s\"\n");
    assert!(Library::parse(&text).unwrap_err().contains("declared twice"));
}

#[test]
fn a_challenge_with_no_criteria_is_refused() {
    let e = with_challenge("id = \"c\"\nkind = \"survival\"\ncomplete_when = { }\n").unwrap_err();
    assert!(e.contains("no criteria"), "{e}");
}

/// The point of validating criteria at load: a criterion naming a fact this
/// manifest never emits is a challenge nobody can ever complete, and it looks
/// exactly like a wrong address.
#[test]
fn a_criterion_over_a_fact_nothing_emits_is_refused() {
    let text = format!(
        "{MINIMAL}\n[[stage]]\nid = \"s\"\n\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n"
    );
    let e = Library::parse(&text).unwrap_err();
    assert!(e.contains("no [[rule]] emits `stage_cleared`"), "{e}");

    let e = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { enemies_defeated = 3 }\n",
    )
    .unwrap_err();
    assert!(e.contains("`enemy_defeated`"), "{e}");

    let e = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { objectives = [\"boss\"] }\n",
    )
    .unwrap_err();
    assert!(e.contains("none at all"), "{e}");
}

/// A per-bit item rule names one item per bit, so `kong_letter:K` is the name a
/// criterion has to use — and the bare `kong_letter` is not one.
#[test]
fn per_bit_items_are_known_by_their_labelled_names() {
    let rule = "[[rule]]\nfield = \"lives\"\nedge = \"bits_set\"\n\
                bit_labels = [\"K\", \"O\"]\nemit = \"item\"\nitem = \"letter\"\n";
    let ok = format!(
        "{MINIMAL}{CLEARABLE}{rule}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         [challenge.complete_when]\nitems_collected = {{ \"letter:K\" = 1 }}\n"
    );
    let lib = Library::parse(&ok).unwrap();
    assert_eq!(lib.challenge("c").unwrap().complete_when.counts["letter:K"], 1);

    let bad = ok.replace("\"letter:K\"", "\"letter\"");
    let e = Library::parse(&bad).unwrap_err();
    assert!(e.contains("letter:K, letter:O"), "{e}");
}

#[test]
fn an_objective_rule_makes_an_objective_criterion_legal() {
    let rule = "[[rule]]\nfield = \"lives\"\nedge = \"enters\"\nvalue = 0\n\
                emit = \"objective\"\nobjective = \"boss-defeated\"\n";
    let text = format!(
        "{MINIMAL}{CLEARABLE}{rule}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ objectives = [\"boss-defeated\"] }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert_eq!(lib.challenge("c").unwrap().complete_when.objectives, vec!["boss-defeated"]);
}

#[test]
fn an_objective_emit_needs_a_name() {
    let text =
        format!("{MINIMAL}\n[[rule]]\nfield = \"lives\"\nedge = \"decreases\"\nemit = \"objective\"\n");
    assert!(Library::parse(&text).unwrap_err().contains("needs an `objective` name"));
}

#[test]
fn a_reason_on_anything_but_an_exit_is_refused() {
    let e = Emit::parse("died", Some("death"), None, None, None, "rule `x`").unwrap_err();
    assert!(e.contains("only `stage_exited` takes a `reason`"), "{e}");
    assert_eq!(
        Emit::parse("stage_exited", Some("death"), None, None, None, "rule `x`").unwrap(),
        Emit::StageExited(ExitReason::Death)
    );
}

// -------------------------------------------------- counters and tallies (0025)

/// A note-placing counter, as a Mario Paint manifest would declare one.
const NOTES: &str = "[[rule]]\nfield = \"notes\"\nedge = \"increases\"\n\
                     emit = \"count\"\ncounter = \"note\"\n";

/// A contributor's own counter is a criterion like any item.
#[test]
fn a_named_counter_can_complete_a_challenge() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}{NOTES}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ counts = {{ note = 20 }} }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert_eq!(lib.challenge("c").unwrap().complete_when.counts["note"], 20);
}

#[test]
fn a_count_emit_needs_a_counter_name() {
    let text = format!(
        "{MINIMAL}\n[[rule]]\nfield = \"notes\"\nedge = \"increases\"\nemit = \"count\"\n"
    );
    assert!(Library::parse(&text).unwrap_err().contains("needs a `counter` name"));
}

/// The older spellings feed the same tallies, so they are still accepted and
/// read into `counts`.
#[test]
fn enemies_defeated_and_items_collected_are_spellings_of_counts() {
    let rules = "[[rule]]\nfield = \"x\"\nedge = \"increases\"\nemit = \"enemy_defeated\"\n\
                 [[rule]]\nfield = \"y\"\nedge = \"increases\"\nemit = \"item\"\nitem = \"coin\"\n";
    let text = format!(
        "{MINIMAL}{CLEARABLE}{rules}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         [challenge.complete_when]\nenemies_defeated = 3\nitems_collected = {{ coin = 10 }}\n"
    );
    let c = Library::parse(&text).unwrap().challenge("c").unwrap().clone();
    assert_eq!(c.complete_when.counts[ENEMY_DEFEATED], 3);
    assert_eq!(c.complete_when.counts["coin"], 10);

    let twice = text.replace("items_collected = { coin = 10 }", "counts = { enemy_defeated = 4 }");
    assert!(Library::parse(&twice).unwrap_err().contains("counted twice"));
}

/// One name, one meaning: an item and a counter cannot share a name, and
/// neither can claim a built-in one.
#[test]
fn a_tally_name_means_one_kind_of_fact() {
    let clash = "[[rule]]\nfield = \"y\"\nedge = \"increases\"\nemit = \"item\"\nitem = \"note\"\n";
    let e = Library::parse(&format!("{MINIMAL}{NOTES}{clash}")).unwrap_err();
    assert!(e.contains("same name"), "{e}");

    let built_in =
        "[[rule]]\nfield = \"y\"\nedge = \"increases\"\nemit = \"count\"\ncounter = \"damaged\"\n";
    let rule = "[[rule]]\nfield = \"hp\"\nedge = \"decreases\"\nemit = \"damaged\"\n";
    let e = Library::parse(&format!("{MINIMAL}{rule}{built_in}")).unwrap_err();
    assert!(e.contains("same name"), "{e}");
}

#[test]
fn a_count_of_a_tally_nothing_advances_is_refused() {
    let e = with_challenge("id = \"c\"\nkind = \"survival\"\ncomplete_when = { counts = { note = 1 } }\n")
        .unwrap_err();
    assert!(e.contains("no [[rule]] advances a tally named `note`"), "{e}");
}

// ---------------------------------------------------- ends_attempt_on (0025)

const HITS: &str = "[[rule]]\nfield = \"hp\"\nedge = \"decreases\"\nemit = \"damaged\"\n";

/// A no-hit run: reach the end, and a hit costs what a death costs.
#[test]
fn ends_attempt_on_names_facts_that_cost_a_life() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}{HITS}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ cleared = true }}\nends_attempt_on = [\"damaged\"]\n"
    );
    let c = Library::parse(&text).unwrap().challenge("c").unwrap().clone();
    assert!(c.ends_attempt_on.contains(DAMAGED));
    assert!(c.spec(&GameId("t".into())).ends_attempt_on.contains(DAMAGED));
}

/// The same check criteria get: a fact nothing emits would make a no-hit run
/// one that every run passes, silently.
#[test]
fn ends_attempt_on_must_name_something_emitted() {
    let e = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleared = true }\n\
         ends_attempt_on = [\"damaged\"]\n",
    )
    .unwrap_err();
    assert!(e.contains("no [[rule]] emits it"), "{e}");
}

#[test]
fn ends_attempt_on_refuses_what_already_ends_an_attempt() {
    let e = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleared = true }\n\
         ends_attempt_on = [\"died\"]\n",
    )
    .unwrap_err();
    assert!(e.contains("already ends an attempt"), "{e}");
}

/// A fact that ends the attempt on its frame can never complete the run on it.
#[test]
fn a_fact_cannot_both_complete_and_end_an_attempt() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}{HITS}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ counts = {{ damaged = 1 }} }}\nends_attempt_on = [\"damaged\"]\n"
    );
    assert!(Library::parse(&text).unwrap_err().contains("can never complete"));
}

// ------------------------------------------------------ generated challenges

/// What a dealer needs from a generated challenge: that it is one, and which
/// version of its table rolls it. The table's contents are not read here.
#[test]
fn a_generated_challenge_is_known_to_be_one() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}\
         [[challenge]]\nid = \"reroll\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ cleared = true }}\ngenerator_version = 2\n\
         [[challenge.patch_slot]]\noffset = 0xC6BD\nwidth = \"u8\"\n\
         values = {{ pool = [0x72, 0x72, 0x81] }}\n"
    );
    let c = Library::parse(&text).unwrap().challenge("reroll").unwrap().clone();
    assert!(c.is_generated());
    assert_eq!(c.generator_version, Some(2));
}

#[test]
fn a_vanilla_challenge_is_not_generated() {
    let lib = with_challenge("id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleared = true }\n")
        .unwrap();
    assert!(!lib.challenge("c").unwrap().is_generated());
}

/// The version is what a correction to a table bumps, so a table without one
/// is a generator whose output cannot be told apart from its predecessor's
/// (ADR 0013).
#[test]
fn patch_slots_without_a_generator_version_are_refused() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}\
         [[challenge]]\nid = \"reroll\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ cleared = true }}\n\
         [[challenge.patch_slot]]\noffset = 0x10\nwidth = \"u8\"\n\
         values = {{ pool = [1] }}\n"
    );
    let e = Library::parse(&text).unwrap_err();
    assert!(e.contains("generator_version"), "{e}");
}

#[test]
fn a_generator_version_with_nothing_to_roll_is_refused() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}\
         [[challenge]]\nid = \"reroll\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ cleared = true }}\ngenerator_version = 1\n"
    );
    let e = Library::parse(&text).unwrap_err();
    assert!(e.contains("nothing is generated"), "{e}");
}

// --------------------------------------------------------- shipped manifests

/// Every shipped manifest yields a library on its own, with no emulator, no
/// ROM and no runner — which is what a server has.
#[test]
fn every_shipped_manifest_parses_without_its_runner() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mut seen = 0;
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lib = Library::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert!(!lib.challenges.is_empty(), "{} offers no challenges", path.display());
        assert_eq!(lib.specs().len(), lib.challenges.len());
        seen += 1;
    }
    assert!(seen >= 3, "expected the three slice manifests, found {seen}");
}
