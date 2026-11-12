// A GUI-subsystem binary: no console window flashes up when Krab is launched
// from a hotkey or the Start menu.
#![cfg_attr(windows, windows_subsystem = "windows")]

use krab::app::{self, Launch, UserEvent};
use krab::cli::{self, Args, Command};
use krab::config::Config;
use krab::ipc::{self, Message, Request};
use krab::output::{self, Image};
use krab::{capture, platform, trace};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;

fn main() -> ExitCode {
    trace::init();
    platform::init_process();

    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(e) => {
            platform::attach_console();
            eprintln!("krab: {e}");
            return ExitCode::from(2);
        }
    };
    if args.trace {
        platform::attach_console();
        trace::enable();
    }
    trace::mark("started");

    let result = match args.command.clone() {
        Command::Help => {
            platform::attach_console();
            println!("{}", cli::HELP);
            Ok(())
        }
        Command::Version => {
            platform::attach_console();
            println!("krab {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        Command::HoldClipboard { kind, path } => hold_clipboard(&kind, &path),
        Command::Quit => {
            platform::attach_console();
            match ipc::send(&Message::Quit) {
                Ok(()) => println!("Krab stopped."),
                Err(_) => println!("Krab is not running in the background."),
            }
            Ok(())
        }
        Command::OpenConfig => {
            platform::attach_console();
            Config::ensure_file()
                .map_err(|e| e.to_string())
                .map(|path| {
                    println!("{}", path.display());
                    output::open_path(&path);
                })
        }
        Command::Autostart(on) => {
            platform::attach_console();
            platform::autostart::set(on)
                .map_err(|e| e.to_string())
                .map(|()| {
                    println!("Start with system: {}", if on { "on" } else { "off" });
                })
        }
        Command::Daemon => {
            if ipc::send(&Message::Ping).is_ok() {
                platform::attach_console();
                println!("Krab is already running in the background.");
                Ok(())
            } else {
                app::run(
                    Config::load(),
                    Launch {
                        daemon: true,
                        pending_capture: None,
                        initial: None,
                        output: None,
                        quiet: args.quiet,
                        exit_after: args.exit_after_ms.map(Duration::from_millis),
                    },
                )
            }
        }
        Command::Snip(req) => snip(&args, req),
    };

    trace::flush();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            platform::attach_console();
            eprintln!("krab: {e}");
            ExitCode::FAILURE
        }
    }
}

fn snip(args: &Args, req: Request) -> Result<(), String> {
    if args.now {
        return capture_now(args);
    }
    // If Krab already runs in the background, let it do the work: its
    // process is warm, so this path is the fastest of all.
    let handoff = !args.standalone && args.output.is_none() && args.exit_after_ms.is_none();
    if handoff {
        platform::allow_foreground_handoff();
        if ipc::send(&Message::Snip(req.clone())).is_ok() {
            trace::mark("handed to the running instance");
            return Ok(());
        }
    }

    // Cold start: grab the screen on a worker thread right away, while the
    // main thread loads the config and brings up the event loop in parallel.
    let (pending_capture, initial) = if req.delay == 0 {
        let (tx, rx) = mpsc::channel();
        let req = req.clone();
        std::thread::spawn(move || {
            let result = capture::capture_all();
            trace::mark("screen captured");
            if let Ok(proxy) = rx.recv() {
                let proxy: winit::event_loop::EventLoopProxy<UserEvent> = proxy;
                let _ = proxy.send_event(UserEvent::Captured(Box::new(result), req));
            }
        });
        (Some(tx), None)
    } else {
        (None, Some(req))
    };

    app::run(
        Config::load(),
        Launch {
            daemon: false,
            pending_capture,
            initial,
            output: args.output.clone(),
            quiet: true,
            exit_after: args.exit_after_ms.map(Duration::from_millis),
        },
    )
}

/// `krab --now`: full-screen capture without any UI.
fn capture_now(args: &Args) -> Result<(), String> {
    let cfg = Config::load();
    let cap = capture::capture_all()?;
    let cursor = cap.cursor;
    let shot = cap
        .shots
        .iter()
        .find(|s| cursor.is_some_and(|(x, y)| s.native.contains(x, y)))
        .or_else(|| cap.shots.iter().find(|s| s.primary))
        .or(cap.shots.first())
        .ok_or("no monitors found")?;
    let img = Image {
        w: shot.w,
        h: shot.h,
        rgba: krab::gfx::argb_to_rgba(&shot.px),
    };
    if cfg.copy_to_clipboard {
        output::copy_image(&img)?;
    }
    let saved = match &args.output {
        Some(path) => Some(img.save(path, cfg.jpeg_quality).map(|_| path.clone())?),
        None if cfg.auto_save => Some(output::auto_save(&img, &cfg)?),
        None => None,
    };
    if let Some(path) = saved {
        platform::attach_console();
        println!("{}", path.display());
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn hold_clipboard(kind: &str, path: &std::path::Path) -> Result<(), String> {
    output::hold_clipboard(kind, path)
}

#[cfg(not(target_os = "linux"))]
fn hold_clipboard(_kind: &str, _path: &std::path::Path) -> Result<(), String> {
    Ok(())
}
