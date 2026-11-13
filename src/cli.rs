//! Command line parsing. Hand-rolled: it runs on every launch, before the
//! first pixel is captured, so it stays allocation-light and dependency-free.

use crate::ipc::Request;
use crate::overlay::{Kind, Mode};
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Take a snip (the default).
    Snip(Request),
    /// Stay in the background with a tray icon and global hotkey.
    Daemon,
    Quit,
    OpenConfig,
    /// Open the settings window (starting the background instance if needed).
    Settings,
    Autostart(bool),
    /// Check for a new version and install it.
    Update,
    /// Install as a desktop application.
    Install,
    /// Remove the installed application.
    Uninstall,
    Help,
    Version,
    /// Internal: keep clipboard contents alive on Linux.
    HoldClipboard {
        kind: String,
        path: PathBuf,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Args {
    pub command: Command,
    /// Save the result to this file.
    pub output: Option<PathBuf>,
    /// Capture the whole screen immediately, without the overlay.
    pub now: bool,
    /// Never hand the request to an already running instance.
    pub standalone: bool,
    /// Suppress the "Krab is running" toast when starting the daemon.
    pub quiet: bool,
    /// Print startup timings to stderr.
    pub trace: bool,
    /// Testing aid: close the overlay automatically after this many ms.
    pub exit_after_ms: Option<u64>,
    /// `--update` started automatically: no output, no restart prompts.
    pub background: bool,
    /// This daemon replaces one that just installed an update.
    pub updated: bool,
    /// This daemon was just installed as an application.
    pub installed: bool,
}

pub const HELP: &str = "\
Krab - a lightweight snipping tool that opens in milliseconds

USAGE:
    krab [MODE] [OPTIONS]      take a snip
    krab --daemon              run in the background (tray icon + global hotkey)

MODES:
    rect          drag a rectangle (default)
    window        click a window
    full          the whole screen
    freeform      draw any shape

OPTIONS:
    -d, --delay <SECONDS>      wait before capturing (menus, hover states...)
    -r, --record               record the screen instead of taking a picture
    -t, --text                 copy the text in the selected area (OCR)
        --scroll               scroll the selected area and stitch one tall image
    -c, --copy                 copy as soon as the area is selected
    -e, --edit                 always open the markup tools after selecting
    -o, --output <FILE>        also save the result to FILE (png, jpg, bmp, gif)
        --now                  capture the full screen right now, no overlay
        --standalone           do not reuse the background instance

BACKGROUND INSTANCE:
        --daemon               start it
        --quit                 stop it
        --autostart <on|off>   start Krab when you log in
        --settings             open the settings window
        --config               open the configuration file
        --update               install the newest version now
        --install              install Krab as an app (Start menu, Applications, app menu)
        --uninstall            remove the installed app

    -h, --help                 show this help
    -V, --version              show the version
";

pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Args, String> {
    let mut req = Request::default();
    let mut command: Option<Command> = None;
    let mut out = Args {
        command: Command::Help,
        output: None,
        now: false,
        standalone: false,
        quiet: false,
        trace: false,
        exit_after_ms: None,
        background: false,
        updated: false,
        installed: false,
    };
    let mut it = args.into_iter();
    let value = |it: &mut dyn Iterator<Item = String>, flag: &str| -> Result<String, String> {
        it.next().ok_or_else(|| format!("{flag} needs a value"))
    };
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "-h" | "--help" | "help" => command = Some(Command::Help),
            "-V" | "--version" | "version" => command = Some(Command::Version),
            "--daemon" | "daemon" | "--tray" | "tray" => command = Some(Command::Daemon),
            "--quit" | "quit" => command = Some(Command::Quit),
            "--config" | "config" => command = Some(Command::OpenConfig),
            "--settings" | "settings" => command = Some(Command::Settings),
            "--update" | "update" => command = Some(Command::Update),
            "--background" => out.background = true,
            "--updated" => out.updated = true,
            "--installed" => out.installed = true,
            "--install" | "install" => command = Some(Command::Install),
            "--uninstall" | "uninstall" => command = Some(Command::Uninstall),
            "--autostart" => {
                let v = value(&mut it, "--autostart")?;
                command = Some(Command::Autostart(match v.to_ascii_lowercase().as_str() {
                    "on" | "true" | "1" | "yes" => true,
                    "off" | "false" | "0" | "no" => false,
                    _ => return Err(format!("--autostart expects on or off, got '{v}'")),
                }));
            }
            "--hold-clipboard" => {
                let kind = value(&mut it, "--hold-clipboard")?;
                let path = PathBuf::from(value(&mut it, "--hold-clipboard")?);
                command = Some(Command::HoldClipboard { kind, path });
            }
            "-m" | "--mode" => {
                let v = value(&mut it, "--mode")?;
                req.mode = Some(Mode::parse(&v).ok_or_else(|| format!("unknown mode '{v}'"))?);
            }
            "-d" | "--delay" => {
                let v = value(&mut it, "--delay")?;
                req.delay = v
                    .parse::<u32>()
                    .map_err(|_| format!("--delay expects seconds, got '{v}'"))?
                    .min(3600);
            }
            "-r" | "--record" | "record" => req.kind = Kind::Record,
            "-t" | "--text" | "text" => req.text = true,
            "--scroll" | "scroll" => req.scroll = true,
            "-c" | "--copy" => req.quick = Some(true),
            "-e" | "--edit" => req.quick = Some(false),
            "-o" | "--output" => out.output = Some(PathBuf::from(value(&mut it, "--output")?)),
            "--now" => out.now = true,
            "--standalone" => out.standalone = true,
            "--quiet" => out.quiet = true,
            "--trace" => out.trace = true,
            "--exit-after-ms" => {
                let v = value(&mut it, "--exit-after-ms")?;
                out.exit_after_ms = Some(
                    v.parse()
                        .map_err(|_| "--exit-after-ms expects a number".to_string())?,
                );
            }
            other => match Mode::parse(other) {
                Some(m) if !other.starts_with('-') => req.mode = Some(m),
                _ => return Err(format!("unknown argument '{other}' (try --help)")),
            },
        }
    }
    if out.now {
        req.mode = Some(Mode::Fullscreen);
    }
    if req.kind == Kind::Record && req.mode == Some(Mode::Freeform) {
        return Err("freeform areas cannot be recorded".into());
    }
    out.command = command.unwrap_or(Command::Snip(req));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> Result<Args, String> {
        parse(s.split_whitespace().map(String::from))
    }

    fn snip(s: &str) -> Request {
        match p(s).unwrap().command {
            Command::Snip(r) => r,
            other => panic!("expected snip, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_means_default_snip() {
        assert_eq!(snip(""), Request::default());
    }

    #[test]
    fn modes_and_flags_combine() {
        let r = snip("window --delay 5 --copy");
        assert_eq!(
            (r.mode, r.delay, r.quick),
            (Some(Mode::Window), 5, Some(true))
        );
        let r = snip("-m full -r");
        assert_eq!((r.mode, r.kind), (Some(Mode::Fullscreen), Kind::Record));
        let a = p("rect -o shot.png --standalone").unwrap();
        assert_eq!(a.output, Some(PathBuf::from("shot.png")));
        assert!(a.standalone);
    }

    #[test]
    fn now_implies_fullscreen() {
        assert_eq!(snip("--now").mode, Some(Mode::Fullscreen));
        assert!(p("--now").unwrap().now);
    }

    #[test]
    fn subcommands() {
        assert_eq!(p("--daemon").unwrap().command, Command::Daemon);
        assert_eq!(p("--quit").unwrap().command, Command::Quit);
        assert_eq!(
            p("--autostart on").unwrap().command,
            Command::Autostart(true)
        );
        assert_eq!(
            p("--autostart off").unwrap().command,
            Command::Autostart(false)
        );
        assert_eq!(p("-V").unwrap().command, Command::Version);
        assert_eq!(p("--update").unwrap().command, Command::Update);
        assert_eq!(p("--settings").unwrap().command, Command::Settings);
        let a = p("--daemon --updated").unwrap();
        assert!(a.updated && a.command == Command::Daemon);
    }

    #[test]
    fn mistakes_are_reported() {
        assert!(p("--delay").is_err());
        assert!(p("--delay soon").is_err());
        assert!(p("--mode sideways").is_err());
        assert!(p("--frobnicate").is_err());
        assert!(p("--autostart maybe").is_err());
        assert!(p("freeform --record").is_err());
    }
}
