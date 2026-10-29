//! The GamerTrials client (ADR 0027): a native game window with a transparent
//! Tauri overlay over it.
//!
//! This is the window spike that Phase B opens with. `cargo run -p app-client
//! -- --game smb3` plays a stage behind a transparent webview that runs the
//! placeholder intro, countdown and death flash; `--spike` adds a scripted
//! self-test that moves and resizes the game window, injects a keypress with
//! the overlay focused, and logs whether the overlay followed and the game
//! heard the key.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod game;
mod keys;
mod overlay;

use std::sync::mpsc;
use std::time::Duration;

use tauri::Manager;



struct Args {
    game: String,
    stage: Option<String>,
    scale: usize,
    spike: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { game: "smb3".into(), stage: None, scale: 2, spike: false };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--game" => args.game = it.next().ok_or("--game wants a name")?,
            "--stage" => args.stage = Some(it.next().ok_or("--stage wants an id")?),
            "--scale" => {
                args.scale = it.next().ok_or("--scale wants 1, 2 or 4")?.parse().map_err(|_| "--scale wants a number")?
            }
            "--spike" => args.spike = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(2);
        }
    };
    let (cmd_tx, cmd_rx) = mpsc::channel::<game::Command>();
    let (rep_tx, rep_rx) = mpsc::channel::<game::Report>();
    let cfg = game::Config {
        manifest: game::manifest_path(&args.game),
        stage: args.stage.clone(),
        scale: args.scale,
    };
    let spike = args.spike;

    tauri::Builder::default()
        .setup(move |app| {
            let handle = app.handle().clone();
            std::thread::Builder::new()
                .name("game".into())
                .spawn(move || {
                    if let Err(e) = game::run(cfg, cmd_rx, rep_tx) {
                        eprintln!("[game] {e}");
                        handle.exit(1);
                    } else {
                        println!("[client] game window closed");
                        handle.exit(0);
                    }
                })
                .expect("spawn game thread");

            // The game window comes up once the core has loaded and the entry
            // state is restored; a cold DOS boot can take a while.
            let (hwnd, width, height, title, console) = loop {
                match rep_rx.recv_timeout(Duration::from_secs(60)) {
                    Ok(game::Report::Window { hwnd, width, height, title, console }) => {
                        break (hwnd, width, height, title, console)
                    }
                    Ok(game::Report::Closed) => return Err("the game window closed before it opened".into()),
                    Ok(other) => println!("[client] before window: {other:?}"),
                    Err(_) => return Err("no game window after 60 s".into()),
                }
            };
            println!("[client] game window {hwnd:#x}, {width}x{height} native");

            let ov = overlay::Overlay::create(app.handle(), hwnd)?;
            let _ = cmd_tx.send(game::Command::Overlay(ov.hwnd()));
            ov.set_passthrough(true)?;

            // Keep the overlay on the game window's client area.
            {
                let ov = ov.clone();
                std::thread::spawn(move || ov.track());
            }

            // Forward what the game reports to the page.
            {
                let ov = ov.clone();
                std::thread::spawn(move || {
                    for r in rep_rx {
                        match r {
                            game::Report::Died { frame } => ov.emit(serde_json::json!({ "kind": "death", "frame": frame })),
                            game::Report::Cleared { frame } => ov.emit(serde_json::json!({ "kind": "garland", "frame": frame })),
                            game::Report::Exited { frame } => ov.emit(serde_json::json!({ "kind": "skull", "frame": frame })),
                            game::Report::Input { bits, overlay_in_front } => {
                                println!("[client] input bits {bits:#x}, overlay in front: {overlay_in_front}");
                            }
                            game::Report::Closed => break,
                            game::Report::Window { .. } => {}
                        }
                    }
                });
            }

            // The flow every challenge will have: intro, countdown, go.
            {
                let ov = ov.clone();
                let cmd_tx = cmd_tx.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(Duration::from_millis(800));
                    ov.emit(serde_json::json!({
                        "kind": "intro", "title": title, "console": console, "goal": "Reach the end!"
                    }));
                    std::thread::sleep(Duration::from_millis(2500));
                    ov.emit(serde_json::json!({ "kind": "intro-out" }));
                    std::thread::sleep(Duration::from_millis(600));
                    for n in [3, 2, 1] {
                        ov.emit(serde_json::json!({ "kind": "countdown", "n": n }));
                        std::thread::sleep(Duration::from_millis(900));
                    }
                    let _ = cmd_tx.send(game::Command::Resume);
                    ov.emit(serde_json::json!({ "kind": "go" }));
                });
            }

            // The game thread treats a disconnected command channel as quit, so
            // a sender lives as long as the app does; it is also how the
            // client's own commands will reach the game.
            app.manage(cmd_tx);

            if spike {
                let ov = ov.clone();
                let app = app.handle().clone();
                std::thread::spawn(move || {
                    overlay::spike(&ov, hwnd);
                    app.exit(0);
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
