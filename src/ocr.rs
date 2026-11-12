//! "Text actions": pull text and QR codes out of a snip.
//!
//! Windows uses the OCR engine that ships with the OS (no downloads, works
//! offline). On macOS and Linux Krab shells out to `tesseract` if installed.
//! QR codes are decoded in-process on every platform.

use crate::output::Image;

/// Luma of a pixel, with transparency flattened onto white.
fn luma(p: &[u8]) -> u8 {
    let a = p[3] as u32;
    let f = |c: u8| (c as u32 * a + 255 * (255 - a)) / 255;
    ((f(p[0]) * 299 + f(p[1]) * 587 + f(p[2]) * 114) / 1000) as u8
}

/// Decode every QR code in the image.
pub fn read_qr_codes(img: &Image) -> Vec<String> {
    let (w, h) = (img.w as usize, img.h as usize);
    if w < 21 || h < 21 {
        return Vec::new();
    }
    let mut prepared = rqrr::PreparedImage::prepare_from_greyscale(w, h, |x, y| {
        let i = (y * w + x) * 4;
        luma(&img.rgba[i..i + 4])
    });
    prepared
        .detect_grids()
        .into_iter()
        .filter_map(|grid| grid.decode().ok().map(|(_, content)| content))
        .collect()
}

/// Recognized text followed by the payload of any QR codes.
pub fn extract_text(img: &Image) -> Result<String, String> {
    let codes = read_qr_codes(img);
    let text = match recognize(img) {
        Ok(t) => t,
        Err(e) if codes.is_empty() => return Err(e),
        Err(_) => String::new(),
    };
    Ok(merge(text.trim(), &codes))
}

fn merge(text: &str, codes: &[String]) -> String {
    let mut out = text.to_string();
    for code in codes {
        if !out.contains(code.as_str()) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(code);
        }
    }
    out
}

/// OCR engines do much better on large glyphs; screen text is small.
#[cfg_attr(not(windows), allow(dead_code))]
fn upscale_factor(w: u32, h: u32, max_dim: u32) -> u32 {
    let longest = w.max(h).max(1);
    if longest * 3 <= max_dim && longest < 900 {
        3
    } else if longest * 2 <= max_dim && longest < 2600 {
        2
    } else {
        1
    }
}

#[cfg(windows)]
fn recognize(img: &Image) -> Result<String, String> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    let run = || -> windows::core::Result<String> {
        let max_dim = OcrEngine::MaxImageDimension()?;
        let k = upscale_factor(img.w, img.h, max_dim);
        let (w, h) = (img.w * k, img.h * k);
        if w > max_dim || h > max_dim {
            return Ok(String::new());
        }
        // BGRA, flattened on white, nearest-neighbour upscaled.
        let mut bgra = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            let row = ((y / k) * img.w) as usize;
            for x in 0..w {
                let i = (row + (x / k) as usize) * 4;
                let p = &img.rgba[i..i + 4];
                let a = p[3] as u32;
                let f = |c: u8| ((c as u32 * a + 255 * (255 - a)) / 255) as u8;
                bgra.extend_from_slice(&[f(p[2]), f(p[1]), f(p[0]), 255]);
            }
        }
        let writer = DataWriter::new()?;
        writer.WriteBytes(&bgra)?;
        let buffer = writer.DetachBuffer()?;
        let bitmap = SoftwareBitmap::CreateCopyFromBuffer(
            &buffer,
            BitmapPixelFormat::Bgra8,
            w as i32,
            h as i32,
        )?;
        let engine = OcrEngine::TryCreateFromUserProfileLanguages()?;
        let result = engine.RecognizeAsync(&bitmap)?.get()?;
        let mut lines = Vec::new();
        for line in result.Lines()? {
            lines.push(line.Text()?.to_string());
        }
        Ok(lines.join("\n"))
    };
    run().map_err(|e| format!("Text recognition failed: {}", e.message()))
}

#[cfg(not(windows))]
fn recognize(img: &Image) -> Result<String, String> {
    use std::process::{Command, Stdio};
    let path = std::env::temp_dir().join(format!("krab-ocr-{}.png", std::process::id()));
    img.save(&path, 100)?;
    let out = Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let _ = std::fs::remove_file(&path);
    match out {
        Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).into_owned()),
        Ok(_) => Err("tesseract could not read this image".into()),
        Err(_) => {
            Err("Text recognition needs the `tesseract` command (install tesseract-ocr)".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_appends_only_new_codes() {
        assert_eq!(merge("hello", &[]), "hello");
        assert_eq!(merge("", &["https://a.b".into()]), "https://a.b");
        assert_eq!(
            merge("see https://a.b", &["https://a.b".into(), "X".into()]),
            "see https://a.b\nX"
        );
    }

    #[test]
    fn upscale_respects_engine_limit() {
        assert_eq!(upscale_factor(300, 100, 10_000), 3);
        assert_eq!(upscale_factor(1920, 1080, 10_000), 2);
        assert_eq!(upscale_factor(4000, 2000, 10_000), 1);
        assert_eq!(upscale_factor(800, 600, 1000), 1);
    }

    #[test]
    fn blank_image_has_no_qr_codes() {
        let img = Image {
            w: 64,
            h: 64,
            rgba: vec![255; 64 * 64 * 4],
        };
        assert!(read_qr_codes(&img).is_empty());
    }
}
