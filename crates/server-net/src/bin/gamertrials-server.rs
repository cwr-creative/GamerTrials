//! `gamertrials-server [--listen <addr>] [--manifests <dir>] [--udp <port>|off] [--udp-public <host:port>]`
//!
//! The server as its own process (ADR 0030): lobbies by invite code, each
//! dealt and judged by `server::Server`. It reads the challenge half of the
//! manifests and nothing else; a client must hold the same library to join.
//!
//! `--udp` is the port head-to-head players probe (ADR 0036), the listen
//! port by default; `off` deals no bouts. `--udp-public` is what players are
//! told to probe when a proxy fronts the WebSocket (ADR 0033), since UDP does
//! not pass through it.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut listen = format!("0.0.0.0:{}", server_net::DEFAULT_PORT);
    let mut manifests = PathBuf::from("manifests");
    let mut udp: Option<String> = None;
    let mut udp_public: Option<String> = None;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        let mut value = |name: &str| {
            args.next().unwrap_or_else(|| {
                eprintln!("{name} needs a value");
                std::process::exit(2)
            })
        };
        match arg.as_str() {
            "--listen" => listen = value("--listen"),
            "--manifests" => manifests = PathBuf::from(value("--manifests")),
            "--udp" => udp = Some(value("--udp")),
            "--udp-public" => udp_public = Some(value("--udp-public")),
            "-h" | "--help" => {
                println!(
                    "gamertrials-server [--listen <addr>] [--manifests <dir>] [--udp <port>|off] \
                     [--udp-public <host:port>]"
                );
                return ExitCode::SUCCESS;
            }
            other => {
                eprintln!("unknown argument: {other}");
                return ExitCode::from(2);
            }
        }
    }
    let catalog = match server_net::load_catalog(&manifests) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let listener = match std::net::TcpListener::bind(&listen) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("cannot listen on {listen}: {e}");
            return ExitCode::FAILURE;
        }
    };
    let rendezvous = match udp.as_deref() {
        Some("off") => None,
        port => {
            let host = listen.rsplit_once(':').map(|(h, _)| h).unwrap_or("0.0.0.0");
            let port = port
                .map(str::to_string)
                .unwrap_or_else(|| listener.local_addr().map(|a| a.port().to_string()).unwrap_or_default());
            match server_net::Rendezvous::bind(&format!("{host}:{port}"), udp_public) {
                Ok(r) => Some(r),
                Err(e) => {
                    eprintln!("cannot bind udp {host}:{port}: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    };
    println!(
        "gamertrials-server on {listen}: {} games, library {:016x}, protocol v{}, head-to-head {}",
        catalog.games().len(),
        catalog.hash(),
        protocol::VERSION,
        match &rendezvous {
            Some(r) => format!("probes to {}", r.public),
            None => "off".into(),
        }
    );
    match server_net::serve(listener, catalog, transport::Rules::DEFAULT, rendezvous) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
