//! The GamerTrials client (ADR 0027): a native game window with a transparent
//! Tauri overlay over it, and a menu and lobby in an ordinary window.
//!
//! `cargo run -p app-client` opens the menu. Host game opens a lobby on the
//! server set in Settings › Profile, or a lobby of one on an in-process server
//! when none is (ADR 0009); Join game takes an invite (ADR 0030); Start plays
//! a match. `--host <name>` skips the menu (`--server <addr>` picks where),
//! `--join <invite>` joins instead (as `--name <name>`), `--start` presses Start as soon as the lobby allows, and
//! `--mode`, `--format kind:n`, `--games a,b` and `--limit <s>` set the
//! match up first: a match from the
//! command line, for playtesting and for the spike. `--spike` adds
//! the window self-test from the first build to the first round: it moves and
//! resizes the game window, injects a keypress with the overlay focused, and
//! logs whether the overlay followed and the game heard the key.
//!
//! Settings is the player's bindings and video, set once and overridable per
//! core and per game (ADR 0028), saved to `settings.toml` in the app's config
//! directory. They apply from the next round dealt; `--scale` overrides the
//! window scale they give.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod capture;
mod flow;
mod game;
mod keys;
mod library;
mod link;
mod local;
mod overlay;
mod settings;
mod stream;
mod video;

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use protocol::{ChallengeRef, LobbySettings};
use tauri::{Manager, State};

use flow::{Msg, UiCmd};
use settings::{Input, Settings};

struct Args {
    manifests: PathBuf,
    /// Over the settings' window scale, when given.
    scale: Option<u32>,
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
    /// With `--host`: the server to host on, over the profile's.
    server: Option<String>,
    /// Join this invite instead of showing the menu.
    join: Option<String>,
    /// With `--join`: the display name, over the profile's.
    name: Option<String>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        manifests: PathBuf::from("manifests"),
        scale: None,
        spike: false,
        host: None,
        start: false,
        mode: None,
        format: None,
        games: None,
        limit: None,
        server: None,
        join: None,
        name: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--manifests" => args.manifests = PathBuf::from(it.next().ok_or("--manifests wants a directory")?),
            "--host" => args.host = Some(it.next().ok_or("--host wants a player name")?),
            "--start" => args.start = true,
            "--server" => args.server = Some(it.next().ok_or("--server wants host[:port]")?),
            "--join" => args.join = Some(it.next().ok_or("--join wants an invite, host[:port]/CODE")?),
            "--name" => args.name = Some(it.next().ok_or("--name wants a player name")?),
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
                let v: u32 = it
                    .next()
                    .ok_or("--scale wants 1 to 6")?
                    .parse()
                    .map_err(|_| "--scale wants a number")?;
                args.scale = Some(v.clamp(1, 6));
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
    settings_meta: serde_json::Value,
    settings: Arc<Mutex<Settings>>,
    /// Where the settings live; known once Tauri has said where config goes.
    settings_path: Mutex<Option<PathBuf>>,
    /// Why the settings file could not be read, for the page to say once.
    settings_warning: Mutex<Option<String>>,
    capture: capture::Capture,
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
fn join(state: State<'_, Shared>, name: String, invite: String) {
    state.send(UiCmd::Join { name, invite });
}

#[tauri::command]
fn leave(state: State<'_, Shared>) {
    state.send(UiCmd::Leave);
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

#[tauri::command]
fn give_up(state: State<'_, Shared>) {
    state.send(UiCmd::GiveUp);
}

/// From the spectate sheet: watch this player instead (ADR 0032).
#[tauri::command]
fn watch(state: State<'_, Shared>, player: String) {
    state.send(UiCmd::Watch { player });
}

#[tauri::command]
fn get_settings(state: State<'_, Shared>) -> serde_json::Value {
    serde_json::json!({
        "settings": *state.settings.lock().unwrap(),
        "warning": state.settings_warning.lock().unwrap().take(),
        "path": state.settings_path.lock().unwrap().as_ref().map(|p| p.display().to_string()),
    })
}

/// Validate, store and save. Returns what was kept, which the page shows, so
/// a value brought into range is never displayed as what the player typed.
#[tauri::command]
fn put_settings(state: State<'_, Shared>, settings: Settings) -> Result<Settings, String> {
    let mut s = settings;
    s.sanitize();
    *state.settings.lock().unwrap() = s.clone();
    if let Some(path) = state.settings_path.lock().unwrap().as_ref() {
        s.save(path)?;
    }
    Ok(s)
}

#[tauri::command]
fn settings_meta(state: State<'_, Shared>) -> serde_json::Value {
    state.settings_meta.clone()
}

/// The next input the player presses, read the way play reads it (`capture`).
#[tauri::command]
async fn capture_input(
    app: tauri::AppHandle,
    state: State<'_, Shared>,
    chord: bool,
) -> Result<Option<Vec<Input>>, String> {
    let deadzone = state.settings.lock().unwrap().input.deadzone;
    let main = main_hwnd(&app);
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || capture.next(main, deadzone, chord))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn cancel_capture(state: State<'_, Shared>) {
    state.capture.cancel();
}

/// Everything held now, for the page to light up the pad it draws.
#[tauri::command]
async fn input_preview(app: tauri::AppHandle, state: State<'_, Shared>) -> Result<Vec<Input>, String> {
    let deadzone = state.settings.lock().unwrap().input.deadzone;
    let main = main_hwnd(&app);
    let capture = state.capture.clone();
    tauri::async_runtime::spawn_blocking(move || capture.held(main, deadzone))
        .await
        .map_err(|e| e.to_string())
}

fn main_hwnd(app: &tauri::AppHandle) -> Option<isize> {
    app.get_webview_window("main").and_then(|w| overlay::handle(&w))
}

fn main() {
    // Every window of ours is an X11 window on Linux, XWayland under a
    // Wayland desktop: Wayland lets no program place its window over
    // another's, and the overlay is exactly that (ADR 0035). A player's own
    // setting wins. WebKitGTK's DMA-BUF renderer is left alone: turning it
    // off stops the transparent overlay clearing what it drew before.
    #[cfg(target_os = "linux")]
    if std::env::var_os("GDK_BACKEND").is_none() {
        std::env::set_var("GDK_BACKEND", "x11");
    }
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
    let settings = Arc::new(Mutex::new(Settings::default()));
    let shared = Shared {
        ui: Mutex::new(None),
        catalog: library.ui_catalog(),
        settings_meta: library.settings_meta(),
        settings: settings.clone(),
        settings_path: Mutex::new(None),
        settings_warning: Mutex::new(None),
        capture: capture::Capture::spawn(),
    };
    let scale = args.scale;
    let spike = args.spike;
    let auto = flow::Auto {
        host: args.host,
        server: args.server,
        join: args.join,
        name: args.name,
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
            join,
            leave,
            set_settings,
            set_enabled,
            start,
            begin,
            play_again,
            back_to_lobby,
            end_match,
            resume,
            give_up,
            watch,
            get_settings,
            put_settings,
            settings_meta,
            capture_input,
            cancel_capture,
            input_preview
        ])
        .setup(move |app| {
            match app.path().app_config_dir() {
                Ok(dir) => {
                    let path = dir.join("settings.toml");
                    let (loaded, warning) = Settings::load(&path);
                    if let Some(w) = &warning {
                        eprintln!("[settings] {w}");
                    }
                    println!("[settings] {}", path.display());
                    *settings.lock().unwrap() = loaded;
                    let state = app.state::<Shared>();
                    *state.settings_warning.lock().unwrap() = warning;
                    *state.settings_path.lock().unwrap() = Some(path);
                }
                Err(e) => eprintln!("[settings] no config directory ({e}); settings will not be saved"),
            }
            let ui = flow::Flow::spawn(app.handle().clone(), library, settings.clone(), scale, spike, auto);
            *app.state::<Shared>().ui.lock().unwrap() = Some(ui);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
