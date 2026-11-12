//! "Start with the system" for the background instance.

use std::io;

#[cfg(not(target_os = "macos"))]
const APP: &str = "Krab";

fn exe() -> io::Result<String> {
    Ok(std::env::current_exe()?.to_string_lossy().into_owned())
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use std::ptr::null_mut;
    use windows_sys::Win32::System::Registry::{
        RegDeleteKeyValueW, RegGetValueW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ, RRF_RT_REG_SZ,
    };

    const RUN_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn is_enabled() -> bool {
        let (key, name) = (wide(RUN_KEY), wide(APP));
        unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                RRF_RT_REG_SZ,
                null_mut(),
                null_mut(),
                null_mut(),
            ) == 0
        }
    }

    pub fn set(enabled: bool) -> io::Result<()> {
        let (key, name) = (wide(RUN_KEY), wide(APP));
        let status = unsafe {
            if enabled {
                let cmd = wide(&format!("\"{}\" --daemon --quiet", exe()?));
                RegSetKeyValueW(
                    HKEY_CURRENT_USER,
                    key.as_ptr(),
                    name.as_ptr(),
                    REG_SZ,
                    cmd.as_ptr() as *const c_void,
                    (cmd.len() * 2) as u32,
                )
            } else {
                RegDeleteKeyValueW(HKEY_CURRENT_USER, key.as_ptr(), name.as_ptr())
            }
        };
        // 2 = ERROR_FILE_NOT_FOUND: disabling something that was never enabled.
        if status == 0 || (!enabled && status == 2) {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(status as i32))
        }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    fn plist() -> PathBuf {
        dirs::home_dir()
            .unwrap_or_default()
            .join("Library/LaunchAgents/dev.krab.daemon.plist")
    }

    pub fn is_enabled() -> bool {
        plist().exists()
    }

    pub fn set(enabled: bool) -> io::Result<()> {
        let path = plist();
        if !enabled {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>dev.krab.daemon</string>\n\
             <key>ProgramArguments</key><array><string>{}</string><string>--daemon</string><string>--quiet</string></array>\n\
             <key>RunAtLoad</key><true/>\n\
             </dict></plist>\n",
            xml_escape(&exe()?)
        );
        std::fs::write(path, body)
    }

    fn xml_escape(s: &str) -> String {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use std::path::PathBuf;

    fn desktop_file() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_default()
            .join("autostart/krab.desktop")
    }

    pub fn is_enabled() -> bool {
        desktop_file().exists()
    }

    pub fn set(enabled: bool) -> io::Result<()> {
        let path = desktop_file();
        if !enabled {
            return match std::fs::remove_file(path) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
                _ => Ok(()),
            };
        }
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let body = format!(
            "[Desktop Entry]\nType=Application\nName={APP}\nComment=Lightweight snipping tool\n\
             Exec=\"{}\" --daemon --quiet\nX-GNOME-Autostart-enabled=true\nNoDisplay=true\n",
            exe()?
        );
        std::fs::write(path, body)
    }
}

pub fn is_enabled() -> bool {
    imp::is_enabled()
}

pub fn set(enabled: bool) -> io::Result<()> {
    imp::set(enabled)
}
