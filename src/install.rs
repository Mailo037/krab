//! Installing Krab as a regular desktop application, per user, no admin
//! rights needed:
//!
//! | System  | Program                                  | Shows up in                          |
//! |---------|------------------------------------------|--------------------------------------|
//! | Windows | `%LOCALAPPDATA%\Programs\Krab\krab.exe`  | Start menu, Settings > Apps          |
//! | macOS   | `~/Applications/Krab.app`                | Launchpad, Spotlight, Finder         |
//! | Linux   | `~/.local/bin/krab` + `krab.desktop`     | the desktop's application menu       |
//!
//! Double-clicking the downloaded program installs it there and starts the
//! installed copy; `krab --uninstall` (or Settings > Apps on Windows) removes it.

use std::path::{Path, PathBuf};

/// Where the installed program lives.
pub fn installed_exe() -> Option<PathBuf> {
    imp::installed_exe()
}

/// Whether this process runs from an installed copy (ours, or one a package
/// manager put in place).
pub fn is_installed() -> bool {
    let Ok(exe) = std::env::current_exe().and_then(|p| p.canonicalize()) else {
        return true;
    };
    if let Some(target) = installed_exe().and_then(|p| p.canonicalize().ok()) {
        if exe == target {
            return true;
        }
    }
    imp::managed_elsewhere(&exe)
}

/// Whether a plain launch of this copy should install it first.
pub fn should_install() -> bool {
    if std::env::var_os("KRAB_NO_INSTALL").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    !crate::update::is_dev_build(&exe) && !is_installed()
}

/// Copy the running program to its installed place and register it with the
/// desktop. Returns the installed program.
pub fn install() -> Result<PathBuf, String> {
    let src = std::env::current_exe().map_err(|e| e.to_string())?;
    let dst = installed_exe().ok_or("no place to install to")?;
    imp::install(&src, &dst)?;
    Ok(dst)
}

/// Remove everything [`install`] created. Settings and screenshots stay.
pub fn uninstall() -> Result<(), String> {
    let _ = crate::platform::autostart::set(false);
    imp::uninstall()
}

/// Keep the registered version number current after an update.
pub fn refresh() {
    if is_installed() {
        imp::refresh();
    }
}

/// Copy a file, also when the destination is a running program (Windows
/// lets a running executable be renamed, not overwritten).
fn copy_over(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(dir) = dst.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let same = src.canonicalize().ok() == dst.canonicalize().ok() && dst.exists();
    if same {
        return Ok(());
    }
    let tmp = dst.with_extension("new");
    std::fs::copy(src, &tmp).map_err(|e| format!("{}: {e}", dst.display()))?;
    if dst.exists() && std::fs::remove_file(dst).is_err() {
        let mut old = dst.as_os_str().to_os_string();
        old.push(".old");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(dst, &old).map_err(|e| format!("{}: {e}", dst.display()))?;
    }
    std::fs::rename(&tmp, dst).map_err(|e| format!("{}: {e}", dst.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dst, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// The app icon as PNG.
#[cfg_attr(windows, allow(dead_code))]
fn icon_png(size: u32) -> Vec<u8> {
    let rgba = crate::brand::mark_rgba(size, true);
    let mut out = Vec::new();
    let _ = image::write_buffer_with_format(
        &mut std::io::Cursor::new(&mut out),
        &rgba,
        size,
        size,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    );
    out
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::ffi::c_void;
    use windows_sys::Win32::System::Registry::{
        RegDeleteTreeW, RegSetKeyValueW, HKEY_CURRENT_USER, REG_DWORD, REG_SZ,
    };

    const UNINSTALL_KEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Krab";

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn installed_exe() -> Option<PathBuf> {
        Some(
            dirs::data_local_dir()?
                .join("Programs")
                .join("Krab")
                .join("krab.exe"),
        )
    }

    fn shortcut() -> Option<PathBuf> {
        Some(
            dirs::data_dir()?
                .join("Microsoft\\Windows\\Start Menu\\Programs")
                .join("Krab.lnk"),
        )
    }

    pub fn managed_elsewhere(exe: &Path) -> bool {
        // Program Files means an installer for all users put it there.
        exe.to_string_lossy()
            .to_ascii_lowercase()
            .contains("\\program files")
    }

    fn set_sz(name: &str, value: &str) {
        let (key, name, value) = (wide(UNINSTALL_KEY), wide(name), wide(value));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_SZ,
                value.as_ptr() as *const c_void,
                (value.len() * 2) as u32,
            );
        }
    }

    fn set_dword(name: &str, value: u32) {
        let (key, name) = (wide(UNINSTALL_KEY), wide(name));
        unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                name.as_ptr(),
                REG_DWORD,
                &value as *const u32 as *const c_void,
                4,
            );
        }
    }

    pub fn install(src: &Path, dst: &Path) -> Result<(), String> {
        copy_over(src, dst)?;
        let dir = dst.parent().unwrap_or(dst);
        let exe = dst.display().to_string();
        if let Some(lnk) = shortcut() {
            create_shortcut(dst, &lnk).map_err(|e| format!("Start menu entry: {e}"))?;
        }
        // Settings > Apps: name, icon, version and how to uninstall.
        set_sz("DisplayName", "Krab");
        set_sz("DisplayIcon", &format!("{exe},0"));
        set_sz("DisplayVersion", env!("CARGO_PKG_VERSION"));
        set_sz("Publisher", "Krab");
        set_sz("InstallLocation", &dir.display().to_string());
        set_sz("UninstallString", &format!("\"{exe}\" --uninstall"));
        set_sz("URLInfoAbout", "https://github.com/Mailo037/krab");
        set_dword("NoModify", 1);
        set_dword("NoRepair", 1);
        let size_kb = std::fs::metadata(dst).map(|m| m.len() / 1024).unwrap_or(0);
        set_dword("EstimatedSize", size_kb as u32);
        Ok(())
    }

    pub fn refresh() {
        set_sz("DisplayVersion", env!("CARGO_PKG_VERSION"));
    }

    pub fn uninstall() -> Result<(), String> {
        if let Some(lnk) = shortcut() {
            let _ = std::fs::remove_file(lnk);
        }
        let key = wide(UNINSTALL_KEY);
        unsafe {
            RegDeleteTreeW(HKEY_CURRENT_USER, key.as_ptr());
        }
        // The running program cannot delete itself: a short-lived helper
        // removes the folder once this process has exited.
        if let Some(dir) = installed_exe().and_then(|p| p.parent().map(Path::to_path_buf)) {
            if dir.exists() {
                let mut cmd = std::process::Command::new("cmd.exe");
                cmd.arg("/C").arg(format!(
                    "ping -n 3 127.0.0.1 >NUL & rmdir /S /Q \"{}\"",
                    dir.display()
                ));
                crate::platform::hide_console(&mut cmd);
                let _ = cmd.spawn();
            }
        }
        Ok(())
    }

    /// A Start menu shortcut, made through the shell's own COM interface.
    fn create_shortcut(target: &Path, lnk: &Path) -> windows::core::Result<()> {
        use windows::core::{Interface, HSTRING};
        use windows::Win32::System::Com::{
            CoCreateInstance, CoInitializeEx, IPersistFile, CLSCTX_INPROC_SERVER,
            COINIT_APARTMENTTHREADED,
        };
        use windows::Win32::UI::Shell::{IShellLinkW, ShellLink};
        if let Some(dir) = lnk.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        unsafe {
            // Already initialised on this thread is fine too.
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
            let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
            link.SetPath(&HSTRING::from(target.as_os_str()))?;
            if let Some(dir) = target.parent() {
                link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
            }
            link.SetDescription(&HSTRING::from("Snip, annotate and record the screen"))?;
            link.SetIconLocation(&HSTRING::from(target.as_os_str()), 0)?;
            let file: IPersistFile = link.cast()?;
            file.Save(&HSTRING::from(lnk.as_os_str()), true)?;
        }
        Ok(())
    }
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;

    fn app() -> Option<PathBuf> {
        Some(dirs::home_dir()?.join("Applications").join("Krab.app"))
    }

    pub fn installed_exe() -> Option<PathBuf> {
        Some(app()?.join("Contents/MacOS/krab"))
    }

    pub fn managed_elsewhere(exe: &Path) -> bool {
        // Any Krab.app (for example one dragged into /Applications) or a
        // Homebrew install counts as installed.
        let s = exe.to_string_lossy();
        s.contains(".app/Contents/MacOS/")
            || s.starts_with("/opt/homebrew/")
            || s.starts_with("/usr/local/Cellar/")
    }

    fn info_plist() -> String {
        let v = env!("CARGO_PKG_VERSION");
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>CFBundleName</key><string>Krab</string>\n\
             <key>CFBundleDisplayName</key><string>Krab</string>\n\
             <key>CFBundleIdentifier</key><string>dev.krab.Krab</string>\n\
             <key>CFBundleExecutable</key><string>krab</string>\n\
             <key>CFBundleIconFile</key><string>krab</string>\n\
             <key>CFBundlePackageType</key><string>APPL</string>\n\
             <key>CFBundleShortVersionString</key><string>{v}</string>\n\
             <key>CFBundleVersion</key><string>{v}</string>\n\
             <key>LSMinimumSystemVersion</key><string>11.0</string>\n\
             <key>LSUIElement</key><true/>\n\
             <key>NSHighResolutionCapable</key><true/>\n\
             <key>NSScreenCaptureUsageDescription</key><string>Krab captures the screen when you take a snip or a recording.</string>\n\
             </dict></plist>\n"
        )
    }

    /// An `.icns` file: a header followed by PNG images tagged by size.
    fn icns() -> Vec<u8> {
        let mut body = Vec::new();
        for (tag, size) in [
            (b"ic07", 128u32),
            (b"ic08", 256),
            (b"ic09", 512),
            (b"ic10", 1024),
        ] {
            let png = icon_png(size);
            body.extend_from_slice(tag);
            body.extend_from_slice(&((png.len() + 8) as u32).to_be_bytes());
            body.extend_from_slice(&png);
        }
        let mut out = b"icns".to_vec();
        out.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
        out.extend_from_slice(&body);
        out
    }

    pub fn install(src: &Path, dst: &Path) -> Result<(), String> {
        let app = app().ok_or("no home folder")?;
        copy_over(src, dst)?;
        let write = |rel: &str, data: &[u8]| -> Result<(), String> {
            let p = app.join(rel);
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
            }
            std::fs::write(&p, data).map_err(|e| format!("{}: {e}", p.display()))
        };
        write("Contents/Info.plist", info_plist().as_bytes())?;
        write("Contents/Resources/krab.icns", &icns())?;
        // An ad-hoc signature for the whole bundle (required on Apple silicon).
        let _ = std::process::Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&app)
            .output();
        // Make Launchpad and Spotlight notice it right away.
        let _ = std::process::Command::new(
            "/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister",
        )
        .args(["-f"])
        .arg(&app)
        .output();
        Ok(())
    }

    pub fn refresh() {
        if let Some(app) = app() {
            let _ = std::fs::write(app.join("Contents/Info.plist"), info_plist());
        }
    }

    pub fn uninstall() -> Result<(), String> {
        if let Some(app) = app() {
            if app.exists() {
                std::fs::remove_dir_all(&app).map_err(|e| format!("{}: {e}", app.display()))?;
            }
        }
        Ok(())
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;

    pub fn installed_exe() -> Option<PathBuf> {
        Some(dirs::home_dir()?.join(".local/bin/krab"))
    }

    fn desktop_file() -> Option<PathBuf> {
        Some(dirs::data_dir()?.join("applications/krab.desktop"))
    }

    fn icon_file(size: u32) -> Option<PathBuf> {
        Some(dirs::data_dir()?.join(format!("icons/hicolor/{size}x{size}/apps/krab.png")))
    }

    const ICON_SIZES: [u32; 4] = [48, 128, 256, 512];

    pub fn managed_elsewhere(exe: &Path) -> bool {
        // Installed by a package manager, Flatpak or similar.
        let s = exe.to_string_lossy();
        s.starts_with("/usr/")
            || s.starts_with("/opt/")
            || s.starts_with("/app/")
            || s.starts_with("/nix/")
    }

    pub fn install(src: &Path, dst: &Path) -> Result<(), String> {
        copy_over(src, dst)?;
        for size in ICON_SIZES {
            if let Some(p) = icon_file(size) {
                if let Some(d) = p.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                std::fs::write(&p, icon_png(size)).map_err(|e| format!("{}: {e}", p.display()))?;
            }
        }
        let exe = dst.display();
        let entry = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Krab\n\
             GenericName=Screenshot Tool\n\
             Comment=Snip, annotate and record the screen\n\
             Exec=\"{exe}\"\n\
             Icon=krab\n\
             Terminal=false\n\
             StartupNotify=false\n\
             Categories=Utility;Graphics;\n\
             Keywords=screenshot;snip;snipping;capture;record;screen;\n\
             Actions=Record;Settings;\n\
             \n\
             [Desktop Action Record]\n\
             Name=Record screen\n\
             Exec=\"{exe}\" --record\n\
             \n\
             [Desktop Action Settings]\n\
             Name=Settings\n\
             Exec=\"{exe}\" --settings\n"
        );
        let path = desktop_file().ok_or("no data folder")?;
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, entry).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Some(apps) = path.parent() {
            let _ = std::process::Command::new("update-desktop-database")
                .arg(apps)
                .output();
        }
        Ok(())
    }

    pub fn refresh() {}

    pub fn uninstall() -> Result<(), String> {
        let mut files: Vec<PathBuf> = ICON_SIZES.iter().filter_map(|s| icon_file(*s)).collect();
        files.extend(desktop_file());
        files.extend(installed_exe());
        for f in files {
            let _ = std::fs::remove_file(f);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_location_is_per_user() {
        let exe = installed_exe().expect("a home folder");
        let home = dirs::home_dir().unwrap();
        assert!(exe.starts_with(&home), "{}", exe.display());
    }

    #[test]
    fn icon_is_a_png() {
        assert_eq!(&icon_png(48)[..4], b"\x89PNG");
    }

    #[test]
    fn copy_over_replaces_an_existing_file() {
        let dir = std::env::temp_dir().join(format!("krab-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (src, dst) = (dir.join("a"), dir.join("sub").join("b"));
        std::fs::write(&src, b"new").unwrap();
        copy_over(&src, &dst).unwrap();
        std::fs::write(&src, b"newer").unwrap();
        copy_over(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"newer");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
