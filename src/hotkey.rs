//! Global keyboard shortcuts: registering them, telling the user which ones
//! work, and turning a key press in the settings window into a shortcut.

use crate::config::Config;
use global_hotkey::hotkey::HotKey;
use global_hotkey::GlobalHotKeyManager;
use std::str::FromStr;
use winit::keyboard::KeyCode;

/// The shortcuts Krab offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Snip,
    SnipAlt,
    Record,
}

impl Slot {
    pub const ALL: [Slot; 3] = [Slot::Snip, Slot::SnipAlt, Slot::Record];

    pub fn label(self) -> &'static str {
        match self {
            Slot::Snip => "New snip",
            Slot::SnipAlt => "New snip (second shortcut)",
            Slot::Record => "Record screen",
        }
    }

    pub fn get(self, cfg: &Config) -> &str {
        match self {
            Slot::Snip => &cfg.hotkey,
            Slot::SnipAlt => &cfg.hotkey_alt,
            Slot::Record => &cfg.hotkey_record,
        }
    }

    pub fn set(self, cfg: &mut Config, spec: String) {
        match self {
            Slot::Snip => cfg.hotkey = spec,
            Slot::SnipAlt => cfg.hotkey_alt = spec,
            Slot::Record => cfg.hotkey_record = spec,
        }
    }
}

/// What happened when a shortcut was registered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// No shortcut set.
    Off,
    /// Registered and working.
    Active,
    /// Another program already owns this combination.
    Taken,
    /// The same combination is used by another Krab shortcut above.
    Duplicate,
    /// The text could not be understood.
    Invalid,
    /// This desktop does not let applications register shortcuts.
    Unsupported,
}

impl Status {
    pub fn message(&self) -> &'static str {
        match self {
            Status::Off => "Not set",
            Status::Active => "Active",
            Status::Taken => "In use by another app",
            Status::Duplicate => "Already used above",
            Status::Invalid => "Not a valid shortcut",
            Status::Unsupported => "Set it in your desktop's keyboard settings",
        }
    }

    pub fn is_problem(&self) -> bool {
        matches!(self, Status::Taken | Status::Duplicate | Status::Invalid)
    }
}

/// Whether this session lets applications register global shortcuts.
/// Wayland deliberately does not; X11, Windows and macOS do.
pub fn supported() -> bool {
    if cfg!(target_os = "linux") {
        std::env::var_os("WAYLAND_DISPLAY").is_none_or(|v| v.is_empty())
            && std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty())
    } else {
        true
    }
}

/// Registered shortcuts of the background instance.
pub struct Hotkeys {
    manager: Option<GlobalHotKeyManager>,
    registered: Vec<(Slot, HotKey)>,
}

impl Default for Hotkeys {
    fn default() -> Self {
        Self::new()
    }
}

impl Hotkeys {
    pub fn new() -> Hotkeys {
        Hotkeys {
            manager: if supported() {
                GlobalHotKeyManager::new().ok()
            } else {
                None
            },
            registered: Vec::new(),
        }
    }

    /// Release every shortcut (while the settings window records a new one,
    /// so pressing a current shortcut does not start a snip).
    pub fn suspend(&mut self) {
        if let Some(m) = &self.manager {
            for (_, hk) in self.registered.drain(..) {
                let _ = m.unregister(hk);
            }
        }
        self.registered.clear();
    }

    /// Register the shortcuts from `cfg`, replacing the current ones.
    pub fn apply(&mut self, cfg: &Config) -> [Status; 3] {
        self.suspend();
        let mut out = [Status::Off, Status::Off, Status::Off];
        let mut seen: Vec<HotKey> = Vec::new();
        for (i, slot) in Slot::ALL.iter().enumerate() {
            let spec = slot.get(cfg).trim();
            if spec.is_empty() {
                continue;
            }
            let Ok(hk) = HotKey::from_str(spec) else {
                out[i] = Status::Invalid;
                continue;
            };
            if seen.contains(&hk) {
                out[i] = Status::Duplicate;
                continue;
            }
            seen.push(hk);
            out[i] = match &self.manager {
                None => Status::Unsupported,
                Some(m) => match m.register(hk) {
                    Ok(()) => {
                        self.registered.push((*slot, hk));
                        Status::Active
                    }
                    Err(_) => Status::Taken,
                },
            };
        }
        out
    }

    /// Which shortcut a hotkey event belongs to.
    pub fn slot(&self, id: u32) -> Option<Slot> {
        self.registered
            .iter()
            .find(|(_, hk)| hk.id() == id)
            .map(|(s, _)| *s)
    }
}

/// Modifier keys held while a shortcut is being recorded.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub meta: bool,
}

impl Mods {
    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.meta
    }

    fn prefix(self) -> String {
        let mut s = String::new();
        for (on, name) in [
            (self.ctrl, "Ctrl"),
            (self.alt, "Alt"),
            (self.shift, "Shift"),
            (
                self.meta,
                if cfg!(target_os = "macos") {
                    "Cmd"
                } else {
                    "Super"
                },
            ),
        ] {
            if on {
                s.push_str(name);
                s.push('+');
            }
        }
        s
    }
}

fn is_modifier(code: KeyCode) -> bool {
    use KeyCode::*;
    matches!(
        code,
        ShiftLeft
            | ShiftRight
            | ControlLeft
            | ControlRight
            | AltLeft
            | AltRight
            | SuperLeft
            | SuperRight
            | Meta
            | Hyper
            | Fn
            | FnLock
            | CapsLock
            | NumLock
    )
}

/// Keys that make sense as a shortcut on their own, without a modifier.
fn works_alone(code: KeyCode) -> bool {
    use KeyCode::*;
    matches!(
        code,
        PrintScreen
            | Pause
            | ScrollLock
            | Insert
            | F1
            | F2
            | F3
            | F4
            | F5
            | F6
            | F7
            | F8
            | F9
            | F10
            | F11
            | F12
            | F13
            | F14
            | F15
            | F16
            | F17
            | F18
            | F19
            | F20
            | F21
            | F22
            | F23
            | F24
    )
}

/// What pressing `code` with `mods` means while recording a shortcut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recorded {
    /// Only modifiers so far; show them and keep waiting.
    Pending(String),
    /// A complete shortcut, in config notation.
    Done(String),
    /// Not usable; the message says why.
    Rejected(&'static str),
}

pub fn record(code: KeyCode, mods: Mods) -> Recorded {
    if is_modifier(code) {
        return Recorded::Pending(mods.prefix());
    }
    if !mods.any() && !works_alone(code) {
        return Recorded::Rejected("Add Ctrl, Alt, Shift or a system key");
    }
    let name = format!("{code:?}");
    let key = name
        .strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .filter(|k| k.len() == 1)
        .unwrap_or(&name);
    let spec = format!("{}{key}", mods.prefix());
    match HotKey::from_str(&spec) {
        Ok(_) => Recorded::Done(spec),
        Err(_) => Recorded::Rejected("This key cannot be used"),
    }
}

/// A shortcut as the user reads it: "Alt + Shift + S", "Print Screen".
pub fn display(spec: &str) -> String {
    let spec = spec.trim();
    if spec.is_empty() {
        return String::new();
    }
    spec.split('+')
        .map(|part| {
            let p = part.trim();
            match p.to_ascii_lowercase().as_str() {
                "super" | "cmd" | "command" | "meta" | "win" => {
                    if cfg!(target_os = "macos") {
                        "Cmd"
                    } else if cfg!(windows) {
                        "Win"
                    } else {
                        "Super"
                    }
                }
                .to_string(),
                "ctrl" | "control" => "Ctrl".into(),
                "alt" | "option" => {
                    if cfg!(target_os = "macos") {
                        "Option".into()
                    } else {
                        "Alt".into()
                    }
                }
                "shift" => "Shift".into(),
                "printscreen" => "Print Screen".into(),
                "pageup" => "Page Up".into(),
                "pagedown" => "Page Down".into(),
                "scrolllock" => "Scroll Lock".into(),
                _ => {
                    let p = p.strip_prefix("Key").filter(|k| k.len() == 1).unwrap_or(p);
                    let p = p
                        .strip_prefix("Digit")
                        .filter(|k| k.len() == 1)
                        .unwrap_or(p);
                    let p = p.strip_prefix("Arrow").unwrap_or(p);
                    p.to_string()
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(ctrl: bool, alt: bool, shift: bool) -> Mods {
        Mods {
            ctrl,
            alt,
            shift,
            meta: false,
        }
    }

    #[test]
    fn recording_builds_config_notation() {
        assert_eq!(
            record(KeyCode::KeyS, mods(false, true, true)),
            Recorded::Done("Alt+Shift+S".into())
        );
        assert_eq!(
            record(KeyCode::Digit4, mods(true, false, true)),
            Recorded::Done("Ctrl+Shift+4".into())
        );
        assert_eq!(
            record(KeyCode::PrintScreen, Mods::default()),
            Recorded::Done("PrintScreen".into())
        );
        assert_eq!(
            record(KeyCode::F9, mods(true, false, false)),
            Recorded::Done("Ctrl+F9".into())
        );
    }

    #[test]
    fn plain_letters_and_bare_modifiers_are_not_shortcuts() {
        assert!(matches!(
            record(KeyCode::KeyA, Mods::default()),
            Recorded::Rejected(_)
        ));
        assert_eq!(
            record(KeyCode::ShiftLeft, mods(true, false, true)),
            Recorded::Pending("Ctrl+Shift+".into())
        );
    }

    #[test]
    fn every_recorded_shortcut_parses() {
        for code in [
            KeyCode::KeyZ,
            KeyCode::Digit0,
            KeyCode::Space,
            KeyCode::ArrowUp,
            KeyCode::Minus,
            KeyCode::F12,
        ] {
            if let Recorded::Done(spec) = record(code, mods(true, true, false)) {
                assert!(HotKey::from_str(&spec).is_ok(), "{spec}");
            }
        }
    }

    #[test]
    fn display_is_readable() {
        assert_eq!(display("Alt+Shift+S"), {
            if cfg!(target_os = "macos") {
                "Option + Shift + S"
            } else {
                "Alt + Shift + S"
            }
        });
        assert_eq!(display("PrintScreen"), "Print Screen");
        assert_eq!(display("Ctrl+Shift+KeyX"), "Ctrl + Shift + X");
        assert_eq!(display(""), "");
    }

    #[test]
    fn defaults_parse_and_duplicates_are_reported() {
        let cfg = Config::default();
        for slot in Slot::ALL {
            let spec = slot.get(&cfg);
            assert!(spec.is_empty() || HotKey::from_str(spec).is_ok(), "{spec}");
        }
        // Without a manager nothing is registered, but duplicates and typos
        // are still found.
        let mut hk = Hotkeys {
            manager: None,
            registered: Vec::new(),
        };
        let cfg = Config {
            hotkey: "Alt+Shift+S".into(),
            hotkey_alt: "alt+shift+s".into(),
            hotkey_record: "Shift+Nonsense".into(),
            ..Config::default()
        };
        let st = hk.apply(&cfg);
        assert_eq!(st[0], Status::Unsupported);
        assert_eq!(st[1], Status::Duplicate);
        assert_eq!(st[2], Status::Invalid);
    }
}
