//! The server on a socket (ADR 0030). `server::Server` is a state machine
//! with no transport; this crate is the transport, and nothing else.
//!
//! Three kinds of thread:
//!
//! * **Accept.** [`serve`] takes connections and gives each its own thread.
//! * **Connection.** Speaks WebSocket to one client: each text frame is one
//!   [`protocol`] packet. Its first message must be a `Hello`, and
//!   `Hello.lobby` picks the lobby: `None` makes a new one, a code joins it.
//! * **Lobby.** One per lobby, owning its `Server`. Connections send it what
//!   they read; it hands each reply to the connection it is addressed to. A
//!   lobby's rules are the state machine's, so a lobby here behaves exactly
//!   like the client's in-process one (ADR 0009).
//!
//! A lobby ends when its last connection does. Nothing is stored. A
//! connection ends when its socket closes, or when nothing has come from it
//! for [`Liveness::silence`]: a player whose link dropped leaves as one who
//! closed it does, in seconds rather than whenever TCP gives up.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fs;
use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use challenge_library::Catalog;
use protocol::{Beat, Heartbeat, Liveness, ToClient, ToServer};
use server::{ConnId, Server};
use tungstenite::{Message, WebSocket};

/// The port a client assumes when an invite code names none.
pub const DEFAULT_PORT: u16 = 7878;

/// How long a connection waits for a frame before it looks for replies to
/// send. Bounds the latency the server adds to a reply.
const POLL: Duration = Duration::from_millis(5);

/// Reads every manifest in `dir`, as `app-client`'s `Library::load` does, so
/// the two hash the same texts and a client holding the same library agrees.
pub fn load_catalog(dir: &Path) -> Result<Catalog, String> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut files: Vec<_> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "toml"))
        .collect();
    files.sort();
    let mut texts = Vec::with_capacity(files.len());
    for path in &files {
        texts.push(fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?);
    }
    if texts.is_empty() {
        return Err(format!("no manifests in {}", dir.display()));
    }
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    Catalog::parse_all(&refs)
}

/// What a connection tells its lobby.
enum LobbyMsg {
    /// A new connection, with where its replies go. Answered with its id.
    Open(Sender<ToClient>, Sender<ConnId>),
    Msg(ConnId, ToServer),
    Closed(ConnId),
}

struct Router {
    catalog: Catalog,
    liveness: Liveness,
    lobbies: Mutex<HashMap<String, Sender<LobbyMsg>>>,
    /// Stepped for every code and seed.
    entropy: Mutex<u64>,
}

impl Router {
    fn next_random(&self) -> u64 {
        let mut state = self.entropy.lock().unwrap();
        *state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = *state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A new lobby, on its own thread, under a code nobody else holds.
    fn create(self: &Arc<Self>) -> (String, Sender<LobbyMsg>) {
        let mut lobbies = self.lobbies.lock().unwrap();
        let code = loop {
            let code = code_from(self.next_random());
            if !lobbies.contains_key(&code) {
                break code;
            }
        };
        let seed = self.next_random();
        let (tx, rx) = mpsc::channel();
        lobbies.insert(code.clone(), tx.clone());
        let router = Arc::clone(self);
        let (name, c) = (format!("lobby-{code}"), code.clone());
        thread::Builder::new()
            .name(name)
            .spawn(move || lobby(router, c, seed, rx))
            .expect("spawn a lobby thread");
        println!("[lobby {code}] open, seeded {seed}");
        (code, tx)
    }

    fn find(&self, code: &str) -> Option<Sender<LobbyMsg>> {
        self.lobbies.lock().unwrap().get(&code.to_ascii_uppercase()).cloned()
    }
}

/// Five letters with no I or O, which read as 1 and 0.
fn code_from(mut x: u64) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ";
    (0..5)
        .map(|_| {
            let c = ALPHABET[(x % ALPHABET.len() as u64) as usize] as char;
            x /= ALPHABET.len() as u64;
            c
        })
        .collect()
}

/// A lobby's thread: its `Server`, and who each reply goes to.
fn lobby(router: Arc<Router>, code: String, seed: u64, rx: Receiver<LobbyMsg>) {
    let mut server = Server::new(router.catalog.clone(), code.clone(), seed);
    let mut conns: HashMap<ConnId, Sender<ToClient>> = HashMap::new();
    for msg in rx.iter() {
        let out = match msg {
            LobbyMsg::Open(reply, ack) => {
                let id = server.connect();
                conns.insert(id, reply);
                let _ = ack.send(id);
                continue;
            }
            LobbyMsg::Msg(id, m) => server.handle(id, m),
            LobbyMsg::Closed(id) => {
                conns.remove(&id);
                let out = server.disconnect(id);
                if conns.is_empty() {
                    // Joiners that found the lobby before this were queued
                    // behind it; dropping `rx` drops their acks, and they are
                    // told there is no such lobby.
                    router.lobbies.lock().unwrap().remove(&code);
                    println!("[lobby {code}] closed");
                    return;
                }
                out
            }
        };
        for (to, reply) in out {
            if let Some(tx) = conns.get(&to) {
                let _ = tx.send(reply);
            }
        }
    }
}

/// Serves until the listener fails. Each connection gets a thread.
/// `liveness` is [`Liveness::DEFAULT`] but for tests that cannot wait for it.
pub fn serve(listener: TcpListener, catalog: Catalog, liveness: Liveness) -> io::Result<()> {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED);
    let router = Arc::new(Router {
        catalog,
        liveness,
        lobbies: Mutex::new(HashMap::new()),
        entropy: Mutex::new(seed),
    });
    for stream in listener.incoming() {
        let stream = stream?;
        let router = Arc::clone(&router);
        let peer = stream.peer_addr().map(|a| a.to_string()).unwrap_or_default();
        thread::Builder::new()
            .name(format!("conn-{peer}"))
            .spawn(move || {
                if let Err(e) = connection(stream, &router) {
                    println!("[conn {peer}] {e}");
                }
            })
            .expect("spawn a connection thread");
    }
    Ok(())
}

/// Binds and serves on a thread; the address it bound. For tests, and for
/// anything else that wants a server without owning the accept loop.
pub fn spawn(addr: &str, catalog: Catalog, liveness: Liveness) -> io::Result<std::net::SocketAddr> {
    let listener = TcpListener::bind(addr)?;
    let bound = listener.local_addr()?;
    thread::Builder::new()
        .name("accept".into())
        .spawn(move || serve(listener, catalog, liveness))
        .expect("spawn the accept thread");
    Ok(bound)
}

/// Where a connection is.
enum Seat {
    /// Has not said hello.
    New,
    In { lobby: Sender<LobbyMsg>, id: ConnId },
}

fn connection(stream: TcpStream, router: &Arc<Router>) -> Result<(), String> {
    let silence = router.liveness.silence;
    stream.set_nodelay(true).map_err(|e| e.to_string())?;
    // A peer that stops reading is as gone as one that stops writing: a
    // write that cannot finish in `silence` ends the connection too. So does
    // a handshake that never does.
    stream.set_write_timeout(Some(silence)).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(silence)).map_err(|e| e.to_string())?;
    let mut ws = tungstenite::accept(stream).map_err(|e| format!("handshake: {e}"))?;
    ws.get_ref().set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
    let (reply_tx, reply_rx) = mpsc::channel::<ToClient>();
    let mut seat = Seat::New;
    let result = pump(&mut ws, router, &mut seat, &reply_tx, &reply_rx);
    if let Seat::In { lobby, id } = seat {
        let _ = lobby.send(LobbyMsg::Closed(id));
    }
    let _ = ws.close(None);
    let _ = ws.flush();
    result
}

/// Reads frames and writes replies until the socket closes or goes silent.
fn pump(
    ws: &mut WebSocket<TcpStream>,
    router: &Arc<Router>,
    seat: &mut Seat,
    reply_tx: &Sender<ToClient>,
    reply_rx: &Receiver<ToClient>,
) -> Result<(), String> {
    let mut heart = Heartbeat::new(router.liveness, Instant::now());
    loop {
        let read = ws.read();
        if read.is_ok() {
            heart.heard(Instant::now());
        }
        match read {
            Ok(Message::Text(text)) => {
                let msg = match protocol::decode::<ToServer>(text.as_str()) {
                    Ok(m) => m,
                    Err(e) => {
                        // A peer on another version cannot read this either,
                        // but its own decode says why.
                        send(ws, &ToClient::Refused { reason: e.to_string() })?;
                        return Err(e.to_string());
                    }
                };
                if !route(router, seat, reply_tx, msg) {
                    while let Ok(m) = reply_rx.try_recv() {
                        send(ws, &m)?;
                    }
                    return Ok(());
                }
            }
            Ok(Message::Close(_)) => return Ok(()),
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {}
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(())
            }
            Err(e) => return Err(e.to_string()),
        }
        while let Ok(m) = reply_rx.try_recv() {
            send(ws, &m)?;
        }
        match heart.check(Instant::now()) {
            Beat::Wait => {}
            Beat::Ping => ws.send(Message::Ping(Default::default())).map_err(|e| e.to_string())?,
            Beat::Silent => return Err(format!("nothing heard in {}s", heart.silence().as_secs())),
        }
    }
}

fn send(ws: &mut WebSocket<TcpStream>, m: &ToClient) -> Result<(), String> {
    ws.send(Message::Text(protocol::encode(m).into())).map_err(|e| e.to_string())
}

/// Hands a message to the connection's lobby, finding or making the lobby on
/// the first. `false` when the connection should close: it asked for a lobby
/// that is not there.
fn route(router: &Arc<Router>, seat: &mut Seat, reply_tx: &Sender<ToClient>, msg: ToServer) -> bool {
    match seat {
        Seat::In { lobby, id } => {
            let _ = lobby.send(LobbyMsg::Msg(*id, msg));
            true
        }
        Seat::New => {
            let ToServer::Hello { lobby: code, .. } = &msg else {
                let _ = reply_tx.send(ToClient::Refused { reason: "say hello first".into() });
                return true;
            };
            let refuse = |reason: String| {
                let _ = reply_tx.send(ToClient::Refused { reason });
                false
            };
            let lobby = match code {
                None => router.create().1,
                Some(code) => match router.find(code) {
                    Some(l) => l,
                    None => return refuse(format!("there is no lobby {code}")),
                },
            };
            let (ack_tx, ack_rx) = mpsc::channel();
            if lobby.send(LobbyMsg::Open(reply_tx.clone(), ack_tx)).is_err() {
                return refuse("that lobby has closed".into());
            }
            let Ok(id) = ack_rx.recv() else {
                return refuse("that lobby has closed".into());
            };
            let _ = lobby.send(LobbyMsg::Msg(id, msg));
            *seat = Seat::In { lobby, id };
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_five_unambiguous_letters() {
        for x in [0, 1, u64::MAX, 0xDEAD_BEEF] {
            let c = code_from(x);
            assert_eq!(c.len(), 5);
            assert!(c.chars().all(|ch| ch.is_ascii_uppercase() && ch != 'I' && ch != 'O'), "{c}");
        }
        assert_ne!(code_from(1), code_from(2));
    }
}
