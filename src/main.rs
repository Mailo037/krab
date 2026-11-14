// A GUI-subsystem binary: no console window flashes up when Krab is launched
// from a hotkey or the Start menu.
#![cfg_attr(windows, windows_subsystem = "windows")]

use krab::app::{self, Launch, UserEvent};
use krab::cli::{self, Args, Command};
use krab::config::Config;
use krab::install;
use krab::ipc::{self, Message, Request};
use krab::output::{self, Image};
use krab::update::{self, Outcome};
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
    // Started by double-click / from a file manager, without arguments.
    let plain_launch = std::env::args_os().len() == 1;

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
        Command::Update => run_update(&args),
        Command::Install => {
            platform::attach_console();
            install_and_start().map(|exe| println!("Installed to {}", exe.display()))
        }
        Command::Uninstall => uninstall_app(),
        Command::Daemon => {
            if args.updated {
                // The instance that installed the update is shutting down;
                // wait until it has let go of the single-instance socket.
                let deadline = std::time::Instant::now() + Duration::from_secs(15);
                while ipc::send(&Message::Ping).is_ok() && std::time::Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
            if ipc::send(&Message::Ping).is_ok() {
                platform::attach_console();
                println!("Krab is already running in the background.");
                Ok(())
            } else {
                run_app(
                    Config::load(),
                    Launch {
                        daemon: true,
                        pending_capture: None,
                        initial: None,
                        output: None,
                        quiet: args.quiet,
                        updated: args.updated,
                        installed: args.installed,
                        open_settings: false,
                        exit_after: args.exit_after_ms.map(Duration::from_millis),
                    },
                )
            }
        }
        Command::Settings => {
            if ipc::send(&Message::Settings).is_ok() {
                Ok(())
            } else {
                run_app(
                    Config::load(),
                    Launch {
                        daemon: true,
                        pending_capture: None,
                        initial: None,
                        output: None,
                        quiet: true,
                        updated: false,
                        installed: false,
                        open_settings: true,
                        exit_after: None,
                    },
                )
            }
        }
        Command::Snip(req) => {
            // The downloaded program installs itself on its first plain launch.
            if plain_launch && install::should_install() {
                match install_and_start() {
                    Ok(_) => {
                        trace::flush();
                        return ExitCode::SUCCESS;
                    }
                    Err(e) => eprintln!("krab: could not install: {e}"),
                }
            }
            snip(&args, req)
        }
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
        return capture_now(args, req.active);
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
            let mut result = capture::capture_all(req.active);
            trace::mark("screen captured");
            // The main thread loads the config too; this copy only decides
            // whether to render the covered windows before the overlay is up.
            if req.kind == krab::overlay::Kind::Snip && Config::load().precapture_windows {
                result = result.map(capture::Capture::precapture);
            }
            if let Ok(proxy) = rx.recv() {
                let proxy: winit::event_loop::EventLoopProxy<UserEvent> = proxy;
                let _ = proxy.send_event(UserEvent::Captured(Box::new(result), req));
            }
        });
        (Some(tx), None)
    } else {
        (None, Some(req))
    };

    let cfg = Config::load();
    // An interactive snip with nothing running in the background becomes the
    // background instance, so Krab shows up in the tray and the shortcuts work.
    let stay = cfg.run_in_background
        && !args.standalone
        && args.output.is_none()
        && args.exit_after_ms.is_none();
    let auto_update = cfg.auto_update;
    run_app(
        cfg,
        Launch {
            daemon: stay,
            pending_capture,
            initial,
            output: args.output.clone(),
            quiet: true,
            updated: false,
            installed: false,
            open_settings: false,
            exit_after: args.exit_after_ms.map(Duration::from_millis),
        },
    )?;
    // Without a background instance nobody else looks for updates. The check
    // runs in its own process once a day, after the snip is done.
    if !stay && args.exit_after_ms.is_none() && update::due() && auto_update {
        update::mark_checked();
        update::spawn_background_check();
    }
    Ok(())
}

/// Stop a running background instance and wait until it is gone.
fn stop_daemon() {
    if ipc::send(&Message::Quit).is_ok() {
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while ipc::send(&Message::Ping).is_ok() && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

/// Install this program as an app and start the installed copy.
fn install_and_start() -> Result<std::path::PathBuf, String> {
    stop_daemon();
    let exe = install::install()?;
    let mut cmd = std::process::Command::new(&exe);
    cmd.args(["--daemon", "--installed"])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    platform::hide_console(&mut cmd);
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(exe)
}

/// `krab --uninstall`, also what Settings > Apps runs on Windows.
fn uninstall_app() -> Result<(), String> {
    use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
    let answer = MessageDialog::new()
        .set_title("Uninstall Krab")
        .set_description(
            "Remove Krab from this computer?\n\nYour settings and screenshots are kept.",
        )
        .set_level(MessageLevel::Info)
        .set_buttons(MessageButtons::YesNo)
        .show();
    if answer != MessageDialogResult::Yes {
        return Ok(());
    }
    stop_daemon();
    let result = install::uninstall();
    let (level, text) = match &result {
        Ok(()) => (MessageLevel::Info, "Krab was removed.".to_string()),
        Err(e) => (
            MessageLevel::Error,
            format!("Krab could not be removed completely:\n{e}"),
        ),
    };
    MessageDialog::new()
        .set_title("Uninstall Krab")
        .set_description(text)
        .set_level(level)
        .set_buttons(MessageButtons::Ok)
        .show();
    result
}

/// Run the event loop; if it stopped to make way for an update, start the
/// new version as the background instance.
fn run_app(cfg: Config, launch: Launch) -> Result<(), String> {
    app::run(cfg, launch)?;
    match update::restart_requested() {
        Some(exe) => update::spawn_daemon(&exe),
        None => Ok(()),
    }
}

/// `krab --update`, typed by the user or started by [`snip`].
fn run_update(args: &Args) -> Result<(), String> {
    if args.background {
        update::cleanup();
        // A failed automatic check is retried tomorrow; nothing to report.
        if let Ok(Outcome::Updated { exe, .. }) = update::check_and_install() {
            // A background instance that started in the meantime keeps
            // running the old version until it restarts; ask it to.
            restart_running_daemon(&exe);
        }
        return Ok(());
    }
    platform::attach_console();
    println!("Checking for updates...");
    update::mark_checked();
    match update::check_and_install()? {
        Outcome::UpToDate(v) => println!("Krab {v} is the newest version."),
        Outcome::Updated { from, to, exe } => {
            println!("Updated Krab {from} -> {to}.");
            if restart_running_daemon(&exe) {
                println!("A running Krab from this location restarts into the new version.");
            }
        }
    }
    Ok(())
}

/// Tell a running background instance that `exe` was updated. It restarts
/// into the new version only if it runs from that same file - a Krab that
/// was started from somewhere else is left alone.
fn restart_running_daemon(exe: &std::path::Path) -> bool {
    ipc::send(&Message::Updated(exe.display().to_string())).is_ok()
}

/// `krab --now`: full-screen capture without any UI; with `--active`, the
/// window that has the focus.
fn capture_now(args: &Args, active: bool) -> Result<(), String> {
    let cfg = Config::load();
    let cap = capture::capture_all(active)?;
    let img = if active {
        active_window_image(cap, &cfg)?
    } else {
        let cursor = cap.cursor;
        let shot = cap
            .shots
            .iter()
            .find(|s| cursor.is_some_and(|(x, y)| s.native.contains(x, y)))
            .or_else(|| cap.shots.iter().find(|s| s.primary))
            .or(cap.shots.first())
            .ok_or("no monitors found")?;
        Image {
            w: shot.w,
            h: shot.h,
            rgba: krab::gfx::argb_to_rgba(&shot.px),
        }
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

/// The active window as a window snip would take it - without the windows
/// on top of it, with the configured border or background - through the
/// overlay, which is never shown.
fn active_window_image(cap: capture::Capture, cfg: &Config) -> Result<Image, String> {
    use krab::overlay::{Kind, Mode, Options, Overlay, Shared};
    let id = cap.active.ok_or("no active window found")?;
    let rect = cap
        .windows
        .iter()
        .find(|w| w.id == id)
        .map(|w| w.rect)
        .ok_or("no active window found")?;
    let windows = cap.windows;
    let shot = cap
        .shots
        .into_iter()
        .max_by_key(|s| s.native.intersect(&rect).area())
        .ok_or("no monitors found")?;
    let shared = Shared {
        mode: Mode::Window,
        kind: Kind::Snip,
        delay: cfg.delay_seconds,
    };
    let opts = Options {
        quick_copy: false,
        ..Options::from_config(cfg)
    };
    let mut overlay = Overlay::new(shot, &windows, opts, shared);
    if !overlay.select_window_id(id) {
        return Err("the active window is not on any screen".into());
    }
    overlay
        .compose()
        .ok_or_else(|| "could not render the window".into())
}

#[cfg(target_os = "linux")]
fn hold_clipboard(kind: &str, path: &std::path::Path) -> Result<(), String> {
    output::hold_clipboard(kind, path)
}

#[cfg(not(target_os = "linux"))]
fn hold_clipboard(_kind: &str, _path: &std::path::Path) -> Result<(), String> {
    Ok(())
}
