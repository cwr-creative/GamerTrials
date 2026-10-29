//! The client's line to a server: the in-process one (ADR 0009) or a
//! `gamertrials-server` over a WebSocket (ADR 0030). Either way the flow
//! sends `ToServer` down a channel and hears `ToClient` as [`Msg::Server`],
//! so nothing above this module knows which it has.

use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use challenge_library::Catalog;
use protocol::{ToClient, ToServer};
use tungstenite::{Message, WebSocket};

use crate::flow::Msg;
use crate::local;

/// The port an address or invite assumes when it names none (`server-net`'s).
pub const DEFAULT_PORT: u16 = 7878;

/// How long the socket thread waits for a frame before sending what is queued.
const POLL: Duration = Duration::from_millis(5);

pub struct Link {
    tx: Sender<ToServer>,
    /// `host:port` of a networked server; `None` for the in-process one.
    pub server: Option<String>,
}

impl Link {
    /// A lobby of one on this machine.
    pub fn local(catalog: Catalog, out: Sender<Msg>) -> Link {
        let seed = local::fresh_seed();
        println!("[match] local server seeded {seed}");
        Link { tx: local::spawn(catalog, seed, out), server: None }
    }

    /// Connects to a `gamertrials-server`. What it says arrives on `out`; if
    /// the socket fails, a [`Msg::Disconnected`] tagged with `generation`
    /// follows. Dropping the link closes the socket quietly.
    pub fn connect(server: &str, generation: u64, out: Sender<Msg>) -> Result<Link, String> {
        let server = normalize(server);
        let addr = server
            .to_socket_addrs()
            .map_err(|e| format!("{server}: {e}"))?
            .next()
            .ok_or_else(|| format!("{server}: no address"))?;
        let stream = TcpStream::connect_timeout(&addr, Duration::from_secs(3))
            .map_err(|e| format!("cannot reach {server}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let (mut ws, _) = tungstenite::client(format!("ws://{server}/"), stream)
            .map_err(|e| format!("{server}: {e}"))?;
        ws.get_ref().set_read_timeout(Some(POLL)).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel::<ToServer>();
        thread::Builder::new()
            .name("server-link".into())
            .spawn(move || {
                let reason = loop {
                    match pump_once(&mut ws, &out) {
                        Ok(true) => {}
                        Ok(false) => return,
                        Err(reason) => break reason,
                    }
                    let mut gone = false;
                    loop {
                        match rx.try_recv() {
                            Ok(m) => {
                                if let Err(e) = ws.send(Message::Text(protocol::encode(&m).into())) {
                                    let _ = out.send(Msg::Disconnected { generation, reason: e.to_string() });
                                    return;
                                }
                            }
                            Err(TryRecvError::Empty) => break,
                            Err(TryRecvError::Disconnected) => {
                                gone = true;
                                break;
                            }
                        }
                    }
                    if gone {
                        let _ = ws.close(None);
                        let _ = ws.flush();
                        return;
                    }
                };
                let _ = out.send(Msg::Disconnected { generation, reason });
            })
            .expect("spawn the server link");
        println!("[link] connected to {server}");
        Ok(Link { tx, server: Some(server) })
    }

    pub fn send(&self, msg: ToServer) {
        let _ = self.tx.send(msg);
    }
}

/// Reads at most one frame. `Ok(false)` when the flow has gone, `Err` when
/// the server has.
fn pump_once(ws: &mut WebSocket<TcpStream>, out: &Sender<Msg>) -> Result<bool, String> {
    match ws.read() {
        Ok(Message::Text(text)) => {
            let m: ToClient = protocol::decode(text.as_str()).map_err(|e| e.to_string())?;
            Ok(out.send(Msg::Server(m)).is_ok())
        }
        Ok(Message::Close(_)) => Err("the server closed the connection".into()),
        Ok(_) => Ok(true),
        Err(tungstenite::Error::Io(e)) if matches!(e.kind(), io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut) => {
            Ok(true)
        }
        Err(e) => Err(e.to_string()),
    }
}

/// `host` or `host:port`, as `host:port`.
pub fn normalize(server: &str) -> String {
    let s = server.trim().trim_start_matches("ws://").trim_end_matches('/');
    if s.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
        s.to_string()
    } else {
        format!("{s}:{DEFAULT_PORT}")
    }
}

/// An invite is `host[:port]/CODE`, or a bare `CODE` on `default_server`.
/// Returns the server and the code.
pub fn parse_invite(invite: &str, default_server: Option<&str>) -> Result<(String, String), String> {
    let invite = invite.trim();
    let (server, code) = match invite.rsplit_once('/') {
        Some((s, c)) if !s.is_empty() => (s.to_string(), c),
        _ => match default_server.filter(|s| !s.trim().is_empty()) {
            Some(s) => (s.to_string(), invite.trim_start_matches('/')),
            None => {
                return Err("an invite looks like host:port/CODE; set a server in Settings › Profile to join by code alone".into())
            }
        },
    };
    let code = code.trim().to_ascii_uppercase();
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!("`{invite}` has no lobby code"));
    }
    Ok((normalize(&server), code))
}

/// What a host hands out: the server and the lobby's code.
pub fn invite(server: &str, code: &str) -> String {
    let server = server.strip_suffix(&format!(":{DEFAULT_PORT}")).unwrap_or(server);
    format!("{server}/{code}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_gets_the_default_port_unless_it_names_one() {
        assert_eq!(normalize("example.com"), "example.com:7878");
        assert_eq!(normalize(" 10.0.0.2:9000 "), "10.0.0.2:9000");
        assert_eq!(normalize("ws://host/"), "host:7878");
    }

    #[test]
    fn invites_carry_the_server_or_borrow_the_profiles() {
        assert_eq!(parse_invite("host:9000/kxqfm", None), Ok(("host:9000".into(), "KXQFM".into())));
        assert_eq!(parse_invite("host/KXQFM", None), Ok(("host:7878".into(), "KXQFM".into())));
        assert_eq!(parse_invite("KXQFM", Some("lan-box")), Ok(("lan-box:7878".into(), "KXQFM".into())));
        assert!(parse_invite("KXQFM", None).is_err());
        assert!(parse_invite("host/", None).is_err());
    }

    #[test]
    fn an_invite_leaves_the_default_port_off_and_reads_back() {
        assert_eq!(invite("host:7878", "KXQFM"), "host/KXQFM");
        assert_eq!(invite("host:9000", "KXQFM"), "host:9000/KXQFM");
        assert_eq!(parse_invite(&invite("host:9000", "AB"), None), Ok(("host:9000".into(), "AB".into())));
    }
}
