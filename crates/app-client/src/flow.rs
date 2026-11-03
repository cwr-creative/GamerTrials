//! The client's coordinator: one thread that hears from the server, the game
//! thread and the pages, and does what each means for the others.
//!
//! The server deals and judges (ADR 0009). The game thread runs and reports.
//! The overlay page animates and, when an animation ends, says so. This
//! thread is what connects them, and it holds no rule about a match: it
//! forwards facts and carries out verdicts.
//!
//! The server is in-process for solo play and a `gamertrials-server` for a
//! lobby of several ([`Link`]); the flow is the same over either. A guest
//! differs from the host only in what the server lets them do, and in what
//! leaving means: a host's closed window ends the match and keeps the lobby,
//! a guest's leaves the lobby.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use std::net::{SocketAddr, ToSocketAddrs};

use protocol::{ChallengeRef, FileReport, Kind, LobbySettings, LobbyState, ToClient, ToServer};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager};

use crate::game::{self, BoutLoad, Command, Load, Report, Watching};
use crate::library::Library;
use crate::overlay::Overlay;
use crate::link::{self, Link};
use crate::overlay;
use crate::settings::Settings;

/// Everything the coordinator can hear.
pub enum Msg {
    Server(ToClient),
    /// The link to a networked server failed. Tagged, so a link already
    /// replaced cannot end the next one.
    Disconnected { generation: u64, reason: String },
    Game(Report),
    Ui(UiCmd),
}

/// What the pages ask for.
pub enum UiCmd {
    Host { name: String },
    /// Join a lobby on a networked server by invite (ADR 0030).
    Join { name: String, invite: String },
    /// Leave the lobby, from the lobby page or a guest's pause menu.
    Leave,
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
    /// Give up the run, from the pause menu.
    GiveUp,
    /// The lobby's periodic file check is due.
    Recheck,
    /// Watch another player, from the spectate sheet (ADR 0032).
    Watch { player: String },
    /// Back a seated player in the bout, from the backing sheet (ADR 0036).
    Back { player: String },
}

/// How often the lobby checks this machine's files again.
const RECHECK: Duration = Duration::from_secs(3);

/// What the command line asked for in place of the menu.
#[derive(Default)]
pub struct Auto {
    pub host: Option<String>,
    /// `--server`: host on this server rather than the profile's.
    pub server: Option<String>,
    /// `--join`: an invite to join instead of hosting, as `--name` (or the
    /// profile's name).
    pub join: Option<String>,
    pub name: Option<String>,
    pub start: bool,
    pub mode: Option<protocol::Mode>,
    pub format: Option<protocol::Format>,
    pub games: Option<Vec<String>>,
    pub limit: Option<f64>,
    /// `--h2h`: allow head-to-head challenges in the lobby (ADR 0036).
    pub head_to_head: bool,
    /// `--players <n>`: with `--start`, wait until this many are in.
    pub players: Option<usize>,
    /// `--script <seed>`: press random buttons instead of reading the pad,
    /// for a bout played by two clients with nobody at either keyboard.
    pub script: Option<u64>,
    /// `--give-up-after <s>`: give every run up that many seconds of play in,
    /// so a scripted client becomes a spectator (ADR 0037).
    pub give_up_after: Option<f64>,
}

/// The round being played, as the overlay needs to describe it.
struct Current {
    round: u64,
    /// Who is still playing it, for the waiting view.
    playing: Vec<String>,
}

/// A round this player is watching rather than playing (ADR 0032): their
/// run is over, or they are out of the match.
struct Spectate {
    round: u64,
    /// Who can still be watched: still playing, and not this player.
    players: Vec<String>,
    watching: Option<String>,
}

pub struct Flow {
    app: AppHandle,
    library: Arc<Library>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    /// The player's settings, shared with the settings page. Read when a
    /// round is dealt, so a change applies from the next round.
    settings: Arc<Mutex<Settings>>,
    /// `--scale`, over the settings' window scale.
    scale: Option<u32>,
    spike: bool,
    link: Option<Link>,
    /// Counts links, so a failure report from an old one is recognised.
    generation: u64,
    player: Option<String>,
    lobby: Option<LobbyState>,
    game: Option<Sender<Command>>,
    game_hwnd: Option<isize>,
    overlay: Option<Overlay>,
    /// A round dealt before the game window was up.
    waiting_load: Option<Load>,
    current: Option<Current>,
    spectate: Option<Spectate>,
    /// Which challenge each dealt round was, for the cards.
    dealt: BTreeMap<u64, ChallengeRef>,
    /// Each dealt round's seed and limit, so a watcher can prepare the same
    /// round it replays (ADR 0037).
    terms: BTreeMap<u64, (u64, u64)>,
    /// Who plays each dealt round, in slot order (ADR 0036).
    slots: BTreeMap<u64, Vec<String>>,
    /// Where a bout's probes go, as the server said at hello.
    rendezvous: Option<String>,
    /// The bout this player is backing someone in, if any.
    backing_round: Option<u64>,
    /// The round this player has claimed a verdict for. A relayed bout's
    /// judge sees only its own machine's facts, so the other slot's
    /// completion reaches it as the server's verdict, which it then claims.
    claimed: Option<u64>,
    spiked: bool,
    auto: Auto,
    /// `--start` has been acted on.
    auto_started: bool,
    /// The file report the server last heard.
    files: Option<FileReport>,
    /// The recheck timer is running.
    ticking: bool,
    /// The flow closed the game window itself; its `Closed` is not the
    /// player leaving.
    closing: bool,
}

impl Flow {
    pub fn spawn(
        app: AppHandle,
        library: Arc<Library>,
        settings: Arc<Mutex<Settings>>,
        scale: Option<u32>,
        spike: bool,
        auto: Auto,
    ) -> Sender<Msg> {
        let (tx, rx) = mpsc::channel();
        let ui = tx.clone();
        if let Some(invite) = auto.join.clone() {
            let name = auto.name.clone().unwrap_or_default();
            let _ = tx.send(Msg::Ui(UiCmd::Join { name, invite }));
        } else if let Some(name) = auto.host.clone() {
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
                    settings,
                    scale,
                    spike,
                    link: None,
                    generation: 0,
                    player: None,
                    lobby: None,
                    game: None,
                    game_hwnd: None,
                    overlay: None,
                    waiting_load: None,
                    current: None,
                    spectate: None,
                    dealt: BTreeMap::new(),
                    terms: BTreeMap::new(),
                    slots: BTreeMap::new(),
                    rendezvous: None,
                    backing_round: None,
                    claimed: None,
                    spiked: false,
                    auto,
                    auto_started: false,
                    files: None,
                    ticking: false,
                    closing: false,
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
                Msg::Disconnected { generation, reason } => {
                    if generation == self.generation && self.link.is_some() {
                        eprintln!("[link] {reason}");
                        let text = format!("Lost the server: {reason}");
                        self.emit(json!({ "kind": "error", "text": text }));
                        self.leave();
                        self.notice(&text);
                    }
                }
                Msg::Game(r) => self.game(r),
            }
        }
    }

    // --------------------------------------------------------------- pages

    fn ui(&mut self, cmd: UiCmd) {
        match cmd {
            UiCmd::Host { name } => {
                if self.link.is_some() {
                    return;
                }
                let server = self
                    .auto
                    .server
                    .clone()
                    .or_else(|| Some(self.settings.lock().unwrap().server.clone()))
                    .filter(|s| !s.trim().is_empty());
                let link = match server {
                    None => Link::local(self.library.catalog.clone(), self.tx.clone()),
                    Some(server) => match self.connect(&server) {
                        Ok(l) => l,
                        Err(e) => return self.notice(&e),
                    },
                };
                self.hello(link, name, None);
            }
            UiCmd::Join { name, invite } => {
                if self.link.is_some() {
                    return;
                }
                let profile = self.settings.lock().unwrap().server.clone();
                let (server, code) = match link::parse_invite(&invite, Some(&profile)) {
                    Ok(v) => v,
                    Err(e) => return self.notice(&e),
                };
                match self.connect(&server.url()) {
                    Ok(l) => self.hello(l, name, Some(code)),
                    Err(e) => self.notice(&e),
                }
            }
            UiCmd::Leave => {
                self.leave();
                self.notice("");
            }
            UiCmd::Recheck => {
                // Between matches only: in one, the table is up and the check
                // would hash dumps under a running game for nothing.
                if self.lobby.is_some() && self.game.is_none() {
                    self.send_files();
                }
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
            UiCmd::GiveUp => {
                if let Some(g) = &self.game {
                    let _ = g.send(Command::Forfeit);
                }
                self.passthrough(true);
            }
            UiCmd::Watch { player } => self.watch(Some(player)),
            UiCmd::Back { player } => {
                if let Some(round) = self.backing_round {
                    self.send(ToServer::Back { round, player });
                }
            }
        }
    }

    // -------------------------------------------------------------- server

    fn server(&mut self, m: ToClient) {
        match m {
            ToClient::Welcome { player, lobby, rendezvous } => {
                println!("[lobby] joined {} as {player}", lobby.code);
                self.player = Some(player.clone());
                self.rendezvous = rendezvous;
                let invite = self.link.as_ref().and_then(|l| l.server.as_ref()).map(|s| link::invite(s, &lobby.code));
                let _ = self.app.emit_to("main", "session", &json!({ "me": player, "invite": invite }));
                if !lobby.players.first().is_some_and(|p| p.id == player) {
                    // A guest: the lobby's settings are the host's to set.
                    self.show_lobby(lobby);
                    return;
                }
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
                if self.auto.head_to_head {
                    settings.allow_head_to_head = true;
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
                if self.player.is_none() {
                    // Not let in: a lobby that is not there, or another library.
                    self.leave();
                }
                self.notice(&reason);
            }
            ToClient::Lobby(lobby) => {
                if !lobby.in_match && self.game.is_some() {
                    // The host went back to the lobby; so does everyone.
                    self.leave_table();
                }
                self.show_lobby(lobby);
            }
            ToClient::Dealt { round, challenge, kind, seed, limit_frames, players, backing_seconds } => {
                println!(
                    "[match] round {round}: {} / {} ({kind:?}), seed {seed}, limit {limit_frames} frames, players {players:?}",
                    challenge.game, challenge.challenge
                );
                self.dealt.insert(round, challenge.clone());
                self.terms.insert(round, (seed, limit_frames));
                self.slots.insert(round, players.clone());
                self.backing_round = None;
                let me = self.player.clone().unwrap_or_default();
                let bout = kind == Kind::HeadToHead;
                self.stop_spectating();
                if !players.contains(&me) {
                    // Out of the match, or not seated in this bout: nothing
                    // to play, so watch (ADR 0032) — and in a bout, back a
                    // slot first (ADR 0036).
                    if let Some(g) = &self.game {
                        let _ = g.send(Command::Unload);
                    }
                    self.current = None;
                    if bout {
                        self.backing_round = Some(round);
                        self.emit(json!({
                            "kind": "backing", "round": round, "players": players,
                            "seconds": backing_seconds.unwrap_or(15),
                        }));
                    } else {
                        self.emit(json!({ "kind": "eliminated", "round": round, "players": players }));
                    }
                    self.start_spectating(round, players);
                    return;
                }
                self.current = Some(Current { round, playing: players.clone() });
                let bout = bout.then(|| {
                    let slot = players.iter().position(|p| *p == me).unwrap_or(0) as u8;
                    let (input_delay, port) = {
                        let s = self.settings.lock().unwrap();
                        (s.bout_input_delay, s.bout_port)
                    };
                    BoutLoad { slot, slots: players.len() as u8, rendezvous: self.rendezvous_addr(), input_delay, port }
                });
                let mut load = match self.load_for(round) {
                    Ok(l) => l,
                    Err(e) => {
                        self.unprepared(round, e);
                        return;
                    }
                };
                load.bout = bout;
                load.script = self.auto.script;
                load.give_up_after = self.auto.give_up_after;
                self.emit(json!({ "kind": "loading", "round": round }));
                match &self.game {
                    Some(g) => {
                        let _ = g.send(Command::Unload);
                        let _ = g.send(Command::Load(Box::new(load)));
                    }
                    None => {
                        self.waiting_load = Some(load);
                        self.open_table();
                    }
                }
            }
            ToClient::Go { round } => {
                if self.backing_round == Some(round) {
                    self.emit(json!({ "kind": "bout_go", "round": round }));
                } else if self.current.as_ref().map_or(true, |c| c.round != round) {
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
            ToClient::Voided { round, player, reason } => {
                println!("[match] round {round} void: {player} cannot prepare it: {reason}");
                if let Some(g) = &self.game {
                    let _ = g.send(Command::Unload);
                }
                if self.is_current(round) {
                    self.current = None;
                }
                self.stop_spectating();
                let who = if self.player.as_deref() == Some(player.as_str()) { "you".to_string() } else { player };
                self.emit(json!({
                    "kind": "voided", "round": round,
                    "text": format!("Round skipped: {who} cannot prepare it ({reason})"),
                }));
            }
            ToClient::PlayerFinished { round, player, outcome } => {
                println!("[match] round {round}: {player} {outcome:?}");
                let me = self.player.clone().unwrap_or_default();
                if player == me && self.is_current(round) && self.claimed != Some(round) {
                    // The server ruled on a run this client could not judge
                    // for itself: the other machine's facts never came here,
                    // or the link was lost. Its verdict is the claim.
                    self.claimed = Some(round);
                    let completed = outcome.completed();
                    self.emit(json!({
                        "kind": if completed { "garland" } else { "skull" },
                        "frame": outcome.frames,
                        "text": format!("{:?}", outcome.ending).to_lowercase(),
                    }));
                    if outcome.ending != protocol::Ending::Forfeited {
                        self.send(ToServer::Finished { round, outcome: outcome.clone() });
                    }
                }
                // A seated opponent who gave up hands this player the win
                // (ADR 0036); the judge on the game thread is told.
                if outcome.ending == protocol::Ending::Forfeited && self.is_current(round) {
                    if let Some(slot) = self.slots.get(&round).and_then(|s| s.iter().position(|p| *p == player)) {
                        if let Some(g) = &self.game {
                            let _ = g.send(Command::PeerForfeited { slot: slot as u8, frame: outcome.frames });
                        }
                    }
                }
                if let Some(c) = self.current.as_mut().filter(|c| c.round == round) {
                    c.playing.retain(|p| p != &player);
                    let playing = c.playing.clone();
                    self.emit(json!({ "kind": "playing", "round": round, "players": playing }));
                    // This player's bout is over by the server's word: watch
                    // whoever is still going, if anyone is.
                    if player == me && self.spectate.is_none() && !playing.is_empty() {
                        self.start_spectating(round, playing);
                    }
                }
                if let Some(s) = self.spectate.as_mut().filter(|s| s.round == round) {
                    s.players.retain(|p| p != &player);
                    if s.watching.as_deref() == Some(player.as_str()) {
                        // Whoever is still going, or nobody.
                        let next = s.players.first().cloned();
                        self.watch(next);
                    } else {
                        self.emit_spectate();
                    }
                }
            }
            ToClient::Watched { round, watchers } => {
                println!("[match] round {round}: {watchers} watching");
                if let Some(g) = &self.game {
                    let _ = g.send(Command::Stream(watchers));
                }
            }
            ToClient::Video { round, player, frame, key, width, height, aspect, data } => {
                if self.spectate.as_ref().is_some_and(|s| s.round == round && s.watching.as_deref() == Some(&*player)) {
                    self.emit(json!({
                        "kind": "video", "player": player, "frame": frame, "key": key,
                        "width": width, "height": height, "aspect": aspect, "data": protocol::to_base64(&data.0),
                    }));
                }
            }
            ToClient::Audio { round, player, frame, rate, channels, data } => {
                if self.spectate.as_ref().is_some_and(|s| s.round == round && s.watching.as_deref() == Some(&*player)) {
                    self.emit(json!({
                        "kind": "audio", "player": player, "frame": frame,
                        "rate": rate, "channels": channels, "data": protocol::to_base64(&data.0),
                    }));
                }
            }
            ToClient::Inputs { round, player, attempt, from, count, frames, checks, artifact } => {
                if self.spectate.as_ref().is_some_and(|s| s.round == round && s.watching.as_deref() == Some(&*player)) {
                    if let Some(g) = &self.game {
                        let checks = checks.into_iter().map(|c| (c.frame, c.picture)).collect();
                        let _ = g.send(Command::Inputs { player, attempt, from, count, frames: frames.0, checks, artifact });
                    }
                }
            }
            ToClient::Progress { round, player, frame, attempt, clock, events } => {
                if self.spectate.as_ref().is_some_and(|s| s.round == round && s.watching.as_deref() == Some(&*player)) {
                    // Frames on the wire; the page shows them at the game's own rate.
                    let fps = self
                        .dealt
                        .get(&round)
                        .and_then(|c| self.library.catalog.game(&c.game))
                        .map_or(60.0, |g| g.game.framerate.0);
                    let said: Vec<String> = events.iter().map(|e| said(&e.to_session())).collect();
                    self.emit(json!({
                        "kind": "progress", "player": player, "frame": frame, "attempt": attempt,
                        "clock": clock, "fps": fps, "events": said,
                    }));
                }
            }
            ToClient::Sides { round, backing } => {
                println!("[match] round {round} sides: {backing:?}");
                self.emit(json!({ "kind": "sides", "round": round, "backing": backing, "me": self.player }));
            }
            ToClient::Peers { round, peers } => {
                println!("[match] round {round} peers: {peers:?}");
                if !self.is_current(round) {
                    return;
                }
                let parsed: Vec<(u8, Vec<SocketAddr>)> = peers
                    .iter()
                    .map(|p| {
                        let addrs = p
                            .candidates
                            .iter()
                            .filter_map(|c| c.to_socket_addrs().ok().and_then(|mut a| a.next()))
                            .collect();
                        (p.slot, addrs)
                    })
                    .collect();
                if let Some(g) = &self.game {
                    let _ = g.send(Command::Peers(parsed));
                }
            }
            ToClient::RoundResult { round, entries, eliminated, standings } => {
                println!("[match] round {round} result:");
                for e in &entries {
                    println!("         {:>2}. {} {:?}", e.place, e.player, e.outcome.ending);
                }
                self.stop_spectating();
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
                self.stop_spectating();
                let mode = self.lobby.as_ref().map(|l| l.settings.mode);
                let format = self.lobby.as_ref().map(|l| l.settings.format);
                self.emit(json!({
                    "kind": "score", "standings": standings, "mode": mode, "format": format,
                    "me": self.player, "host": self.is_host(),
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
                    let _ = g.send(Command::Load(Box::new(load)));
                }
            }
            Report::Loaded { round, artifact } => {
                if !self.is_current(round) {
                    // Voided while it loaded.
                    return;
                }
                println!("[match] round {round} prepared: {artifact}");
                self.send(ToServer::Prepared { round, artifact });
                if let Some(c) = self.dealt.get(&round).cloned() {
                    let mut intro = self.intro(&c);
                    intro["round"] = json!(round);
                    self.emit(intro);
                }
            }
            Report::LoadFailed { round, error } => {
                if self.is_current(round) {
                    self.unprepared(round, error);
                }
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
            Report::Clock { round, frames, fps, stopped } => {
                if self.is_current(round) {
                    self.emit(json!({ "kind": "clock", "frames": frames, "fps": fps, "stopped": stopped }));
                }
            }
            Report::Finished { round, result, forfeited_at } => {
                println!("[match] round {round}: provisional {result}");
                self.claimed = Some(round);
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
                // Done, and others are not: watch them (ADR 0032). In a bout
                // the server settles later than this judge does, and a watch
                // before it has is refused; `PlayerFinished` starts it then.
                let bout = self.dealt_kind(round) == Some(Kind::HeadToHead);
                let me = self.player.clone().unwrap_or_default();
                let others: Vec<String> = self
                    .current
                    .as_ref()
                    .filter(|c| c.round == round)
                    .map(|c| c.playing.iter().filter(|p| **p != me).cloned().collect())
                    .unwrap_or_default();
                if !others.is_empty() && !bout {
                    self.start_spectating(round, others);
                }
            }
            Report::Video { round, frame, key, width, height, aspect, data } => {
                self.send(ToServer::Video { round, frame, key, width, height, aspect, data: data.into() });
            }
            Report::Audio { round, frame, rate, channels, data } => {
                self.send(ToServer::Audio { round, frame, rate, channels, data: data.into() });
            }
            Report::Replay { round, player, state } => {
                if self.spectate.as_ref().is_some_and(|s| s.round == round && s.watching.as_deref() == Some(&*player)) {
                    println!("[watch] replaying {player}: {state:?}");
                    let (kind, text) = state.describe();
                    self.emit(json!({ "kind": "replay", "player": player, "state": kind, "text": text }));
                }
            }
            Report::Inputs { round, attempt, from, count, frames, checks } => {
                let checks = checks.into_iter().map(|(frame, picture)| protocol::Check { frame, picture }).collect();
                self.send(ToServer::Inputs { round, attempt, from, count, frames: frames.into(), checks });
            }
            Report::Endpoint { round, token, local } => {
                if self.is_current(round) {
                    self.send(ToServer::Endpoint { round, token, local });
                }
            }
            Report::PeerLost { round, frame } => {
                if self.is_current(round) {
                    self.send(ToServer::PeerLost { round, frame });
                    self.emit(json!({ "kind": "hud", "text": "lost the other player's machine; the server rules" }));
                }
            }
            Report::Paused => {
                let actions = self.settings.lock().unwrap().input.actions.clone();
                self.emit(json!({
                    "kind": "pause", "pause": actions.pause, "give_up": actions.give_up, "host": self.is_host(),
                }));
                self.passthrough(false);
            }
            Report::Closed => {
                println!("[client] game window closed");
                self.game = None;
                self.game_hwnd = None;
                self.overlay = None;
                let asked = std::mem::take(&mut self.closing);
                if self.is_host() {
                    if self.current.is_some() {
                        self.send(ToServer::EndMatch);
                    }
                    self.send(ToServer::BackToLobby);
                } else if !asked && self.link.is_some() {
                    // A guest who closes the table leaves; the server ends
                    // the match for everyone, as for any player who leaves.
                    self.leave();
                    self.notice("You left the match.");
                }
                self.current = None;
                if let Some(w) = self.app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        }
    }

    // ------------------------------------------------------------- helpers

    fn is_current(&self, round: u64) -> bool {
        self.current.as_ref().is_some_and(|c| c.round == round)
    }

    /// The kind of the challenge dealt for `round`, from the library.
    fn dealt_kind(&self, round: u64) -> Option<Kind> {
        let c = self.dealt.get(&round)?;
        let lib = self.library.catalog.game(&c.game)?;
        Some(lib.challenge(&c.challenge)?.kind.into())
    }

    /// Where a bout's probes go: the server's own word, with `:port` meaning
    /// the host this link dialled (ADR 0036). `None` on the in-process
    /// server, which deals no bouts.
    fn rendezvous_addr(&self) -> Option<SocketAddr> {
        let said = self.rendezvous.as_deref()?;
        let addr = match said.strip_prefix(':') {
            Some(port) => {
                let host = self.link.as_ref().and_then(|l| l.server.as_ref()).map(|s| s.host().to_string())?;
                format!("{host}:{port}")
            }
            None => said.to_string(),
        };
        match addr.to_socket_addrs() {
            Ok(mut a) => a.find(|a| a.is_ipv4()).or_else(|| addr.to_socket_addrs().ok()?.next()),
            Err(e) => {
                eprintln!("[bout] rendezvous {addr}: {e}");
                None
            }
        }
    }

    // ---------------------------------------------------------- spectating

    /// This player's part in the round is over while others play on: watch
    /// the first of them. The sheet is interactive, so the mouse is ours.
    fn start_spectating(&mut self, round: u64, players: Vec<String>) {
        let me = self.player.clone().unwrap_or_default();
        let players: Vec<String> = players.into_iter().filter(|p| *p != me).collect();
        let first = players.first().cloned();
        self.spectate = Some(Spectate { round, players, watching: None });
        self.passthrough(false);
        self.watch(first);
    }

    /// Watch `player`, or nobody, in the round being spectated.
    fn watch(&mut self, player: Option<String>) {
        let Some(s) = self.spectate.as_mut() else { return };
        if let Some(p) = &player {
            if !s.players.contains(p) {
                return;
            }
        }
        s.watching = player.clone();
        let round = s.round;
        // Asked for before the server is told, so the backlog it answers
        // with finds the replica ready for it.
        let replay = self.dealt_kind(round) != Some(Kind::HeadToHead);
        if replay {
            let watching = match &player {
                Some(p) => match self.load_for(round) {
                    Ok(load) => Some(Box::new(Watching { load, player: p.clone() })),
                    Err(e) => {
                        self.emit(json!({ "kind": "replay", "player": p, "state": "lost", "text": format!("cannot replay this run here: {e}") }));
                        None
                    }
                },
                None => None,
            };
            if let Some(g) = &self.game {
                let _ = g.send(Command::Watch(watching));
            }
        }
        self.send(ToServer::Watch { round, player });
        self.emit_spectate();
    }

    fn emit_spectate(&self) {
        if let Some(s) = &self.spectate {
            let replay = self.dealt_kind(s.round) != Some(Kind::HeadToHead);
            self.emit(json!({
                "kind": "spectate", "round": s.round, "player": s.watching, "players": s.players,
                "replay": replay,
            }));
        }
    }

    /// The round is over, or the match: nothing to watch, nothing to send.
    fn stop_spectating(&mut self) {
        if let Some(s) = self.spectate.take() {
            if s.watching.is_some() {
                self.send(ToServer::Watch { round: s.round, player: None });
            }
        }
        if let Some(g) = &self.game {
            let _ = g.send(Command::Stream(0));
            let _ = g.send(Command::Watch(None));
        }
    }

    /// What preparing `round` takes on this machine: its content and this
    /// player's settings for its game. A player loads it to play; a watcher
    /// loads the same to replay someone else's run (ADR 0037).
    fn load_for(&self, round: u64) -> Result<Load, String> {
        let challenge = self.dealt.get(&round).ok_or_else(|| format!("round {round} was never dealt"))?;
        let &(seed, limit) = self.terms.get(&round).ok_or_else(|| format!("round {round} was never dealt"))?;
        let lib = self
            .library
            .catalog
            .game(&challenge.game)
            .ok_or_else(|| format!("this client does not hold the game `{}`", challenge.game))?;
        let c = lib
            .challenge(&challenge.challenge)
            .ok_or_else(|| format!("this client does not hold the challenge `{}`", challenge.challenge))?;
        let game = &challenge.game;
        let core = &self.library.cores[game];
        let lr = &self.library.runner[game].libretro;
        let (input, mut video, display) = {
            let s = self.settings.lock().unwrap();
            (s.resolve_input(core, game, lr.controller, &lr.pad_keys), s.resolve_video(core), s.video.display)
        };
        if let Some(scale) = self.scale {
            video.window_scale = scale;
        }
        Ok(Load {
            round,
            manifest: self.library.paths[game].clone(),
            stage: c.stage.0.clone(),
            challenge: c.id.0.clone(),
            seed,
            limit,
            input,
            video,
            display,
            bout: None,
            script: None,
            give_up_after: None,
        })
    }

    /// This client cannot run the round; the server voids it (ADR 0029).
    fn unprepared(&mut self, round: u64, reason: String) {
        eprintln!("[match] round {round} cannot be prepared: {reason}");
        self.current = None;
        self.send(ToServer::Unprepared { round, reason });
    }

    /// Sends the file report if it is not what the server last heard.
    fn send_files(&mut self) {
        let report = self.library.file_report();
        if self.files.as_ref() != Some(&report) {
            self.send(ToServer::Files(report.clone()));
            self.files = Some(report);
        }
    }

    fn start_ticking(&mut self) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        let tx = self.tx.clone();
        thread::Builder::new()
            .name("recheck".into())
            .spawn(move || loop {
                thread::sleep(RECHECK);
                if tx.send(Msg::Ui(UiCmd::Recheck)).is_err() {
                    break;
                }
            })
            .expect("spawn the recheck timer");
    }

    fn send(&self, msg: ToServer) {
        if let Some(link) = &self.link {
            link.send(msg);
        }
    }

    fn notice(&self, text: &str) {
        let _ = self.app.emit_to("main", "notice", &json!({ "text": text }));
    }

    fn connect(&mut self, server: &str) -> Result<Link, String> {
        self.generation += 1;
        Link::connect(server, self.generation, self.tx.clone())
    }

    /// Says hello on a new link and reports this machine's files.
    fn hello(&mut self, link: Link, name: String, lobby: Option<String>) {
        self.link = Some(link);
        let name = if name.trim().is_empty() { self.settings.lock().unwrap().name.clone() } else { name };
        self.send(ToServer::Hello { name, library_hash: self.library.hash_hex(), lobby });
        self.files = None;
        self.send_files();
        self.start_ticking();
    }

    /// Off the server: the link dropped, back to the menu.
    fn leave(&mut self) {
        if self.link.take().is_none() {
            return;
        }
        self.generation += 1;
        self.player = None;
        self.lobby = None;
        self.files = None;
        self.auto_started = false;
        if self.game.is_some() {
            self.leave_table();
        }
        let _ = self.app.emit_to("main", "left", &json!({}));
    }

    fn is_host(&self) -> bool {
        match (&self.lobby, &self.player) {
            (Some(l), Some(me)) => l.players.first().is_some_and(|p| &p.id == me),
            _ => false,
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
            && (!self.auto.head_to_head || lobby.settings.allow_head_to_head)
            && self.auto.players.map_or(true, |n| lobby.players.len() >= n)
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
        thread::Builder::new()
            .name("game".into())
            .spawn(move || {
                if let Err(e) = game::run(cmd_rx, rep_tx) {
                    eprintln!("[game] {e}");
                }
            })
            .expect("spawn the game thread");
        let to_flow = self.tx.clone();
        thread::spawn(move || {
            // One `Closed` per window: the game thread's own, or this one if
            // it ended without saying (an error, a panic).
            let mut closed = false;
            for r in rep_rx {
                closed |= matches!(r, Report::Closed);
                if to_flow.send(Msg::Game(r)).is_err() {
                    return;
                }
            }
            if !closed {
                let _ = to_flow.send(Msg::Game(Report::Closed));
            }
        });
        self.game = Some(cmd_tx);
    }

    fn leave_table(&mut self) {
        if let Some(g) = self.game.take() {
            self.closing = true;
            let _ = g.send(Command::Quit);
        }
        self.current = None;
        self.spectate = None;
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
        let me = self.player.clone().unwrap_or_default();
        let vs: Vec<String> = self
            .current
            .as_ref()
            .and_then(|cur| self.slots.get(&cur.round))
            .map(|s| s.iter().filter(|p| **p != me).cloned().collect())
            .unwrap_or_default();
        json!({
            "kind": "intro",
            "title": ch.display_title(lib),
            "name": ch.name,
            "console": lib.game.console.to_uppercase(),
            "goal": ch.goal_text(),
            "vs": if ch.is_head_to_head() { vs } else { Vec::new() },
            "challenge_kind": match ch.kind {
                session_api::ChallengeKind::TimeTrial => "Time trial",
                session_api::ChallengeKind::Survival => "Survival",
                session_api::ChallengeKind::HeadToHead => "Head-to-head",
            },
            "custom": ch.is_custom(),
        })
    }
}

/// A watched player's event in a few words, for the panel a watcher sees
/// when the picture cannot reach them (ADR 0034).
fn said(e: &session_api::SessionEvent) -> String {
    use session_api::SessionEvent;
    match e {
        SessionEvent::StageEntered { .. } => "entered the stage".into(),
        SessionEvent::StageCleared { .. } => "cleared the stage".into(),
        SessionEvent::StageExited { .. } => "left the stage".into(),
        SessionEvent::Died { .. } => "died".into(),
        SessionEvent::Damaged { .. } => "took a hit".into(),
        SessionEvent::EnemyDefeated { .. } => "defeated an enemy".into(),
        SessionEvent::ItemCollected { item, .. } => format!("got {}", item.replace('_', " ")),
        SessionEvent::Counted { counter, .. } => counter.replace('_', " "),
        SessionEvent::ObjectiveReached { objective, .. } => format!("reached {}", objective.replace('-', " ")),
    }
}
