//! The server's half, in-process (ADR 0009): a lobby of one, on a thread, over
//! a channel. It is the `server` crate's state machine unchanged; what a
//! remote server adds is a socket in front of exactly this.

use std::sync::mpsc::{self, Sender};
use std::thread;

use challenge_library::Catalog;
use protocol::ToServer;
use server::Server;

use crate::flow::Msg;

/// Starts a local server and connects one client to it. What the server says
/// arrives on `out` as [`Msg::Server`]; what the client says goes down the
/// returned sender.
pub fn spawn(catalog: Catalog, seed: u64, out: Sender<Msg>) -> Sender<ToServer> {
    let (tx, rx) = mpsc::channel::<ToServer>();
    thread::Builder::new()
        .name("local-server".into())
        .spawn(move || {
            let mut server = Server::new(catalog, "local", seed);
            let conn = server.connect();
            for msg in rx {
                for (to, reply) in server.handle(conn, msg) {
                    if to == conn && out.send(Msg::Server(reply)).is_err() {
                        return;
                    }
                }
            }
        })
        .expect("spawn the local server");
    tx
}

/// A seed nobody chose: the clock. The server derives every later seed from
/// it, so a replay from the logged value reproduces the match.
pub fn fresh_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
}
