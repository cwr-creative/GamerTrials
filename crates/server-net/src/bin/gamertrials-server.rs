//! `gamertrials-server [--listen <addr>] [--manifests <dir>]`
//!
//! The server as its own process (ADR 0030): lobbies by invite code, each
//! dealt and judged by `server::Server`. It reads the challenge half of the
//! manifests and nothing else; a client must hold the same library to join.

use std::path::PathBuf;
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut listen = format!("0.0.0.0:{}", server_net::DEFAULT_PORT);
    let mut manifests = PathBuf::from("manifests");
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
            "-h" | "--help" => {
                println!("gamertrials-server [--listen <addr>] [--manifests <dir>]");
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
    println!(
        "gamertrials-server on {listen}: {} games, library {:016x}, protocol v{}",
        catalog.games().len(),
        catalog.hash(),
        protocol::VERSION
    );
    match server_net::serve(listener, catalog, transport::Rules::DEFAULT) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
