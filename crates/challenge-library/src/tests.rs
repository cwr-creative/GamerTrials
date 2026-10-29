use std::path::Path;

use super::*;

/// No `[runner]`, no `[watch]`, no ROM file and no entry state: the whole of
/// what a server can see of a game. A manifest this crate parses cannot be
/// loaded by `session-libretro`, and that is the point of the fixtures here.
const MINIMAL: &str = r#"
schema_version = 3
[game]
id = "t"
title = "Test"
console = "nes"
framerate = 60.0
[[rom]]
id = "base"
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
rom = "base"
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

/// The runner half is someone else's: it is neither required nor read. That
/// includes the rest of a `[[rom]]` — a path, a patch, a hash — which this
/// crate must not learn (ADR 0020).
#[test]
fn the_runner_half_is_ignored() {
    let text = format!(
        "{MINIMAL}path = \"a.nes\"\nheader = \"ines\"\nmin_bytes = 1\n\
         {CLEARABLE}entry = {{ state = \"a.state\" }}\n\
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
    let text = format!("{MINIMAL}{CLEARABLE}[[stage]]\nid = \"s\"\nrom = \"base\"\n");
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
        "{MINIMAL}\n[[stage]]\nid = \"s\"\nrom = \"base\"\n\
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

// ------------------------------------------------------- roms and stages (0020)

/// What this crate reads of a `[[rom]]`: a name, and whether it is derived.
#[test]
fn a_stage_knows_which_rom_it_runs_on() {
    let text = format!(
        "{MINIMAL}name = \"The Dump\"\n\
         [[rom]]\nid = \"hack\"\nname = \"A Hack Of It\"\nfrom = \"base\"\npatch = \"a.bps\"\n\
         {CLEARABLE}\
         [[stage]]\nid = \"h\"\nrom = \"hack\"\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert!(!lib.rom("base").unwrap().derived);
    assert_eq!(lib.rom("base").unwrap().name, "The Dump");
    assert!(lib.rom("hack").unwrap().derived);
    assert_eq!(lib.rom_of(&StageId("h".into())).unwrap().name, "A Hack Of It");
    assert_eq!(lib.stage("s").unwrap().rom, "base");
}

#[test]
fn a_stage_naming_an_undeclared_rom_is_refused() {
    let text = format!("{MINIMAL}{CLEARABLE}").replace("rom = \"base\"", "rom = \"hack\"");
    let e = Library::parse(&text).unwrap_err();
    assert!(e.contains("stage `s`") && e.contains("rom `hack`"), "{e}");
}

#[test]
fn a_stage_without_a_rom_is_refused() {
    let text = format!("{MINIMAL}{CLEARABLE}").replace("rom = \"base\"\n", "");
    assert!(Library::parse(&text).unwrap_err().contains("rom"));
}

/// Which ROM is the base, and that there is one, is the runner's rule to
/// enforce: this crate does not pretend to, so the two loaders cannot refuse
/// the same manifest with two different messages.
#[test]
fn how_many_roms_are_the_base_is_not_this_crates_business() {
    let text = format!("{MINIMAL}[[rom]]\nid = \"other\"\n{CLEARABLE}");
    assert_eq!(Library::parse(&text).unwrap().roms.len(), 2);
}

// ------------------------------------------- custom challenges and titles

const HACKED: &str = "[[rom]]\nid = \"hack\"\nname = \"A Hack Of It\"\nfrom = \"base\"\n\
                      [[stage]]\nid = \"h\"\nrom = \"hack\"\n";

/// Custom means "not the game as it shipped": generated content, or a stage
/// on a hack. The lobby's toggle filters on this and nothing else.
#[test]
fn a_challenge_on_a_hack_or_a_roll_is_custom() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}{HACKED}\
         [[challenge]]\nid = \"plain\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n\
         [[challenge]]\nid = \"hacked\"\nstage = \"h\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n\
         [[challenge]]\nid = \"rolled\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\ngenerator_version = 1\n\
         [[challenge.patch_slot]]\noffset = 0x10\nwidth = \"u8\"\nvalues = {{ pool = [1] }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert!(!lib.challenge("plain").unwrap().is_custom());
    assert!(lib.challenge("hacked").unwrap().is_custom());
    assert!(lib.challenge("rolled").unwrap().is_custom());
}

/// A hack has its own title, and `[game].title` is the base game's (ADR 0020).
#[test]
fn a_challenge_on_a_hack_is_titled_by_the_hack() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}{HACKED}\
         [[challenge]]\nid = \"plain\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n\
         [[challenge]]\nid = \"hacked\"\nstage = \"h\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert_eq!(lib.challenge("plain").unwrap().display_title(&lib), "Test");
    assert_eq!(lib.challenge("hacked").unwrap().display_title(&lib), "A Hack Of It");
}

// -------------------------------------------------------------- goal text

/// The contributor's own words win, verbatim.
#[test]
fn an_authored_goal_is_shown_as_written() {
    let lib = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { cleared = true }\n\
         goal = \"Get to the flag!\"\n",
    )
    .unwrap();
    let c = lib.challenge("c").unwrap();
    assert_eq!(c.goal.as_deref(), Some("Get to the flag!"));
    assert_eq!(c.goal_text(), "Get to the flag!");
}

/// Without one, a goal is composed from the criteria: one phrase each, in a
/// fixed order, named as the manifest names them.
#[test]
fn a_goal_is_composed_from_the_criteria_when_none_is_written() {
    let rules = "[[rule]]\nfield = \"x\"\nedge = \"increases\"\nemit = \"enemy_defeated\"\n\
                 [[rule]]\nfield = \"y\"\nedge = \"increases\"\nemit = \"item\"\nitem = \"coin\"\n\
                 [[rule]]\nfield = \"b\"\nedge = \"enters\"\nvalue = 0\n\
                 emit = \"objective\"\nobjective = \"boss-defeated\"\n";
    let challenge = |id: &str, body: &str| {
        format!("[[challenge]]\nid = \"{id}\"\nstage = \"s\"\n{body}\n")
    };
    let text = format!(
        "{MINIMAL}{CLEARABLE}{NOTES}{rules}{}{}{}{}{}{}",
        challenge("end", "kind = \"survival\"\ncomplete_when = { cleared = true }"),
        challenge("sv", "kind = \"survival\"\ncomplete_when = { survive_seconds = 45 }"),
        challenge("one", "kind = \"time_trial\"\ncomplete_when = { enemies_defeated = 1 }"),
        challenge("many", "kind = \"time_trial\"\ncomplete_when = { enemies_defeated = 5 }"),
        challenge(
            "all",
            "kind = \"time_trial\"\n[challenge.complete_when]\ncleared = true\n\
             items_collected = { coin = 10 }\ncounts = { note = 20 }\n\
             objectives = [\"boss-defeated\"]",
        ),
        challenge("boss", "kind = \"time_trial\"\ncomplete_when = { objectives = [\"boss-defeated\"] }"),
    );
    let lib = Library::parse(&text).unwrap();
    let goal = |id: &str| lib.challenge(id).unwrap().goal_text();
    assert_eq!(goal("end"), "Reach the end!");
    assert_eq!(goal("sv"), "Survive for 45 seconds!");
    assert_eq!(goal("one"), "Defeat 1 enemy!");
    assert_eq!(goal("many"), "Defeat 5 enemies!");
    assert_eq!(goal("all"), "Reach the end and Collect 10 coin and Reach 20 note and Reach boss-defeated!");
    assert_eq!(goal("boss"), "Reach boss-defeated!");
    assert!(lib.challenge("end").unwrap().goal.is_none());
}

/// Taking hits is not a goal anyone is set, so a `damaged` count composes no
/// phrase — and a challenge made of nothing else still shows something.
#[test]
fn a_damaged_count_composes_no_phrase() {
    let text = format!(
        "{MINIMAL}{CLEARABLE}{HITS}\
         [[challenge]]\nid = \"c\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ cleared = true, counts = {{ damaged = 3 }} }}\n\
         [[challenge]]\nid = \"only\"\nstage = \"s\"\nkind = \"survival\"\n\
         complete_when = {{ counts = {{ damaged = 3 }} }}\n"
    );
    let lib = Library::parse(&text).unwrap();
    assert_eq!(lib.challenge("c").unwrap().goal_text(), "Reach the end!");
    assert_eq!(lib.challenge("only").unwrap().goal_text(), "Complete the challenge!");
}

// ------------------------------------------------------------ the bound

/// Authored in seconds, stored in frames, like every other duration.
#[test]
fn a_limit_is_stored_in_frames() {
    let lib = with_challenge(
        "id = \"c\"\nkind = \"time_trial\"\ncomplete_when = { cleared = true }\n\
         limit_seconds = 120\n",
    )
    .unwrap();
    assert_eq!(lib.challenge("c").unwrap().limit_frames, Some(7200));

    let lib = with_challenge("id = \"c\"\nkind = \"time_trial\"\ncomplete_when = { cleared = true }\n")
        .unwrap();
    assert_eq!(lib.challenge("c").unwrap().limit_frames, None);
}

#[test]
fn a_limit_must_be_positive() {
    for bad in ["0", "-5", "inf", "nan"] {
        let e = with_challenge(&format!(
            "id = \"c\"\nkind = \"time_trial\"\ncomplete_when = {{ cleared = true }}\n\
             limit_seconds = {bad}\n"
        ))
        .unwrap_err();
        assert!(e.contains("limit_seconds"), "{bad}: {e}");
    }
}

/// A limit shorter than the survival the criteria ask for is a run that is
/// over before it can complete.
#[test]
fn a_limit_shorter_than_the_survival_is_refused() {
    let e = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { survive_seconds = 45 }\n\
         limit_seconds = 30\n",
    )
    .unwrap_err();
    assert!(e.contains("shorter than survive_seconds"), "{e}");

    let ok = with_challenge(
        "id = \"c\"\nkind = \"survival\"\ncomplete_when = { survive_seconds = 45 }\n\
         limit_seconds = 45\n",
    );
    assert!(ok.is_ok(), "{ok:?}");
}

// ------------------------------------------------------------- the game

#[test]
fn genre_and_year_are_optional_and_read() {
    let lib = Library::parse(MINIMAL).unwrap();
    assert_eq!(lib.game.genre, None);
    assert_eq!(lib.game.year, None);

    let text = MINIMAL.replace("console = \"nes\"", "console = \"nes\"\ngenre = \"platformer\"\nyear = 1990");
    let lib = Library::parse(&text).unwrap();
    assert_eq!(lib.game.genre.as_deref(), Some("platformer"));
    assert_eq!(lib.game.year, Some(1990));
}

// --------------------------------------------------------------- catalog

const SECOND: &str = r#"
schema_version = 3
[game]
id = "u"
title = "Other"
console = "snes"
genre = "racing"
year = 1985
framerate = 60.0
[[rom]]
id = "base"
[[rule]]
field = "lives"
edge = "increases"
emit = "stage_cleared"
[[stage]]
id = "s"
rom = "base"
[[challenge]]
id = "u-run"
stage = "s"
kind = "time_trial"
complete_when = { cleared = true }
"#;

fn first() -> String {
    format!(
        "{MINIMAL}{CLEARABLE}\
         [[challenge]]\nid = \"t-run\"\nstage = \"s\"\nkind = \"time_trial\"\n\
         complete_when = {{ cleared = true }}\n"
    )
}

/// The hash is over the texts in game-id order: the same files read in any
/// order agree, and one changed byte does not.
#[test]
fn the_catalog_hash_is_order_independent_and_text_sensitive() {
    let a = Catalog::parse_all(&[&first(), SECOND]).unwrap();
    let b = Catalog::parse_all(&[SECOND, &first()]).unwrap();
    assert_eq!(a.hash(), b.hash());
    assert_eq!(a.games().iter().map(|l| l.game.id.0.as_str()).collect::<Vec<_>>(), ["t", "u"]);

    let edited = SECOND.replace("year = 1985", "year = 1986");
    let c = Catalog::parse_all(&[&first(), &edited]).unwrap();
    assert_ne!(a.hash(), c.hash());

    // Line endings are not content: a checkout that rewrote them still agrees.
    let crlf = SECOND.replace('\n', "\r\n");
    let d = Catalog::parse_all(&[&first(), &crlf]).unwrap();
    assert_eq!(a.hash(), d.hash());
}

#[test]
fn the_catalog_finds_a_challenge_by_id_alone() {
    let cat = Catalog::parse_all(&[&first(), SECOND]).unwrap();
    let (lib, c) = cat.challenge(&ChallengeId("u-run".into())).unwrap();
    assert_eq!(lib.game.id.0, "u");
    assert_eq!(c.id.0, "u-run");
    assert!(cat.challenge(&ChallengeId("nope".into())).is_none());
    assert_eq!(cat.challenges().count(), 2);
    assert_eq!(cat.game("t").unwrap().game.title, "Test");
}

/// A dealt challenge is named by its id alone, so one id cannot mean two
/// challenges — and two manifests cannot claim one game.
#[test]
fn the_catalog_refuses_an_id_used_twice() {
    let clash = SECOND.replace("id = \"u-run\"", "id = \"t-run\"");
    let e = Catalog::parse_all(&[&first(), &clash]).unwrap_err();
    assert!(e.contains("challenge `t-run`"), "{e}");

    let e = Catalog::parse_all(&[&first(), &first()]).unwrap_err();
    assert!(e.contains("game `t`"), "{e}");
}

#[test]
fn a_manifest_that_does_not_parse_is_named_by_position() {
    let e = Catalog::parse_all(&[&first(), "schema_version = 9\n"]).unwrap_err();
    assert!(e.starts_with("manifest #2"), "{e}");
}

#[test]
fn games_are_grouped_by_era_genre_or_console() {
    let cat = Catalog::parse_all(&[&first(), SECOND]).unwrap();
    let names = |g: Vec<(String, Vec<&Library>)>| -> Vec<(String, Vec<String>)> {
        g.into_iter()
            .map(|(k, v)| (k, v.iter().map(|l| l.game.id.0.clone()).collect()))
            .collect()
    };
    assert_eq!(
        names(cat.grouping(Grouping::Era)),
        [("1980s".to_string(), vec!["u".to_string()]), ("unknown".into(), vec!["t".into()])]
    );
    assert_eq!(
        names(cat.grouping(Grouping::Genre)),
        [("racing".to_string(), vec!["u".to_string()]), ("uncategorised".into(), vec!["t".into()])]
    );
    assert_eq!(
        names(cat.grouping(Grouping::Console)),
        [("nes".to_string(), vec!["t".to_string()]), ("snes".into(), vec!["u".into()])]
    );
}

// --------------------------------------------------------- shipped manifests

fn shipped() -> Vec<(String, String)> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../manifests");
    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        out.push((path.display().to_string(), std::fs::read_to_string(&path).unwrap()));
    }
    out
}

/// Every shipped manifest yields a library on its own, with no emulator, no
/// ROM and no runner — which is what a server has.
#[test]
fn every_shipped_manifest_parses_without_its_runner() {
    let mut seen = 0;
    for (path, text) in shipped() {
        let lib = Library::parse(&text).unwrap_or_else(|e| panic!("{path}: {e}"));
        assert!(!lib.challenges.is_empty(), "{path} offers no challenges");
        assert_eq!(lib.specs().len(), lib.challenges.len());
        seen += 1;
    }
    assert!(seen >= 3, "expected the three slice manifests, found {seen}");
}

fn shipped_catalog() -> Catalog {
    let texts = shipped();
    Catalog::parse_all(&texts.iter().map(|(_, t)| t.as_str()).collect::<Vec<_>>()).unwrap()
}

/// The exact goal every shipped challenge shows, authored or composed. A
/// change here is a change to what a player reads in the intro.
#[test]
fn every_shipped_challenge_has_the_goal_it_should() {
    let cat = shipped_catalog();
    let goal = |id: &str| cat.challenge(&ChallengeId(id.into())).unwrap().1.goal_text();
    // SMB3
    for id in ["1-1-time-trial", "1-1-survival", "1-2-time-trial", "1-2-survival",
               "1-3-time-trial", "1-3-survival", "1-4-survival"] {
        assert_eq!(goal(id), "Reach the end!", "{id}");
    }
    assert_eq!(goal("1-1-stomp-rush"), "Stomp five enemies!");
    assert_eq!(goal("sun-storm-survival"), "Reach the end under the Angry Suns!");
    assert_eq!(goal("sun-storm-time-trial"), "Reach the end under the Angry Suns!");
    assert_eq!(goal("bro-roulette-time-trial"), "Kill all four enemies, by any means!");
    assert_eq!(goal("bro-roulette-survival"), "Kill all four enemies, by any means!");
    // DKC
    for id in ["jungle-hijinx-time-trial", "jungle-hijinx-survival", "reptile-rumble-time-trial",
               "reptile-rumble-survival", "ropey-rampage-time-trial", "ropey-rampage-survival",
               "barrel-cannon-canyon-time-trial", "barrel-cannon-canyon-survival",
               "coral-capers-time-trial", "coral-capers-survival", "mine-cart-carnage-survival"] {
        assert_eq!(goal(id), "Reach the end!", "{id}");
    }
    assert_eq!(goal("mine-cart-carnage-45s"), "Stay in the cart for 45 seconds!");
    assert_eq!(goal("jungle-hijinx-kong"), "Collect K, O, N and G, then reach the end!");
    assert_eq!(goal("reptile-roulette"), "Reach the end through a room full of reptiles!");
    // Doom
    assert_eq!(goal("e1m1-time-trial"), "Reach the end!");
    assert_eq!(goal("e1m1-survival"), "Reach the end!");
}

/// E1M1 carries its own bound; everything else takes the lobby's default.
#[test]
fn only_doom_e1m1_declares_a_limit() {
    let cat = shipped_catalog();
    for (lib, c) in cat.challenges() {
        let expected = if lib.game.id.0 == "doom" { Some(8410) } else { None };
        assert_eq!(c.limit_frames, expected, "{}", c.id.0);
    }
}

#[test]
fn the_custom_challenges_are_the_rolls_and_the_hack() {
    let cat = shipped_catalog();
    let custom: Vec<&str> = cat
        .challenges()
        .filter(|(_, c)| c.is_custom())
        .map(|(_, c)| c.id.0.as_str())
        .collect();
    assert_eq!(
        custom,
        ["reptile-roulette", "sun-storm-survival", "sun-storm-time-trial",
         "bro-roulette-time-trial", "bro-roulette-survival"]
    );
    assert!(!cat.challenge(&ChallengeId("1-1-time-trial".into())).unwrap().1.is_custom());
}

/// A hack is shown under its own name, and the base game under the game's.
#[test]
fn the_hack_stages_are_titled_by_the_hack() {
    let cat = shipped_catalog();
    let title = |id: &str| {
        let (lib, c) = cat.challenge(&ChallengeId(id.into())).unwrap();
        c.display_title(lib)
    };
    assert_eq!(title("sun-storm-survival"), "SMB3: Gamer Trials");
    assert_eq!(title("bro-roulette-time-trial"), "SMB3: Gamer Trials");
    assert_eq!(title("1-1-time-trial"), "Super Mario Bros. 3");
    assert_eq!(title("reptile-roulette"), "Donkey Kong Country");
    assert_eq!(title("e1m1-survival"), "DOOM (shareware)");
}

#[test]
fn the_shipped_games_group_as_the_lobby_will_show_them() {
    let cat = shipped_catalog();
    let ids = |g: Vec<(String, Vec<&Library>)>| -> Vec<(String, Vec<String>)> {
        g.into_iter()
            .map(|(k, v)| (k, v.iter().map(|l| l.game.id.0.clone()).collect()))
            .collect()
    };
    assert_eq!(
        ids(cat.grouping(Grouping::Era)),
        [("1990s".to_string(), vec!["dkc".to_string(), "doom".into(), "smb3".into()])]
    );
    assert_eq!(
        ids(cat.grouping(Grouping::Genre)),
        [("fps".to_string(), vec!["doom".to_string()]),
         ("platformer".into(), vec!["dkc".into(), "smb3".into()])]
    );
    assert_eq!(
        ids(cat.grouping(Grouping::Console)),
        [("dos".to_string(), vec!["doom".to_string()]),
         ("nes".into(), vec!["smb3".into()]),
         ("snes".into(), vec!["dkc".into()])]
    );
}
