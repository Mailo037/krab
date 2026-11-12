//! User configuration (`config.toml`).

use crate::gfx::{theme, Color};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    /// Global hotkey that starts a snip while Krab runs in the background.
    pub hotkey: String,
    /// Second global hotkey (used when the first one is taken by the system).
    pub hotkey_alt: String,
    /// Snip mode used when none is given: rect, freeform, window or full.
    pub default_mode: String,
    /// What happens once an area is selected: "edit" keeps the overlay open
    /// for annotations, "copy" copies immediately like Win+Shift+S.
    pub after_capture: String,
    /// Put finished snips on the clipboard.
    pub copy_to_clipboard: bool,
    /// Also save every finished snip to `save_dir`.
    pub auto_save: bool,
    /// Folder for automatic saves. Empty = Pictures/Screenshots.
    pub save_dir: String,
    /// png, jpg, bmp or gif.
    pub file_format: String,
    pub jpeg_quality: u8,
    /// Show a confirmation after capturing.
    pub notifications: bool,
    /// Draw a border around every snip.
    pub border: bool,
    pub border_color: String,
    pub border_width: u32,
    /// Show the pixel magnifier while selecting.
    pub loupe: bool,
    /// Seconds used by the delay button.
    pub delay_seconds: u32,
    /// Frames per second for screen recordings.
    pub record_fps: u32,
    /// Default annotation colour.
    pub color: String,
    /// Install new versions automatically (checked once a day).
    pub auto_update: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            hotkey: default_hotkey().into(),
            hotkey_alt: default_hotkey_alt().into(),
            default_mode: "rect".into(),
            after_capture: "edit".into(),
            copy_to_clipboard: true,
            auto_save: true,
            save_dir: String::new(),
            file_format: "png".into(),
            jpeg_quality: 92,
            notifications: true,
            border: false,
            border_color: theme::ACCENT.to_hex_string(),
            border_width: 2,
            loupe: true,
            delay_seconds: 3,
            record_fps: 30,
            color: theme::PALETTE[0].to_hex_string(),
            auto_update: true,
        }
    }
}

fn default_hotkey() -> &'static str {
    if cfg!(target_os = "macos") {
        "Cmd+Shift+2"
    } else {
        "PrintScreen"
    }
}

fn default_hotkey_alt() -> &'static str {
    if cfg!(target_os = "macos") {
        ""
    } else {
        "Alt+Shift+S"
    }
}

impl Config {
    /// `KRAB_CONFIG` overrides the default location.
    pub fn path() -> PathBuf {
        if let Some(p) = std::env::var_os("KRAB_CONFIG").filter(|p| !p.is_empty()) {
            return PathBuf::from(p);
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("krab")
            .join("config.toml")
    }

    /// Load the config; any problem silently falls back to defaults so a
    /// broken file can never stop a screenshot from being taken.
    pub fn load() -> Config {
        match std::fs::read_to_string(Self::path()) {
            Ok(s) => Self::parse(&s),
            Err(_) => Config::default(),
        }
    }

    pub fn parse(s: &str) -> Config {
        toml::from_str::<Config>(s).unwrap_or_default().sanitized()
    }

    fn sanitized(mut self) -> Config {
        self.jpeg_quality = self.jpeg_quality.clamp(10, 100);
        self.border_width = self.border_width.clamp(1, 32);
        self.delay_seconds = self.delay_seconds.clamp(1, 60);
        self.record_fps = self.record_fps.clamp(1, 60);
        self.file_format = match self.file_format.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => "jpg".into(),
            "bmp" => "bmp".into(),
            "gif" => "gif".into(),
            _ => "png".into(),
        };
        self
    }

    /// Write the file (with defaults) if it does not exist yet; returns its path.
    pub fn ensure_file() -> std::io::Result<PathBuf> {
        let path = Self::path();
        if !path.exists() {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            std::fs::write(&path, Config::default().to_commented_toml())?;
        }
        Ok(path)
    }

    pub fn to_commented_toml(&self) -> String {
        let body = toml::to_string_pretty(self).unwrap_or_default();
        format!(
            "# Krab configuration. Restart Krab (tray > Quit, then start it again) after editing.\n\
             # hotkey examples: \"PrintScreen\", \"Alt+Shift+S\", \"Ctrl+Shift+KeyX\", \"\" to disable\n\
             # after_capture:   \"edit\" (annotate in place) or \"copy\" (copy immediately)\n\
             # default_mode:    rect | freeform | window | full\n\
             # file_format:     png | jpg | bmp | gif\n\
             # auto_update:     true installs new versions automatically, false only via `krab --update`\n\n{body}"
        )
    }

    pub fn save_dir(&self) -> PathBuf {
        if !self.save_dir.trim().is_empty() {
            return expand_home(self.save_dir.trim());
        }
        dirs::picture_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Pictures")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Screenshots")
    }

    pub fn video_dir(&self) -> PathBuf {
        if !self.save_dir.trim().is_empty() {
            return expand_home(self.save_dir.trim());
        }
        dirs::video_dir()
            .or_else(|| dirs::home_dir().map(|h| h.join("Videos")))
            .unwrap_or_else(|| PathBuf::from("."))
            .join("Screen Recordings")
    }

    pub fn quick_copy(&self) -> bool {
        self.after_capture.eq_ignore_ascii_case("copy")
    }

    pub fn annotation_color(&self) -> Color {
        Color::parse(&self.color).unwrap_or(theme::PALETTE[0])
    }

    pub fn border_color(&self) -> Color {
        Color::parse(&self.border_color).unwrap_or(theme::ACCENT)
    }
}

fn expand_home(p: &str) -> PathBuf {
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_roundtrip_through_commented_toml() {
        let c = Config::default();
        assert_eq!(Config::parse(&c.to_commented_toml()), c);
    }

    #[test]
    fn partial_file_keeps_other_defaults() {
        let c = Config::parse("auto_save = false\nfile_format = \"JPEG\"\njpeg_quality = 3");
        assert!(!c.auto_save);
        assert_eq!(c.file_format, "jpg");
        assert_eq!(c.jpeg_quality, 10);
        assert_eq!(c.default_mode, "rect");
    }

    #[test]
    fn garbage_falls_back_to_defaults() {
        assert_eq!(Config::parse("this is = = not toml"), Config::default());
    }

    #[test]
    fn custom_save_dir_is_used_verbatim() {
        let c = Config::parse("save_dir = \"/tmp/shots\"");
        assert_eq!(c.save_dir(), PathBuf::from("/tmp/shots"));
    }
}
