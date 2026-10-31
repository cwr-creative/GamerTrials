//! The client's line to a server: the in-process one (ADR 0009) or a
//! `gamertrials-server` over a WebSocket (ADR 0030). Either way the flow
//! sends `ToServer` down a channel and hears `ToClient` as [`Msg::Server`],
//! so nothing above this module knows which it has.
//!
//! A server is named `[ws://|wss://]host[:port]`. `wss://` is TLS, ended by
//! whatever stands in front of the server (ADR 0033); the port is 443 for it
//! and `server-net`'s otherwise, when none is written.
//!
//! The socket runs `transport`'s loop, as each of the server's connections
//! does: the match goes before this player's stream, a stream the uplink
//! cannot keep up with is dropped rather than queued, and a server silent
//! for [`protocol::Liveness::silence`] has gone (ADR 0034).

use std::net::{TcpStream, ToSocketAddrs};
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

use challenge_library::Catalog;
use protocol::{ToClient, ToServer};
use transport::{End, Ends, Next, Rules};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::WebSocket;

use crate::flow::Msg;
use crate::local;

/// The port an address or invite assumes when it names none (`server-net`'s).
pub const DEFAULT_PORT: u16 = 7878;
/// And for `wss://`, where a proxy in front of the server answers.
pub const TLS_PORT: u16 = 443;

type Socket = WebSocket<MaybeTlsStream<TcpStream>>;

/// A server as a player names it, normalised: the scheme decides TLS, and
/// `addr` is always `host:port`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Server {
    pub tls: bool,
    pub addr: String,
}

impl Server {
    /// `host`, `host:port`, or either behind `ws://` or `wss://`.
    pub fn parse(text: &str) -> Server {
        let s = text.trim().trim_end_matches('/');
        let (tls, s) = match s.strip_prefix("wss://") {
            Some(rest) => (true, rest),
            None => (false, s.strip_prefix("ws://").unwrap_or(s)),
        };
        let s = s.trim_end_matches('/');
        let addr = if s.rsplit_once(':').is_some_and(|(_, p)| p.parse::<u16>().is_ok()) {
            s.to_string()
        } else {
            format!("{s}:{}", if tls { TLS_PORT } else { DEFAULT_PORT })
        };
        Server { tls, addr }
    }

    /// The URL dialled.
    pub fn url(&self) -> String {
        format!("{}://{}/", if self.tls { "wss" } else { "ws" }, self.addr)
    }

    /// As written in an invite or shown in the lobby: the scheme only when
    /// it is TLS, the port only when it is not the default.
    pub fn display(&self) -> String {
        let default = if self.tls { TLS_PORT } else { DEFAULT_PORT };
        let addr = self.addr.strip_suffix(&format!(":{default}")).unwrap_or(&self.addr);
        if self.tls {
            format!("wss://{addr}")
        } else {
            addr.to_string()
        }
    }

    /// The name a certificate must carry.
    fn host(&self) -> &str {
        self.addr.rsplit_once(':').map_or(&self.addr, |(h, _)| h)
    }
}

pub struct Link {
    tx: Sender<ToServer>,
    /// The networked server; `None` for the in-process one.
    pub server: Option<Server>,
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
        let server = Server::parse(server);
        // A name may resolve to several addresses — `localhost` is `::1`
        // before `127.0.0.1` on Windows, and the server may listen on one
        // family only — so the first that answers is the one.
        let addrs: Vec<_> = server.addr.to_socket_addrs().map_err(|e| format!("{}: {e}", server.addr))?.collect();
        if addrs.is_empty() {
            return Err(format!("{}: no address", server.addr));
        }
        let mut last = None;
        let mut stream = None;
        for addr in &addrs {
            match TcpStream::connect_timeout(addr, Duration::from_secs(3)) {
                Ok(s) => {
                    stream = Some(s);
                    break;
                }
                Err(e) => last = Some(e),
            }
        }
        let stream = stream.ok_or_else(|| {
            format!("cannot reach {}: {}", server.display(), last.map(|e| e.to_string()).unwrap_or_default())
        })?;
        // A handshake that cannot finish in `silence` is a server that has gone.
        let silence = Rules::DEFAULT.liveness.silence;
        stream.set_write_timeout(Some(silence)).map_err(|e| e.to_string())?;
        stream.set_read_timeout(Some(silence)).map_err(|e| e.to_string())?;
        // `client_tls` reads the scheme: `ws://` leaves the socket plain,
        // `wss://` wraps it in TLS against `host`'s certificate.
        let (mut ws, _): (Socket, _) = tungstenite::client_tls(server.url(), stream)
            .map_err(|e| format!("{} ({}): {e}", server.display(), server.host()))?;
        transport::prepare(tcp(&ws)?).map_err(|e| e.to_string())?;
        let (tx, rx) = mpsc::channel::<ToServer>();
        thread::Builder::new()
            .name("server-link".into())
            .spawn(move || {
                let mut incoming = |m: ToClient| match out.send(Msg::Server(m)) {
                    Ok(()) => Next::Go,
                    Err(_) => Next::Stop,
                };
                let end = transport::run(
                    &mut ws,
                    Rules::DEFAULT,
                    &rx,
                    Ends { incoming: &mut incoming, key_wanted: &mut crate::stream::want_key, label: "link" },
                );
                let reason = match end {
                    // The flow let go of the link: close quietly.
                    End::Done => {
                        transport::close_with(&mut ws, Vec::<ToServer>::new(), Duration::from_secs(1));
                        return;
                    }
                    End::Closed => "the server closed the connection".to_string(),
                    e => e.to_string(),
                };
                let _ = out.send(Msg::Disconnected { generation, reason });
            })
            .expect("spawn the server link");
        println!("[link] connected to {}", server.display());
        Ok(Link { tx, server: Some(server) })
    }

    pub fn send(&self, msg: ToServer) {
        let _ = self.tx.send(msg);
    }
}

/// The TCP stream under the WebSocket, TLS or not.
fn tcp(ws: &Socket) -> Result<&TcpStream, String> {
    match ws.get_ref() {
        MaybeTlsStream::Plain(s) => Ok(s),
        MaybeTlsStream::NativeTls(s) => Ok(s.get_ref()),
        _ => Err("an unexpected stream".into()),
    }
}

/// An invite is `[wss://]host[:port]/CODE`, or a bare `CODE` on
/// `default_server`. Returns the server and the code.
pub fn parse_invite(invite: &str, default_server: Option<&str>) -> Result<(Server, String), String> {
    let invite = invite.trim();
    let (server, code) = match invite.rsplit_once('/') {
        Some((s, c)) if !s.is_empty() && !s.ends_with(':') && s != "ws:/" && s != "wss:/" => (s.to_string(), c),
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
    Ok((Server::parse(&server), code))
}

/// What a host hands out: the server and the lobby's code.
pub fn invite(server: &Server, code: &str) -> String {
    format!("{}/{code}", server.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(tls: bool, addr: &str) -> Server {
        Server { tls, addr: addr.into() }
    }

    #[test]
    fn an_address_gets_the_default_port_unless_it_names_one() {
        assert_eq!(Server::parse("example.com"), s(false, "example.com:7878"));
        assert_eq!(Server::parse(" 10.0.0.2:9000 "), s(false, "10.0.0.2:9000"));
        assert_eq!(Server::parse("ws://host/"), s(false, "host:7878"));
        assert_eq!(Server::parse("wss://games.example.com"), s(true, "games.example.com:443"));
        assert_eq!(Server::parse("wss://games.example.com:8443/"), s(true, "games.example.com:8443"));
        assert_eq!(Server::parse("wss://h").url(), "wss://h:443/");
        assert_eq!(Server::parse("h").url(), "ws://h:7878/");
    }

    #[test]
    fn invites_carry_the_server_or_borrow_the_profiles() {
        assert_eq!(parse_invite("host:9000/kxqfm", None), Ok((s(false, "host:9000"), "KXQFM".into())));
        assert_eq!(parse_invite("host/KXQFM", None), Ok((s(false, "host:7878"), "KXQFM".into())));
        assert_eq!(parse_invite("KXQFM", Some("lan-box")), Ok((s(false, "lan-box:7878"), "KXQFM".into())));
        assert_eq!(parse_invite("KXQFM", Some("wss://games.example.com")), Ok((s(true, "games.example.com:443"), "KXQFM".into())));
        assert_eq!(parse_invite("wss://games.example.com/KXQFM", None), Ok((s(true, "games.example.com:443"), "KXQFM".into())));
        assert!(parse_invite("KXQFM", None).is_err());
        assert!(parse_invite("host/", None).is_err());
        assert!(parse_invite("wss://KXQFM", None).is_err(), "a scheme with no host is not a server");
    }

    #[test]
    fn an_invite_leaves_the_default_port_off_and_reads_back() {
        assert_eq!(invite(&s(false, "host:7878"), "KXQFM"), "host/KXQFM");
        assert_eq!(invite(&s(false, "host:9000"), "KXQFM"), "host:9000/KXQFM");
        assert_eq!(invite(&s(true, "games.example.com:443"), "KXQFM"), "wss://games.example.com/KXQFM");
        assert_eq!(invite(&s(true, "games.example.com:8443"), "KXQFM"), "wss://games.example.com:8443/KXQFM");
        for server in [s(false, "host:9000"), s(true, "games.example.com:443"), s(true, "h:8443")] {
            assert_eq!(parse_invite(&invite(&server, "AB"), None), Ok((server.clone(), "AB".into())));
        }
    }
}
