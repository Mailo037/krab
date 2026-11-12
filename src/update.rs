//! Automatic updates from the GitHub releases page.
//!
//! Every release publishes, next to the archives, the bare executable for each
//! platform and a `SHA256SUMS` file. Updating means:
//!
//! 1. follow the `releases/latest` redirect to learn the newest tag,
//! 2. download that tag's executable for this platform next to the running
//!    one and check it against `SHA256SUMS`,
//! 3. run it once with `--version` to make sure it starts,
//! 4. swap it in place of the running executable.
//!
//! Downloads go through the system's `curl` (built into Windows 10 1803+,
//! macOS and practically every Linux desktop), which keeps a TLS stack out of
//! Krab's binary. Nothing is installed when any step fails.

use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const REPO: &str = "Mailo037/krab";

/// How often the automatic check runs.
pub const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Set once a new version is installed and the running instance should be
/// replaced by it as soon as it is idle: the path of the new executable.
static RESTART: Mutex<Option<PathBuf>> = Mutex::new(None);

#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The running version is the newest one.
    UpToDate(String),
    /// A newer version was downloaded and is now in place at `exe`.
    Updated {
        from: String,
        to: String,
        exe: PathBuf,
    },
}

/// `major.minor.patch` (anything after the patch number is ignored).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u64, pub u64, pub u64);

impl Version {
    pub fn parse(s: &str) -> Option<Version> {
        let s = s.trim().trim_start_matches(['v', 'V']);
        let core = s.split(['-', '+']).next()?;
        let mut it = core.split('.').map(|p| p.parse::<u64>().ok());
        let v = Version(
            it.next()??,
            it.next().unwrap_or(Some(0))?,
            it.next().unwrap_or(Some(0))?,
        );
        Some(v)
    }

    pub fn current() -> Version {
        Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Version(0, 0, 0))
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// Name of the bare executable published for this platform.
pub fn asset_name() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("krab-windows-x64.exe")
    } else if cfg!(target_os = "macos") {
        Some("krab-macos-universal")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("krab-linux-x86_64")
    } else {
        None
    }
}

/// Ask the running instance to restart into the executable at `exe` once idle.
pub fn request_restart(exe: PathBuf) {
    if let Ok(mut r) = RESTART.lock() {
        *r = Some(exe);
    }
}

/// The executable to restart into, if an update was installed.
pub fn restart_requested() -> Option<PathBuf> {
    RESTART.lock().ok().and_then(|r| r.clone())
}

/// Start `exe` as the new background instance. It waits for this process to
/// release the single-instance socket before taking over.
pub fn spawn_daemon(exe: &Path) -> Result<(), String> {
    let mut cmd = Command::new(exe);
    cmd.args(["--daemon", "--updated"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::platform::hide_console(&mut cmd);
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Start `krab --update --background` as a separate process, so a one-shot
/// snip never waits for the network.
pub fn spawn_background_check() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let mut cmd = Command::new(exe);
    cmd.args(["--update", "--background"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::platform::hide_console(&mut cmd);
    let _ = cmd.spawn();
}

// ---- schedule ---------------------------------------------------------------

fn stamp_file() -> Option<PathBuf> {
    Some(dirs::cache_dir()?.join("krab").join("last-update-check"))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether the automatic check is due (last one more than a day ago).
pub fn due() -> bool {
    if !enabled_here() {
        return false;
    }
    let last = stamp_file()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|s| s.trim().parse::<u64>().ok())
        .unwrap_or(0);
    now_secs().saturating_sub(last) >= INTERVAL.as_secs()
}

/// Remember that a check started now (written first, so that several
/// processes starting at once do not all go online).
pub fn mark_checked() {
    if let Some(path) = stamp_file() {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(path, now_secs().to_string());
    }
}

/// Automatic updates are off for builds made from a source checkout (they
/// would be replaced by the release binary) and when `KRAB_NO_UPDATE` is set.
pub fn enabled_here() -> bool {
    if std::env::var_os("KRAB_NO_UPDATE").is_some_and(|v| !v.is_empty()) {
        return false;
    }
    if asset_name().is_none() {
        return false;
    }
    match std::env::current_exe() {
        Ok(exe) => !is_dev_build(&exe),
        Err(_) => false,
    }
}

/// A build running from a cargo `target` folder.
pub fn is_dev_build(exe: &Path) -> bool {
    let parts: Vec<String> = exe
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_ascii_lowercase())
        .collect();
    parts
        .windows(2)
        .any(|w| w[0] == "target" && (w[1] == "debug" || w[1] == "release" || w[1].contains('-')))
}

/// Remove what a previous update left behind (Windows keeps the replaced
/// executable around as `krab.exe.old` until the next start).
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(old_path(&exe));
        let _ = std::fs::remove_file(tmp_path(&exe));
    }
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(".old");
    exe.with_file_name(name)
}

fn tmp_path(exe: &Path) -> PathBuf {
    // Windows decides how to start a file by its extension.
    exe.with_file_name(if cfg!(windows) {
        ".krab-update.exe"
    } else {
        ".krab-update.tmp"
    })
}

// ---- network ----------------------------------------------------------------

fn curl() -> Command {
    let mut cmd = Command::new(if cfg!(windows) { "curl.exe" } else { "curl" });
    cmd.args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--connect-timeout", "15"])
        .args(["--user-agent", concat!("krab/", env!("CARGO_PKG_VERSION"))])
        .stdin(Stdio::null());
    crate::platform::hide_console(&mut cmd);
    cmd
}

fn run(mut cmd: Command) -> Result<Vec<u8>, String> {
    let out = cmd.output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            "curl is not installed - install it to receive updates".to_string()
        } else {
            format!("could not run curl: {e}")
        }
    })?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() {
            "download failed".into()
        } else {
            err
        });
    }
    Ok(out.stdout)
}

/// The newest release tag, read from where `releases/latest` redirects to.
pub fn latest_tag() -> Result<String, String> {
    let mut cmd = curl();
    cmd.args(["--max-time", "30", "--head", "--output"])
        .arg(if cfg!(windows) { "NUL" } else { "/dev/null" })
        .args(["--write-out", "%{url_effective}"])
        .arg(format!("https://github.com/{REPO}/releases/latest"));
    let out = run(cmd).map_err(|e| {
        if e.contains("404") {
            format!("no public release found at github.com/{REPO}")
        } else {
            e
        }
    })?;
    let url = String::from_utf8_lossy(&out).trim().to_string();
    tag_from_url(&url).ok_or_else(|| format!("no release found ({url})"))
}

fn tag_from_url(url: &str) -> Option<String> {
    let tag = url.rsplit_once("/releases/tag/")?.1.trim_end_matches('/');
    (!tag.is_empty()).then(|| tag.to_string())
}

fn download_url(tag: &str, file: &str) -> String {
    format!("https://github.com/{REPO}/releases/download/{tag}/{file}")
}

/// Find `file`'s hash in a `sha256sum`-style listing.
fn expected_hash(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hash = parts.next()?;
        let name = parts.next()?.trim_start_matches('*');
        (name == file && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---- install ----------------------------------------------------------------

/// Check for a newer release and install it. Does not restart anything.
pub fn check_and_install() -> Result<Outcome, String> {
    let current = Version::current();
    let asset = asset_name().ok_or("updates are not available for this platform")?;
    let exe = std::env::current_exe()
        .and_then(|p| p.canonicalize())
        .map_err(|e| format!("cannot locate the Krab executable: {e}"))?;
    if is_dev_build(&exe) {
        return Err("this is a development build; update it from source".into());
    }

    let tag = latest_tag()?;
    let latest = Version::parse(&tag).ok_or_else(|| format!("unexpected release tag '{tag}'"))?;
    if latest <= current {
        return Ok(Outcome::UpToDate(current.to_string()));
    }

    let sums = run({
        let mut cmd = curl();
        cmd.args(["--max-time", "60"])
            .arg(download_url(&tag, "SHA256SUMS"));
        cmd
    })?;
    let want = expected_hash(&String::from_utf8_lossy(&sums), asset)
        .ok_or_else(|| format!("{asset} is missing from the release checksums"))?;

    let tmp = tmp_path(&exe);
    if let Err(e) = std::fs::File::create(&tmp) {
        return Err(format!(
            "no write access to {} ({e}) - download the new version from the releases page",
            exe.parent().unwrap_or(&exe).display()
        ));
    }
    let result = (|| {
        let mut cmd = curl();
        cmd.args(["--max-time", "600", "--output"])
            .arg(&tmp)
            .arg(download_url(&tag, asset));
        run(cmd)?;
        install(&tmp, &want, &exe)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result?;
    Ok(Outcome::Updated {
        from: current.to_string(),
        to: latest.to_string(),
        exe,
    })
}

/// Check the downloaded file `new` against `sha256`, make sure it starts and
/// move it to `exe`. On error `exe` is left as it was.
#[doc(hidden)]
pub fn install(new: &Path, sha256: &str, exe: &Path) -> Result<(), String> {
    let bytes = std::fs::read(new).map_err(|e| e.to_string())?;
    if sha256_hex(&bytes) != sha256.to_ascii_lowercase() {
        return Err("the download is damaged (checksum mismatch)".to_string());
    }
    make_executable(new)?;
    verify_starts(new)?;
    swap(new, exe)
}

#[doc(hidden)]
pub fn sha256_of(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), String> {
    Ok(())
}

/// Run the new executable once; refuse to install one that does not start.
fn verify_starts(path: &Path) -> Result<(), String> {
    let mut cmd = Command::new(path);
    cmd.arg("--version")
        .env("KRAB_NO_UPDATE", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::platform::hide_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("the new version does not start: {e}"))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(status)) => return Err(format!("the new version does not start ({status})")),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(50))
            }
            _ => {
                let _ = child.kill();
                return Err("the new version does not start (timed out)".into());
            }
        }
    }
}

/// Put `new` where `exe` is. A running executable cannot be overwritten on
/// Windows but it can be renamed, so it is moved aside first; on Unix the
/// rename replaces the file atomically while the old inode stays valid for
/// processes that still run it.
fn swap(new: &Path, exe: &Path) -> Result<(), String> {
    if cfg!(windows) {
        let old = old_path(exe);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| format!("cannot replace {}: {e}", exe.display()))?;
        if let Err(e) = std::fs::rename(new, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(format!("cannot replace {}: {e}", exe.display()));
        }
        Ok(())
    } else {
        std::fs::rename(new, exe).map_err(|e| format!("cannot replace {}: {e}", exe.display()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_numerically() {
        assert_eq!(Version::parse("v0.10.2"), Some(Version(0, 10, 2)));
        assert_eq!(Version::parse("1.2"), Some(Version(1, 2, 0)));
        assert_eq!(Version::parse("2.0.0-beta.1"), Some(Version(2, 0, 0)));
        assert_eq!(Version::parse("latest"), None);
        assert!(Version(0, 10, 0) > Version(0, 9, 9));
        assert!(Version::parse(env!("CARGO_PKG_VERSION")).is_some());
    }

    #[test]
    fn tag_is_read_from_the_redirect_target() {
        assert_eq!(
            tag_from_url("https://github.com/Mailo037/krab/releases/tag/v0.2.0"),
            Some("v0.2.0".into())
        );
        // No release yet: GitHub stays on the releases page.
        assert_eq!(
            tag_from_url("https://github.com/Mailo037/krab/releases"),
            None
        );
    }

    #[test]
    fn checksum_lines_are_matched_by_file_name() {
        let a = "a".repeat(64);
        let b = "B".repeat(64);
        let sums = format!("{a}  krab-linux-x86_64\n{b} *krab-windows-x64.exe\n");
        assert_eq!(expected_hash(&sums, "krab-linux-x86_64"), Some(a));
        assert_eq!(
            expected_hash(&sums, "krab-windows-x64.exe"),
            Some(b.to_lowercase())
        );
        assert_eq!(expected_hash(&sums, "krab-macos-universal"), None);
    }

    #[test]
    fn sha256_matches_the_reference() {
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn builds_inside_cargo_target_are_not_updated() {
        assert!(is_dev_build(Path::new("/home/a/krab/target/release/krab")));
        assert!(is_dev_build(Path::new(
            "C:/src/krab/target/x86_64-pc-windows-msvc/release/krab.exe"
        )));
        assert!(!is_dev_build(Path::new("/usr/local/bin/krab")));
        assert!(!is_dev_build(Path::new(
            "C:/Users/a/AppData/Local/Krab/krab.exe"
        )));
    }

    #[test]
    fn windows_keeps_the_old_executable_beside_the_new_one() {
        let exe = Path::new("dir").join("krab.exe");
        assert_eq!(old_path(&exe), Path::new("dir").join("krab.exe.old"));
    }
}
