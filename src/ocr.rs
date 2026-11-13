//! "Text actions": pull text and QR codes out of a snip.
//!
//! Windows uses the OCR engine that ships with the OS (no downloads, works
//! offline). On macOS and Linux Krab shells out to `tesseract` if installed.
//! QR codes are decoded in-process on every platform.

use crate::geom::RectF;
use crate::output::Image;

/// A recognized word and where it is (image pixels).
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: RectF,
}

/// The words of one line of text, left to right.
pub type Line = Vec<Word>;

fn lines_to_text(lines: &[Line]) -> String {
    lines
        .iter()
        .map(|l| {
            l.iter()
                .map(|w| w.text.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

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
        Ok(lines) => lines_to_text(&lines),
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

/// Recognize the text in `img`, line by line, with word positions.
#[cfg(windows)]
pub fn recognize(img: &Image) -> Result<Vec<Line>, String> {
    use windows::Graphics::Imaging::{BitmapPixelFormat, SoftwareBitmap};
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::Streams::DataWriter;

    let run = || -> windows::core::Result<Vec<Line>> {
        let max_dim = OcrEngine::MaxImageDimension()?;
        let k = upscale_factor(img.w, img.h, max_dim);
        let (w, h) = (img.w * k, img.h * k);
        if w > max_dim || h > max_dim {
            return Ok(Vec::new());
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
        let kf = k as f32;
        let mut lines = Vec::new();
        for line in result.Lines()? {
            let mut words = Vec::new();
            for word in line.Words()? {
                let r = word.BoundingRect()?;
                words.push(Word {
                    text: word.Text()?.to_string(),
                    rect: RectF::new(r.X / kf, r.Y / kf, r.Width / kf, r.Height / kf),
                });
            }
            lines.push(words);
        }
        Ok(lines)
    };
    run().map_err(|e| format!("Text recognition failed: {}", e.message()))
}

/// Recognize the text in `img`, line by line, with word positions.
#[cfg(not(windows))]
pub fn recognize(img: &Image) -> Result<Vec<Line>, String> {
    use std::process::{Command, Stdio};
    let path = std::env::temp_dir().join(format!("krab-ocr-{}.png", std::process::id()));
    img.save(&path, 100)?;
    let out = Command::new("tesseract")
        .arg(&path)
        .arg("stdout")
        .arg("tsv")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output();
    let _ = std::fs::remove_file(&path);
    match out {
        Ok(o) if o.status.success() => Ok(parse_tsv(&String::from_utf8_lossy(&o.stdout))),
        Ok(_) => Err("tesseract could not read this image".into()),
        Err(_) => {
            Err("Text recognition needs the `tesseract` command (install tesseract-ocr)".into())
        }
    }
}

/// Words from tesseract's TSV output, grouped into lines.
#[cfg_attr(windows, allow(dead_code))]
fn parse_tsv(tsv: &str) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut current: Option<(u32, u32, u32)> = None;
    for row in tsv.lines().skip(1) {
        let f: Vec<&str> = row.split('\t').collect();
        if f.len() < 12 || f[0] != "5" {
            continue;
        }
        let text = f[11].trim();
        if text.is_empty() {
            continue;
        }
        let num = |i: usize| f[i].parse::<f32>().unwrap_or(0.0);
        let key = (num(2) as u32, num(3) as u32, num(4) as u32);
        if current != Some(key) {
            lines.push(Vec::new());
            current = Some(key);
        }
        if let Some(line) = lines.last_mut() {
            line.push(Word {
                text: text.to_string(),
                rect: RectF::new(num(6), num(7), num(8), num(9)),
            });
        }
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tesseract_tsv_becomes_lines_of_words() {
        let tsv = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext\n\
                   4\t1\t1\t1\t1\t0\t10\t10\t200\t20\t-1\t\n\
                   5\t1\t1\t1\t1\t1\t10\t10\t50\t20\t96\tHello\n\
                   5\t1\t1\t1\t1\t2\t70\t10\t60\t20\t95\tKrab\n\
                   5\t1\t1\t1\t2\t1\t10\t40\t80\t20\t91\tsecond\n";
        let lines = parse_tsv(tsv);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0][1].text, "Krab");
        assert_eq!(lines[0][1].rect, RectF::new(70.0, 10.0, 60.0, 20.0));
        assert_eq!(lines_to_text(&lines), "Hello Krab\nsecond");
    }

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
