//! Where finished snips go: clipboard, disk, other apps.

use crate::config::Config;
use crate::geom::{Pt, RectF};
use crate::gfx::{Canvas, Color};
#[cfg(not(windows))]
use std::borrow::Cow;
use std::io::Cursor;
use std::path::{Path, PathBuf};

/// A finished capture: straight-alpha RGBA.
#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    pub rgba: Vec<u8>,
}

impl Image {
    pub fn is_opaque(&self) -> bool {
        self.rgba.chunks_exact(4).all(|p| p[3] == 255)
    }

    /// Flatten transparency onto white (for formats without alpha).
    fn rgb_on_white(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity((self.w * self.h * 3) as usize);
        for p in self.rgba.chunks_exact(4) {
            let a = p[3] as u32;
            let f = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
            out.extend_from_slice(&[f(p[0]), f(p[1]), f(p[2])]);
        }
        out
    }

    /// Encode in the format implied by `ext` (png, jpg, bmp, gif).
    pub fn encode(&self, ext: &str, jpeg_quality: u8) -> Result<Vec<u8>, String> {
        use image::ImageEncoder;
        let mut out = Vec::new();
        let err = |e: image::ImageError| e.to_string();
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => {
                image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, jpeg_quality)
                    .write_image(
                        &self.rgb_on_white(),
                        self.w,
                        self.h,
                        image::ExtendedColorType::Rgb8,
                    )
                    .map_err(err)?;
            }
            "bmp" => {
                image::codecs::bmp::BmpEncoder::new(&mut out)
                    .write_image(
                        &self.rgb_on_white(),
                        self.w,
                        self.h,
                        image::ExtendedColorType::Rgb8,
                    )
                    .map_err(err)?;
            }
            "gif" => {
                let frame = image::RgbaImage::from_raw(self.w, self.h, self.rgba.clone())
                    .ok_or_else(|| "bad image dimensions".to_string())?;
                image::codecs::gif::GifEncoder::new(&mut out)
                    .encode_frame(image::Frame::new(frame))
                    .map_err(err)?;
            }
            _ => {
                image::codecs::png::PngEncoder::new_with_quality(
                    Cursor::new(&mut out),
                    image::codecs::png::CompressionType::Fast,
                    image::codecs::png::FilterType::Adaptive,
                )
                .write_image(&self.rgba, self.w, self.h, image::ExtendedColorType::Rgba8)
                .map_err(err)?;
            }
        }
        Ok(out)
    }

    pub fn save(&self, path: &Path, jpeg_quality: u8) -> Result<(), String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
        let bytes = self.encode(ext, jpeg_quality)?;
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Put `img` on a gradient from `a` to `b`, with padding, rounded corners
/// and a soft shadow - ready for slides, docs and social media.
pub fn frame(img: &Image, a: Color, b: Color, scale: f32) -> Image {
    let s = scale.max(1.0);
    let pad = (img.w.max(img.h) as f32 * 0.07)
        .clamp(28.0 * s, 140.0 * s)
        .round() as u32;
    let (w, h) = (img.w + 2 * pad, img.h + 2 * pad);
    let mut buf = vec![0u32; (w * h) as usize];
    // The snip, premultiplied like everything tiny-skia draws.
    let px: Vec<u32> = img
        .rgba
        .chunks_exact(4)
        .map(|p| {
            let al = p[3] as u32;
            let m = |c: u8| (c as u32 * al + 127) / 255;
            al << 24 | m(p[0]) << 16 | m(p[1]) << 8 | m(p[2])
        })
        .collect();
    {
        let mut c = Canvas::new(&mut buf, w, h);
        let full = RectF::new(0.0, 0.0, w as f32, h as f32);
        if let Some(path) = crate::gfx::rrect_path(full, 0.0) {
            c.fill_path_gradient(&path, Pt::new(0.0, 0.0), a, Pt::new(w as f32, h as f32), b);
        }
        let dst = RectF::new(pad as f32, pad as f32, img.w as f32, img.h as f32);
        let radius = (10.0 * s).min(img.w.min(img.h) as f32 / 4.0);
        c.shadow(dst, radius, 22.0 * s, 0.9);
        c.draw_image(&px, img.w, img.h, dst, radius);
    }
    Image {
        w,
        h,
        rgba: crate::gfx::premul_argb_to_rgba(&buf),
    }
}

/// `Krab_2026-11-12_143501.png`
pub fn timestamp_name(prefix: &str, ext: &str) -> String {
    format!(
        "{prefix}_{}.{ext}",
        chrono::Local::now().format("%Y-%m-%d_%H%M%S")
    )
}

/// A path in `dir` that does not exist yet.
pub fn unique_path(dir: &Path, prefix: &str, ext: &str) -> PathBuf {
    let first = dir.join(timestamp_name(prefix, ext));
    if !first.exists() {
        return first;
    }
    let stem = first
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(prefix)
        .to_string();
    (2..)
        .map(|i| dir.join(format!("{stem}_{i}.{ext}")))
        .find(|p| !p.exists())
        .unwrap_or(first)
}

/// Save into the configured screenshots folder.
pub fn auto_save(img: &Image, cfg: &Config) -> Result<PathBuf, String> {
    let path = unique_path(&cfg.save_dir(), "Krab", &cfg.file_format);
    img.save(&path, cfg.jpeg_quality)?;
    Ok(path)
}

/// Ask where to save. `None` if the dialog was cancelled.
pub fn save_as(img: &Image, cfg: &Config) -> Result<Option<PathBuf>, String> {
    let dir = cfg.save_dir();
    let _ = std::fs::create_dir_all(&dir);
    let mut dialog = rfd::FileDialog::new()
        .set_title("Save snip")
        .set_directory(&dir)
        .set_file_name(timestamp_name("Krab", &cfg.file_format));
    // The configured format goes first so it is preselected.
    let filters: [(&str, &[&str]); 4] = [
        ("PNG image", &["png"]),
        ("JPEG image", &["jpg", "jpeg"]),
        ("Bitmap", &["bmp"]),
        ("GIF image", &["gif"]),
    ];
    let preferred = filters
        .iter()
        .position(|(_, e)| e.contains(&cfg.file_format.as_str()))
        .unwrap_or(0);
    dialog = dialog.add_filter(filters[preferred].0, filters[preferred].1);
    for (i, (name, exts)) in filters.iter().enumerate() {
        if i != preferred {
            dialog = dialog.add_filter(*name, exts);
        }
    }
    let Some(mut path) = dialog.save_file() else {
        return Ok(None);
    };
    if path.extension().is_none() {
        path.set_extension(&cfg.file_format);
    }
    img.save(&path, cfg.jpeg_quality)?;
    Ok(Some(path))
}

/// Linux selections die with their owner, so a short-lived process cannot
/// simply set the clipboard and exit. A detached helper (`krab
/// --hold-clipboard`) serves the data until something else is copied.
#[cfg(target_os = "linux")]
fn spawn_holder(kind: &str, payload: &[u8]) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!(
        "krab-clip-{}-{}",
        std::process::id(),
        chrono::Local::now().timestamp_millis()
    ));
    std::fs::write(&path, payload).map_err(|e| e.to_string())?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(exe)
        .arg("--hold-clipboard")
        .arg(kind)
        .arg(&path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// Body of the `--hold-clipboard` helper process.
#[cfg(target_os = "linux")]
pub fn hold_clipboard(kind: &str, path: &Path) -> Result<(), String> {
    use arboard::SetExtLinux;
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let _ = std::fs::remove_file(path);
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    if kind == "text" {
        let text = String::from_utf8(data).map_err(|e| e.to_string())?;
        cb.set().wait().text(text).map_err(|e| e.to_string())
    } else if kind == "rgba" {
        // Width and height, then the raw pixels: no PNG round trip.
        if data.len() < 8 {
            return Err("truncated image".into());
        }
        let w = u32::from_le_bytes(data[0..4].try_into().unwrap()) as usize;
        let h = u32::from_le_bytes(data[4..8].try_into().unwrap()) as usize;
        let px = data[8..].to_vec();
        if px.len() != w * h * 4 {
            return Err("truncated image".into());
        }
        cb.set()
            .wait()
            .image(arboard::ImageData {
                width: w,
                height: h,
                bytes: Cow::Owned(px),
            })
            .map_err(|e| e.to_string())
    } else {
        let img = image::load_from_memory(&data)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        cb.set()
            .wait()
            .image(arboard::ImageData {
                width: w,
                height: h,
                bytes: Cow::Owned(img.into_raw()),
            })
            .map_err(|e| e.to_string())
    }
}

/// Clipboard work still running in the background.
static CLIPBOARD_JOBS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Whether a copy is still being completed in the background (the process
/// should not exit yet).
pub fn clipboard_busy() -> bool {
    CLIPBOARD_JOBS.load(std::sync::atomic::Ordering::SeqCst) > 0
}

pub fn copy_image(img: &Image) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::sync::atomic::Ordering;
        // The bitmap is on the clipboard right away; the PNG many programs
        // prefer (it keeps transparency) follows once it is encoded.
        let seq = crate::platform::clipboard::set_image(img.w, img.h, &img.rgba)?;
        let img = img.clone();
        CLIPBOARD_JOBS.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            if let Ok(png) = img.encode("png", 100) {
                crate::platform::clipboard::add_png(&png, seq);
            }
            CLIPBOARD_JOBS.fetch_sub(1, Ordering::SeqCst);
        });
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let mut raw = Vec::with_capacity(8 + img.rgba.len());
        raw.extend_from_slice(&img.w.to_le_bytes());
        raw.extend_from_slice(&img.h.to_le_bytes());
        raw.extend_from_slice(&img.rgba);
        spawn_holder("rgba", &raw)
    }
    #[cfg(target_os = "macos")]
    {
        arboard::Clipboard::new()
            .and_then(|mut cb| {
                cb.set_image(arboard::ImageData {
                    width: img.w as usize,
                    height: img.h as usize,
                    bytes: Cow::Borrowed(&img.rgba),
                })
            })
            .map_err(|e| e.to_string())
    }
}

pub fn copy_text(text: &str) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        spawn_holder("text", text.as_bytes())
    }
    #[cfg(not(target_os = "linux"))]
    {
        arboard::Clipboard::new()
            .and_then(|mut cb| cb.set_text(text.to_string()))
            .map_err(|e| e.to_string())
    }
}

/// Open a file or folder with the system default handler.
pub fn open_path(path: &Path) {
    let _ = open::that_detached(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Image {
        let mut rgba = Vec::new();
        for i in 0..(8 * 6) {
            rgba.extend_from_slice(&[(i * 5) as u8, 100, 200, if i == 0 { 0 } else { 255 }]);
        }
        Image { w: 8, h: 6, rgba }
    }

    #[test]
    fn png_roundtrip_preserves_pixels_and_alpha() {
        let img = sample();
        let bytes = img.encode("png", 90).unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgba8();
        assert_eq!((back.width(), back.height()), (8, 6));
        assert_eq!(back.into_raw(), img.rgba);
    }

    #[test]
    fn every_format_produces_a_decodable_file() {
        let img = sample();
        for ext in ["jpg", "bmp", "gif"] {
            let bytes = img.encode(ext, 90).unwrap();
            let back = image::load_from_memory(&bytes).unwrap_or_else(|e| panic!("{ext}: {e}"));
            assert_eq!((back.width(), back.height()), (8, 6), "{ext}");
        }
    }

    #[test]
    fn transparent_pixels_flatten_to_white() {
        let rgb = sample().rgb_on_white();
        assert_eq!(&rgb[..3], &[255, 255, 255]);
        assert_eq!(&rgb[3..6], &[5, 100, 200]);
    }

    #[cfg(windows)]
    #[test]
    fn clipboard_bitmap_is_bottom_up_bgra() {
        let img = sample();
        let mut out = vec![0u8; 124 + img.rgba.len()];
        crate::platform::clipboard::dibv5(img.w, img.h, &img.rgba, &mut out);
        assert_eq!(u32::from_le_bytes(out[0..4].try_into().unwrap()), 124);
        assert_eq!(
            i32::from_le_bytes(out[8..12].try_into().unwrap()),
            6,
            "bottom-up"
        );
        // The first source row lands last; pixel (0,0) is fully transparent.
        let last_row = 124 + 5 * 8 * 4;
        assert_eq!(&out[last_row..last_row + 4], &[200, 100, 0, 0]);
        // Pixel (1,0): r=5, g=100, b=200 becomes B,G,R,A.
        assert_eq!(&out[last_row + 4..last_row + 8], &[200, 100, 5, 255]);
    }

    #[test]
    fn unique_path_avoids_existing_files() {
        let dir = std::env::temp_dir().join(format!("krab-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = unique_path(&dir, "Krab", "png");
        std::fs::write(&a, b"x").unwrap();
        let b = unique_path(&dir, "Krab", "png");
        // Same second -> must not collide; different second -> trivially distinct.
        assert_ne!(a, b);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

#[cfg(all(test, windows))]
mod clipboard_timing {
    /// `cargo test --release --lib clipboard_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn clipboard_cost() {
        use image::ImageEncoder;
        let (w, h) = (3840u32, 2160u32);
        let rgba: Vec<u8> = (0..w * h * 4)
            .map(|i| (i.wrapping_mul(2654435761) >> 13) as u8 | 0x0F)
            .collect();
        let t = std::time::Instant::now();
        let mut png = Vec::new();
        image::codecs::png::PngEncoder::new(&mut png)
            .write_image(&rgba, w, h, image::ExtendedColorType::Rgba8)
            .unwrap();
        let before = t.elapsed();
        let t = std::time::Instant::now();
        let mut dib = vec![0u8; 124 + rgba.len()];
        crate::platform::clipboard::dibv5(w, h, &rgba, &mut dib);
        let now = t.elapsed();
        println!("4K copy before clipboard is set: arboard PNG {before:?} vs DIB {now:?}");
    }
}
