//! The client's coordinator: one thread that hears from the server, the game
//! thread and the pages, and does what each means for the others.
//!
//! The server deals and judges (ADR 0009). The game thread runs and reports.
//! The overlay page animates and, when an animation ends, says so. This
//! thread is what connects them, and it holds no rule about a match: it
//! forwards facts and carries out verdicts.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread;

use protocol::{ChallengeRef, LobbySettings, LobbyState, ToClient, ToServer};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::game::{self, Command, Load, Report};
use crate::library::Library;
use crate::overlay::Overlay;
use crate::{local, overlay};

/// Everything the coordinator can hear.
pub enum Msg {
    Server(ToClient),
    Game(Report),
    Ui(UiCmd),
}

/// What the pages ask for.
pub enum UiCmd {
    Host { name: String },
    Settings(LobbySettings),
    Enable(Vec<ChallengeRef>),
    Start,
    /// The overlay's countdown ended.
    Begin,
    PlayAgain,
    BackToLobby,
    EndMatch,
    /// From the pause menu.
    Resume,
}

/// What the command line asked for in place of the menu.
#[derive(Default)]
pub struct Auto {
    pub host: Option<String>,
    pub start: bool,
    pub mode: Option<protocol::Mode>,
    pub format: Option<protocol::Format>,
    pub games: Option<Vec<String>>,
    pub limit: Option<f64>,
}

/// The round being played, as the overlay needs to describe it.
struct Current {
    round: u64,
}

pub struct Flow {
    app: AppHandle,
    library: Arc<Library>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    scale: usize,
    spike: bool,
    to_server: Option<Sender<ToServer>>,
    player: Option<String>,
    lobby: Option<LobbyState>,
    game: Option<Sender<Command>>,
    game_hwnd: Option<isize>,
    overlay: Option<Overlay>,
    /// A round dealt before the game window was up.
    waiting_load: Option<Load>,
    current: Option<Current>,
    /// Which challenge each dealt round was, for the cards.
    dealt: BTreeMap<u64, ChallengeRef>,
    spiked: bool,
    auto: Auto,
    /// `--start` has been acted on.
    auto_started: bool,
}

impl Flow {
    pub fn spawn(
        app: AppHandle,
        library: Arc<Library>,
        scale: usize,
        spike: bool,
        auto: Auto,
    ) -> Sender<Msg> {
        let (tx, rx) = mpsc::channel();
        let ui = tx.clone();
        if let Some(name) = auto.host.clone() {
            let _ = tx.send(Msg::Ui(UiCmd::Host { name }));
        }
        thread::Builder::new()
            .name("flow".into())
            .spawn(move || {
                let mut flow = Flow {
                    app,
                    library,
                    rx,
                    tx,
                    scale,
                    spike,
                    to_server: None,
                    player: None,
                    lobby: None,
                    game: None,
                    game_hwnd: None,
                    overlay: None,
                    waiting_load: None,
                    current: None,
                    dealt: BTreeMap::new(),
                    spiked: false,
                    auto,
                    auto_started: false,
                };
                flow.run();
            })
            .expect("spawn the flow thread");
        ui
    }

    fn run(&mut self) {
        while let Ok(msg) = self.rx.recv() {
            match msg {
                Msg::Ui(cmd) => self.ui(cmd),
                Msg::Server(m) => self.server(m),
                Msg::Game(r) => self.game(r),
            }
        }
    }

    // --------------------------------------------------------------- pages

    fn ui(&mut self, cmd: UiCmd) {
        match cmd {
            UiCmd::Host { name } => {
                if self.to_server.is_none() {
                    let seed = local::fresh_seed();
                    println!("[match] local server seeded {seed}");
                    let tx = local::spawn(self.library.catalog.clone(), seed, self.tx.clone());
                    self.to_server = Some(tx);
                }
                let name = if name.trim().is_empty() { "Player".to_string() } else { name };
                self.send(ToServer::Hello { name, library_hash: self.library.hash_hex() });
                self.send(ToServer::Files(self.library.file_report()));
            }
            UiCmd::Settings(s) => self.send(ToServer::Settings(s)),
            UiCmd::Enable(c) => self.send(ToServer::Enable { challenges: c }),
            UiCmd::Start => self.send(ToServer::Start),
            UiCmd::Begin => {
                if let Some(g) = &self.game {
                    let _ = g.send(Command::Begin);
                }
                self.passthrough(true);
            }
            UiCmd::PlayAgain => {
                self.passthrough(true);
                self.send(ToServer::PlayAgain);
            }
            UiCmd::BackToLobby => {
                self.send(ToServer::BackToLobby);
                self.leave_table();
            }
            UiCmd::EndMatch => {
                self.send(ToServer::EndMatch);
            }
            UiCmd::Resume => {
                if let Some(g) = &self.game {
                    let _ = g.send(Command::Resume);
                }
                self.passthrough(true);
                self.emit(json!({ "kind": "resume" }));
            }
        }
    }

    // -------------------------------------------------------------- server

    fn server(&mut self, m: ToClient) {
        match m {
            ToClient::Welcome { player, lobby } => {
                println!("[lobby] joined as {player}");
                self.player = Some(player);
                let mut settings = lobby.settings.clone();
                let mut changed = false;
                if let Some(m) = self.auto.mode {
                    settings.mode = m;
                    changed = true;
                }
                if let Some(f) = self.auto.format {
                    settings.format = f;
                    changed = true;
                }
                if let Some(l) = self.auto.limit {
                    settings.default_limit_seconds = l;
                    changed = true;
                }
                let only: Option<Vec<ChallengeRef>> = self.auto.games.as_ref().map(|games| {
                    lobby.enabled.iter().filter(|r| games.contains(&r.game)).cloned().collect()
                });
                self.show_lobby(lobby);
                if changed {
                    self.send(ToServer::Settings(settings));
                }
                if let Some(refs) = only {
                    self.send(ToServer::Enable { challenges: refs });
                }
            }
            ToClient::Refused { reason } => {
                eprintln!("[server] refused: {reason}");
                let _ = self.app.emit_to("main", "notice", &json!({ "text": reason }));
            }
            ToClient::Lobby(lobby) => self.show_lobby(lobby),
            ToClient::Dealt { round, challenge, kind, seed, limit_frames, players } => {
                println!(
                    "[match] round {round}: {} / {} ({kind:?}), seed {seed}, limit {limit_frames} frames, players {players:?}",
                    challenge.game, challenge.challenge
                );
                self.dealt.insert(round, challenge.clone());
                let Some(lib) = self.library.catalog.game(&challenge.game) else {
                    eprintln!("[match] dealt a game this client does not hold: {}", challenge.game);
                    return;
                };
                let Some(c) = lib.challenge(&challenge.challenge) else {
                    eprintln!("[match] dealt a challenge this client does not hold: {}", challenge.challenge);
                    return;
                };
                let load = Load {
                    round,
                    manifest: self.library.paths[&challenge.game].clone(),
                    stage: c.stage.0.clone(),
                    challenge: c.id.0.clone(),
                    seed,
                    limit: limit_frames,
                };
                self.current = Some(Current { round });
                self.emit(json!({ "kind": "loading", "round": round }));
                match &self.game {
                    Some(g) => {
                        let _ = g.send(Command::Unload);
                        let _ = g.send(Command::Load(load));
                    }
                    None => {
                        self.waiting_load = Some(load);
                        self.open_table();
                    }
                }
            }
            ToClient::Go { round } => {
                if self.current.as_ref().map_or(true, |c| c.round != round) {
                    eprintln!("[match] go for round {round}, which is not the one being prepared");
                }
                self.emit(json!({ "kind": "go", "round": round }));
                if self.spike && !self.spiked {
                    self.spiked = true;
                    if let (Some(ov), Some(hwnd)) = (self.overlay.clone(), self.game_hwnd) {
                        let app = self.app.clone();
                        thread::spawn(move || {
                            overlay::spike(&ov, hwnd);
                            app.exit(0);
                        });
                    }
                }
            }
            ToClient::PlayerFinished { round, player, outcome } => {
                println!("[match] round {round}: {player} {outcome:?}");
            }
            ToClient::RoundResult { round, entries, eliminated, standings } => {
                println!("[match] round {round} result:");
                for e in &entries {
                    println!("         {:>2}. {} {:?}", e.place, e.player, e.outcome.ending);
                }
                let challenge = self.dealt.get(&round).cloned();
                let title = challenge.as_ref().map(|c| self.describe(c)).unwrap_or_default();
                let mode = self.lobby.as_ref().map(|l| l.settings.mode);
                let format = self.lobby.as_ref().map(|l| l.settings.format);
                self.emit(json!({
                    "kind": "round", "round": round, "title": title,
                    "entries": entries, "eliminated": eliminated, "standings": standings,
                    "mode": mode, "format": format, "me": self.player,
                }));
            }
            ToClient::MatchOver { standings } => {
                println!("[match] over:");
                for s in &standings {
                    println!("         #{} {}  completions {}  points {}  frames {}", s.rank, s.player, s.completions, s.points, s.frames);
                }
                if let Some(g) = &self.game {
                    let _ = g.send(Command::Unload);
                }
                self.current = None;
                let mode = self.lobby.as_ref().map(|l| l.settings.mode);
                let format = self.lobby.as_ref().map(|l| l.settings.format);
                self.emit(json!({
                    "kind": "score", "standings": standings, "mode": mode, "format": format,
                    "me": self.player,
                }));
                self.passthrough(false);
            }
            ToClient::Error { message } => {
                eprintln!("[match] error: {message}");
                self.emit(json!({ "kind": "error", "text": message }));
            }
        }
    }

    // ---------------------------------------------------------------- game

    fn game(&mut self, r: Report) {
        match r {
            Report::Window { hwnd } => {
                self.game_hwnd = Some(hwnd);
                match Overlay::create(&self.app, hwnd) {
                    Ok(ov) => {
                        if let Some(g) = &self.game {
                            let _ = g.send(Command::Overlay(ov.hwnd()));
                        }
                        let _ = ov.set_passthrough(true);
                        let tracker = ov.clone();
                        thread::spawn(move || tracker.track());
                        self.overlay = Some(ov);
                    }
                    Err(e) => eprintln!("[overlay] cannot create: {e}"),
                }
                if let Some(w) = self.app.get_webview_window("main") {
                    let _ = w.hide();
                }
                if let (Some(g), Some(load)) = (&self.game, self.waiting_load.take()) {
                    let _ = g.send(Command::Load(load));
                }
            }
            Report::Loaded { round, artifact } => {
                println!("[match] round {round} prepared: {artifact}");
                self.send(ToServer::Prepared { round, artifact });
                if let Some(c) = self.dealt.get(&round).cloned() {
                    let mut intro = self.intro(&c);
                    intro["round"] = json!(round);
                    self.emit(intro);
                }
            }
            Report::LoadFailed { round, error } => {
                eprintln!("[match] round {round} cannot be prepared: {error}");
                self.emit(json!({ "kind": "error", "text": format!("Cannot prepare this round: {error}") }));
                // The protocol has no word for this yet; the match ends.
                self.send(ToServer::EndMatch);
            }
            Report::Frame { round, frame, events } => {
                let died = events.iter().any(|e| matches!(e, session_api::SessionEvent::Died { .. }));
                let wire = events.iter().map(Into::into).collect();
                self.send(ToServer::Frame { round, frame, events: wire });
                if died {
                    self.emit(json!({ "kind": "death", "frame": frame }));
                }
            }
            Report::Reentered { round, attempt, banked } => {
                println!("[match] round {round}: attempt {attempt}, {banked} frames on the clock");
                self.emit(json!({ "kind": "hud", "text": format!("attempt {attempt}") }));
            }
            Report::Finished { round, result, forfeited_at } => {
                println!("[match] round {round}: provisional {result}");
                if let Some(frame) = forfeited_at {
                    self.send(ToServer::Forfeit { round, frame });
                }
                let outcome: protocol::RunOutcome = (&result).into();
                let completed = outcome.completed();
                self.send(ToServer::Finished { round, outcome });
                self.emit(json!({
                    "kind": if completed { "garland" } else { "skull" },
                    "frame": result.frames,
                    "text": result.ending.to_string(),
                }));
            }
            Report::Paused => {
                self.emit(json!({ "kind": "pause" }));
                self.passthrough(false);
            }
            Report::Closed => {
                println!("[client] game window closed");
                self.game = None;
                self.game_hwnd = None;
                self.overlay = None;
                if self.current.is_some() {
                    self.send(ToServer::EndMatch);
                    self.current = None;
                }
                self.send(ToServer::BackToLobby);
                if let Some(w) = self.app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        }
    }

    // ------------------------------------------------------------- helpers

    fn send(&self, msg: ToServer) {
        if let Some(tx) = &self.to_server {
            let _ = tx.send(msg);
        }
    }

    fn emit(&self, payload: serde_json::Value) {
        if let Some(ov) = &self.overlay {
            ov.emit(payload);
        }
    }

    fn passthrough(&self, on: bool) {
        if let Some(ov) = &self.overlay {
            let _ = ov.set_passthrough(on);
            if !on {
                ov.focus();
            }
        }
    }

    fn show_lobby(&mut self, lobby: LobbyState) {
        let _ = self.app.emit_to("main", "lobby", &lobby);
        let settings_applied = self.auto.mode.map_or(true, |m| m == lobby.settings.mode)
            && self.auto.format.map_or(true, |f| f == lobby.settings.format)
            && self.auto.limit.map_or(true, |l| l == lobby.settings.default_limit_seconds)
            && self.auto.games.as_ref().map_or(true, |g| lobby.enabled.iter().all(|r| g.contains(&r.game)));
        if self.auto.start && !self.auto_started && lobby.can_start && settings_applied {
            self.auto_started = true;
            println!("[lobby] --start: starting the match");
            self.send(ToServer::Start);
        }
        self.lobby = Some(lobby);
    }

    /// Opens the game window; the overlay follows once it reports in.
    fn open_table(&mut self) {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (rep_tx, rep_rx) = mpsc::channel();
        let scale = self.scale;
        thread::Builder::new()
            .name("game".into())
            .spawn(move || {
                if let Err(e) = game::run(scale, cmd_rx, rep_tx) {
                    eprintln!("[game] {e}");
                }
            })
            .expect("spawn the game thread");
        let to_flow = self.tx.clone();
        thread::spawn(move || {
            for r in rep_rx {
                if to_flow.send(Msg::Game(r)).is_err() {
                    break;
                }
            }
            let _ = to_flow.send(Msg::Game(Report::Closed));
        });
        self.game = Some(cmd_tx);
    }

    fn leave_table(&mut self) {
        if let Some(g) = self.game.take() {
            let _ = g.send(Command::Quit);
        }
        self.current = None;
        // `Report::Closed` follows and shows the lobby again.
    }

    fn describe(&self, c: &ChallengeRef) -> String {
        let Some(lib) = self.library.catalog.game(&c.game) else { return c.challenge.clone() };
        let Some(ch) = lib.challenge(&c.challenge) else { return c.challenge.clone() };
        format!("{} \u{2014} {}", ch.display_title(lib), ch.name)
    }

    fn intro(&self, c: &ChallengeRef) -> serde_json::Value {
        let Some(lib) = self.library.catalog.game(&c.game) else {
            return json!({ "kind": "intro", "title": c.game, "console": "", "goal": "" });
        };
        let Some(ch) = lib.challenge(&c.challenge) else {
            return json!({ "kind": "intro", "title": lib.game.title, "console": lib.game.console, "goal": "" });
        };
        json!({
            "kind": "intro",
            "title": ch.display_title(lib),
            "name": ch.name,
            "console": lib.game.console.to_uppercase(),
            "goal": ch.goal_text(),
            "challenge_kind": match ch.kind {
                session_api::ChallengeKind::TimeTrial => "Time trial",
                session_api::ChallengeKind::Survival => "Survival",
            },
            "custom": ch.is_custom(),
        })
    }
}
