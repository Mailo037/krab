//! The install step of the updater, run against the real Krab executable.

use krab::update;
use std::path::PathBuf;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("krab-update-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn exe_name() -> &'static str {
    if cfg!(windows) {
        "krab.exe"
    } else {
        "krab"
    }
}

#[test]
fn a_verified_download_replaces_the_executable() {
    let dir = scratch("ok");
    let installed = dir.join(exe_name());
    std::fs::write(&installed, b"old version").unwrap();
    let new = dir.join(if cfg!(windows) {
        ".krab-update.exe"
    } else {
        ".krab-update.tmp"
    });
    std::fs::copy(env!("CARGO_BIN_EXE_krab"), &new).unwrap();
    let bytes = std::fs::read(&new).unwrap();

    update::install(&new, &update::sha256_of(&bytes), &installed).unwrap();

    assert_eq!(std::fs::read(&installed).unwrap(), bytes);
    assert!(!new.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_download_changes_nothing() {
    let dir = scratch("bad");
    let installed = dir.join(exe_name());
    std::fs::write(&installed, b"old version").unwrap();
    let new = dir.join("download");
    std::fs::copy(env!("CARGO_BIN_EXE_krab"), &new).unwrap();

    let err = update::install(&new, &"0".repeat(64), &installed).unwrap_err();

    assert!(err.contains("checksum"), "{err}");
    assert_eq!(std::fs::read(&installed).unwrap(), b"old version");
    let _ = std::fs::remove_dir_all(&dir);
}
