//! The GamerTrials client (ADR 0027): a native game window with a transparent
//! Tauri overlay over it, and a menu and lobby in an ordinary window.
//!
//! `cargo run -p app-client` opens the menu. Host game opens a lobby of one
//! on an in-process server (ADR 0009); Start plays a match. `--host <name>`
//! skips the menu, `--start` presses Start as soon as the lobby allows, and
//! `--mode`, `--format kind:n`, `--games a,b` and `--limit <s>` set the
//! match up first: a match from the
//! command line, for playtesting and for the spike. `--spike` adds
//! the window self-test from the first build to the first round: it moves and
//! resizes the game window, injects a keypress with the overlay focused, and
//! logs whether the overlay followed and the game heard the key.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod flow;
mod game;
mod keys;
mod library;
mod local;
mod overlay;

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use protocol::{ChallengeRef, LobbySettings};
use tauri::{Manager, State};

use flow::{Msg, UiCmd};

struct Args {
    manifests: PathBuf,
    scale: usize,
    spike: bool,
    /// Host a lobby at once, as this player, instead of showing the menu.
    host: Option<String>,
    /// With `--host`: start the match as soon as the lobby allows.
    start: bool,
    mode: Option<protocol::Mode>,
    format: Option<protocol::Format>,
    /// With `--host`: enable only these games' challenges.
    games: Option<Vec<String>>,
    /// With `--host`: the default limit per challenge, in seconds.
    limit: Option<f64>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        manifests: PathBuf::from("manifests"),
        scale: 2,
        spike: false,
        host: None,
        start: false,
        mode: None,
        format: None,
        games: None,
        limit: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--manifests" => args.manifests = PathBuf::from(it.next().ok_or("--manifests wants a directory")?),
            "--host" => args.host = Some(it.next().ok_or("--host wants a player name")?),
            "--start" => args.start = true,
            "--games" => {
                let v = it.next().ok_or("--games wants a comma-separated list of game ids")?;
                args.games = Some(v.split(',').map(|g| g.trim().to_string()).collect());
            }
            "--limit" => {
                let v = it.next().ok_or("--limit wants seconds")?;
                args.limit = Some(v.parse().map_err(|_| format!("--limit wants seconds, got {v}"))?);
            }
            "--mode" => {
                args.mode = Some(match it.next().as_deref() {
                    Some("time_trial") => protocol::Mode::TimeTrial,
                    Some("survival") => protocol::Mode::Survival,
                    Some("combo") => protocol::Mode::Combo,
                    other => return Err(format!("--mode wants time_trial, survival or combo, got {other:?}")),
                })
            }
            "--format" => {
                let v = it.next().ok_or("--format wants kind:n, e.g. challenges:3")?;
                let (kind, n) = v.split_once(':').ok_or("--format wants kind:n, e.g. challenges:3")?;
                let n: u32 = n.parse().map_err(|_| format!("--format: {n} is not a number"))?;
                args.format = Some(match kind {
                    "challenges" => protocol::Format::Challenges(n),
                    "strikes" => protocol::Format::Strikes(n),
                    "wins" => protocol::Format::Wins(n),
                    "stock" => protocol::Format::Stock(n),
                    other => return Err(format!("--format: unknown kind {other}")),
                })
            }
            "--scale" => {
                args.scale = it
                    .next()
                    .ok_or("--scale wants 1, 2 or 4")?
                    .parse()
                    .map_err(|_| "--scale wants a number")?
            }
            "--spike" => args.spike = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

/// What the pages' commands reach.
struct Shared {
    ui: Mutex<Option<Sender<Msg>>>,
    catalog: serde_json::Value,
}

impl Shared {
    fn send(&self, cmd: UiCmd) {
        if let Some(tx) = self.ui.lock().unwrap().as_ref() {
            let _ = tx.send(Msg::Ui(cmd));
        }
    }
}

#[tauri::command]
fn catalog(state: State<'_, Shared>) -> serde_json::Value {
    state.catalog.clone()
}

#[tauri::command]
fn host(state: State<'_, Shared>, name: String) {
    state.send(UiCmd::Host { name });
}

#[tauri::command]
fn set_settings(state: State<'_, Shared>, settings: LobbySettings) {
    state.send(UiCmd::Settings(settings));
}

#[tauri::command]
fn set_enabled(state: State<'_, Shared>, challenges: Vec<ChallengeRef>) {
    state.send(UiCmd::Enable(challenges));
}

#[tauri::command]
fn start(state: State<'_, Shared>) {
    state.send(UiCmd::Start);
}

#[tauri::command]
fn begin(state: State<'_, Shared>) {
    state.send(UiCmd::Begin);
}

#[tauri::command]
fn play_again(state: State<'_, Shared>) {
    state.send(UiCmd::PlayAgain);
}

#[tauri::command]
fn back_to_lobby(state: State<'_, Shared>) {
    state.send(UiCmd::BackToLobby);
}

#[tauri::command]
fn end_match(state: State<'_, Shared>) {
    state.send(UiCmd::EndMatch);
}

#[tauri::command]
fn resume(state: State<'_, Shared>) {
    state.send(UiCmd::Resume);
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let library = match library::Library::load(&args.manifests) {
        Ok(l) => Arc::new(l),
        Err(e) => {
            eprintln!("cannot load the library: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "[library] {} games, hash {}",
        library.catalog.games().len(),
        library.hash_hex()
    );
    let shared = Shared { ui: Mutex::new(None), catalog: library.ui_catalog() };
    let scale = args.scale;
    let spike = args.spike;
    let auto = flow::Auto {
        host: args.host,
        start: args.start,
        mode: args.mode,
        format: args.format,
        games: args.games,
        limit: args.limit,
    };

    tauri::Builder::default()
        .manage(shared)
        .invoke_handler(tauri::generate_handler![
            catalog,
            host,
            set_settings,
            set_enabled,
            start,
            begin,
            play_again,
            back_to_lobby,
            end_match,
            resume
        ])
        .setup(move |app| {
            let ui = flow::Flow::spawn(app.handle().clone(), library, scale, spike, auto);
            *app.state::<Shared>().ui.lock().unwrap() = Some(ui);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
