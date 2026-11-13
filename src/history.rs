//! Recent snips, offered again in the tray menu.
//!
//! Snips that were saved anyway (automatic saving, "Save as") are listed by
//! their file; the others are kept as PNGs in Krab's cache folder. Only the
//! newest [`KEEP`] are remembered and the cached files of older ones are
//! deleted.

use crate::output::Image;
use std::path::{Path, PathBuf};

pub const KEEP: usize = 20;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub path: PathBuf,
    /// Seconds since the Unix epoch.
    pub time: i64,
    pub w: u32,
    pub h: u32,
}

impl Entry {
    /// "14:35 - 1280 x 720" for menus.
    pub fn label(&self) -> String {
        use chrono::TimeZone;
        let when = chrono::Local
            .timestamp_opt(self.time, 0)
            .single()
            .map(|t| t.format("%H:%M").to_string())
            .unwrap_or_default();
        format!("{when}  \u{00B7}  {} \u{00D7} {}", self.w, self.h)
    }
}

/// Where cached snips and the list live.
pub fn dir() -> PathBuf {
    if let Some(p) = std::env::var_os("KRAB_HISTORY_DIR").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("krab")
        .join("history")
}

fn index(dir: &Path) -> PathBuf {
    dir.join("index.txt")
}

fn read(dir: &Path) -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(index(dir)) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(4, '\t');
            let time = f.next()?.parse().ok()?;
            let w = f.next()?.parse().ok()?;
            let h = f.next()?.parse().ok()?;
            Some(Entry {
                time,
                w,
                h,
                path: PathBuf::from(f.next()?),
            })
        })
        .collect()
}

fn write(dir: &Path, entries: &[Entry]) -> std::io::Result<()> {
    let text: String = entries
        .iter()
        .map(|e| format!("{}\t{}\t{}\t{}\n", e.time, e.w, e.h, e.path.display()))
        .collect();
    std::fs::create_dir_all(dir)?;
    std::fs::write(index(dir), text)
}

/// The remembered snips whose files still exist, newest first.
pub fn list() -> Vec<Entry> {
    list_in(&dir())
}

pub fn list_in(dir: &Path) -> Vec<Entry> {
    read(dir).into_iter().filter(|e| e.path.exists()).collect()
}

/// Remember a snip. `saved` is where it was saved, if it was.
pub fn add(img: &Image, saved: Option<&Path>) -> Result<(), String> {
    add_in(&dir(), img, saved)
}

pub fn add_in(dir: &Path, img: &Image, saved: Option<&Path>) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let path = match saved {
        Some(p) => p.to_path_buf(),
        None => {
            let p = crate::output::unique_path(dir, "Krab", "png");
            img.save(&p, 100)?;
            p
        }
    };
    let mut entries = read(dir);
    entries.retain(|e| e.path != path);
    entries.insert(
        0,
        Entry {
            path,
            time: chrono::Local::now().timestamp(),
            w: img.w,
            h: img.h,
        },
    );
    for old in entries.iter().skip(KEEP) {
        // Only Krab's own copies are deleted, never the user's files.
        if old.path.starts_with(dir) {
            let _ = std::fs::remove_file(&old.path);
        }
    }
    entries.truncate(KEEP);
    write(dir, &entries).map_err(|e| e.to_string())
}

/// Forget everything and delete the cached copies.
pub fn clear() {
    clear_in(&dir());
}

pub fn clear_in(dir: &Path) {
    for e in read(dir) {
        if e.path.starts_with(dir) {
            let _ = std::fs::remove_file(&e.path);
        }
    }
    let _ = std::fs::remove_file(index(dir));
}

/// Load a remembered snip.
pub fn load(path: &Path) -> Result<Image, String> {
    let img = image::open(path).map_err(|e| e.to_string())?.to_rgba8();
    Ok(Image {
        w: img.width(),
        h: img.height(),
        rgba: img.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(w: u32, h: u32) -> Image {
        Image {
            w,
            h,
            rgba: vec![128; (w * h * 4) as usize],
        }
    }

    #[test]
    fn keeps_the_newest_and_cleans_up_its_own_files() {
        let dir = std::env::temp_dir().join(format!("krab-history-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let user_file = std::env::temp_dir().join(format!("krab-user-{}.png", std::process::id()));
        img(3, 3).save(&user_file, 100).unwrap();

        add_in(&dir, &img(4, 2), Some(&user_file)).unwrap();
        for i in 0..KEEP + 3 {
            add_in(&dir, &img(10 + i as u32, 5), None).unwrap();
        }
        let list = list_in(&dir);
        assert_eq!(list.len(), KEEP);
        assert_eq!(
            (list[0].w, list[0].h),
            (10 + KEEP as u32 + 2, 5),
            "newest first"
        );
        // The user's own file dropped out of the list but was not deleted.
        assert!(list.iter().all(|e| e.path != user_file));
        assert!(user_file.exists());
        // Only the remembered copies (and the index) remain in the folder.
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, KEEP + 1);
        assert_eq!(load(&list[0].path).unwrap().w, 10 + KEEP as u32 + 2);

        clear_in(&dir);
        assert!(list_in(&dir).is_empty());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_file(&user_file).unwrap();
    }

    #[test]
    fn labels_show_time_and_size() {
        let e = Entry {
            path: "x.png".into(),
            time: 0,
            w: 1280,
            h: 720,
        };
        assert!(e.label().ends_with("1280 \u{00D7} 720"));
    }
}
