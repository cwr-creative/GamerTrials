//! The game thread: the harness's windowed loop, owned by the client.
//!
//! It opens the native game window, runs the emulator at the game's own pace,
//! and reports what the overlay needs to know. It never decides anything
//! about a run — it is told to hold or go, and it says what happened.

use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::time::{Duration, Instant};

use minifb::{Scale, ScaleMode, Window, WindowOptions};
use session_api::{Presentable, SessionEvent, StageRunner};
use session_libretro::libretro;
use session_libretro::manifest::{Controller, Manifest};
use session_libretro::runner::LibretroRunner;
use session_libretro::{audio, input, prep};

use crate::keys;

/// What the client tells the game thread.
#[allow(dead_code)] // Hold and Quit are the client's next callers; the spike only resumes.
pub enum Command {
    /// Stop advancing frames; keep presenting and pumping.
    Hold,
    /// Advance frames again.
    Resume,
    /// The overlay's window handle, so keys typed into it count as ours.
    Overlay(isize),
    Quit,
}

/// What the game thread tells the client.
#[derive(Debug)]
pub enum Report {
    /// The game window is up. Sent once, before the first frame.
    Window { hwnd: isize, width: usize, height: usize, title: String, console: String },
    Died { frame: u64 },
    Cleared { frame: u64 },
    Exited { frame: u64 },
    /// The pad bits the game was fed on a frame they were non-zero, with
    /// whether the overlay was in front at the time. Rate-limited; for the
    /// spike's log.
    Input { bits: u32, overlay_in_front: bool },
    Closed,
}

pub struct Config {
    pub manifest: std::path::PathBuf,
    pub stage: Option<String>,
    pub scale: usize,
}

pub fn run(cfg: Config, cmds: Receiver<Command>, reports: Sender<Report>) -> Result<(), String> {
    let manifest = Manifest::load(&cfg.manifest)?;
    let stage = match &cfg.stage {
        Some(id) => manifest
            .stage(id)
            .ok_or_else(|| format!("{} declares no stage `{id}`", manifest.title))?
            .clone(),
        None => manifest.stages.first().ok_or("the manifest declares no stage")?.clone(),
    };
    let content = prep::content_for(&manifest, &stage, None, None, &Default::default())?;
    let spec = manifest.stage_spec(&stage, content.key.clone());
    let controller = manifest.libretro.controller;
    let title = content.title.clone();
    let console = manifest.console.to_uppercase();
    let mut runner = LibretroRunner::start(manifest, None, content, Vec::new())?;
    let av = runner.av_info();
    let (width, height) = (av.geometry.base_width as usize, av.geometry.base_height as usize);

    let audio = match audio::start(av.timing.sample_rate) {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("[audio] disabled: {e}");
            None
        }
    };

    let mut window = Window::new(
        &format!("{title} — GamerTrials"),
        width.max(1),
        height.max(1),
        WindowOptions {
            scale: match cfg.scale {
                4 => Scale::X4,
                1 => Scale::X1,
                _ => Scale::X2,
            },
            scale_mode: ScaleMode::AspectRatioStretch,
            resize: true,
            ..WindowOptions::default()
        },
    )
    .map_err(|e| format!("cannot open window: {e}"))?;
    window.set_target_fps(0);
    // The window is held through the intro and countdown before the core has
    // drawn anything, and an unpainted window is white; the overlay is
    // transparent, so that white is what the player would see under it.
    window
        .update_with_buffer(&vec![0u32; width.max(1) * height.max(1)], width.max(1), height.max(1))
        .map_err(|e| format!("cannot clear window: {e}"))?;
    let hwnd = window.get_window_handle() as isize;
    let _ = reports.send(Report::Window { hwnd, width, height, title, console });

    let mut pads = match controller {
        Controller::Pad(layout) => Some(input::Pads::new(layout)),
        Controller::Keyboard => None,
    };

    runner.enter(&spec).map_err(|e| e.to_string())?;

    let frame_time = Duration::from_secs_f64(1.0 / av.timing.fps.max(1.0));
    let audio_target = audio
        .as_ref()
        .map(|a| (audio::TARGET_QUEUED_FRAMES as f64 * a.device_rate / av.timing.fps) as usize);
    let mut next_deadline = Instant::now();
    let mut last_seq = 0u64;
    let mut held = true;
    let mut overlay = None;
    let mut ours = vec![hwnd];
    let mut last_input_report = Instant::now() - Duration::from_secs(1);

    while window.is_open() && !keys::quit_pressed() {
        if runner.shutdown_requested() {
            break;
        }
        loop {
            match cmds.try_recv() {
                Ok(Command::Hold) => held = true,
                Ok(Command::Resume) => held = false,
                Ok(Command::Overlay(h)) => {
                    overlay = Some(h);
                    ours = vec![hwnd, h];
                }
                Ok(Command::Quit) | Err(TryRecvError::Disconnected) => {
                    let _ = reports.send(Report::Closed);
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }

        if held {
            // Present whatever is there, pump the window, and wait a frame.
            window.update();
            std::thread::sleep(frame_time);
            continue;
        }

        match audio_target {
            Some(target) => {
                let give_up_at = Instant::now() + frame_time * 4;
                while audio::queued_frames() > target && Instant::now() < give_up_at {
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            None => {
                let now = Instant::now();
                if next_deadline > now {
                    std::thread::sleep(next_deadline - now);
                } else {
                    next_deadline = now;
                }
                next_deadline += frame_time;
            }
        }
        window.update();

        let in_front = keys::ours_in_front(&ours);
        match pads.as_mut() {
            Some(p) => {
                p.pump();
                let bits = if in_front { p.state() | keys::joypad() } else { 0 };
                libretro::JOYPAD.store(bits, Ordering::Relaxed);
                if bits != 0 && last_input_report.elapsed() > Duration::from_millis(250) {
                    last_input_report = Instant::now();
                    let overlay_in_front = overlay
                        .is_some_and(|h| unsafe { winapi::um::winuser::GetForegroundWindow() } as isize == h);
                    let _ = reports.send(Report::Input { bits, overlay_in_front });
                }
            }
            None => {
                if in_front {
                    keys::keyboard_passthrough();
                } else {
                    keys::keyboard_release_all();
                }
            }
        }

        let events = runner.advance().map_err(|e| e.to_string())?.to_vec();
        for event in &events {
            let report = match event {
                SessionEvent::Died { frame } => Report::Died { frame: *frame },
                SessionEvent::StageCleared { frame } => Report::Cleared { frame: *frame },
                SessionEvent::StageExited { frame, .. } => Report::Exited { frame: *frame },
                _ => continue,
            };
            let _ = reports.send(report);
        }

        let mut painted = false;
        let mut paint_error = None;
        last_seq = runner.present(last_seq, &mut |f| {
            painted = true;
            if let Err(e) = window.update_with_buffer(f.pixels, f.width, f.height) {
                paint_error = Some(e.to_string());
            }
        });
        if let Some(e) = paint_error {
            return Err(format!("cannot present frame: {e}"));
        }
        if !painted {
            window.update();
        }
    }
    let _ = reports.send(Report::Closed);
    Ok(())
}

/// `manifests/<name>.toml` beside the workspace, or a path as given.
pub fn manifest_path(game: &str) -> std::path::PathBuf {
    let as_path = Path::new(game);
    if as_path.exists() {
        return as_path.to_path_buf();
    }
    Path::new("manifests").join(format!("{game}.toml"))
}
