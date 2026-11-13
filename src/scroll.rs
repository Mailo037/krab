//! Scrolling capture: one tall image of something longer than the screen.
//!
//! The selected area is grabbed again and again while it scrolls - by
//! itself on Windows (Krab turns the mouse wheel), by the user elsewhere.
//! Consecutive frames are matched row by row to find how far the content
//! moved, and only the new rows are appended. Rows that stay put (sticky
//! headers and footers) are recognized and kept once.

use crate::capture::RegionGrabber;
use crate::geom::RectI;
use crate::output::Image;
use crate::platform;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

/// Taller results get unwieldy (and huge in memory).
pub const MAX_HEIGHT: usize = 20_000;

/// Accumulates frames into one image.
pub struct Stitcher {
    w: usize,
    h: usize,
    /// Columns left out when comparing rows (scroll bars move).
    cmp_w: usize,
    prev: Vec<u32>,
    prev_rows: Vec<u64>,
    /// Everything above the footer so far.
    body: Vec<u32>,
    /// Rows at the bottom that never scroll.
    footer: usize,
    moved: bool,
}

fn row_hashes(px: &[u32], w: usize, cmp_w: usize) -> Vec<u64> {
    px.chunks_exact(w)
        .map(|row| {
            // FNV-1a over the compared part of the row.
            let mut h: u64 = 0xcbf2_9ce4_8422_2325;
            for p in &row[..cmp_w] {
                h ^= (*p & 0x00FF_FFFF) as u64;
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
            h
        })
        .collect()
}

/// How the content moved between two frames: `(shift, top, bottom)` where
/// rows `top..h-bottom` scrolled up by `shift` and the rest stood still.
/// `None` when the frames do not line up.
pub fn find_shift(prev: &[u64], next: &[u64]) -> Option<(usize, usize, usize)> {
    let h = prev.len();
    if h != next.len() || h == 0 {
        return None;
    }
    if prev == next {
        return Some((0, h, 0));
    }
    let top = prev.iter().zip(next).take_while(|(a, b)| a == b).count();
    let bottom = prev
        .iter()
        .rev()
        .zip(next.iter().rev())
        .take_while(|(a, b)| a == b)
        .count()
        .min(h - top);
    let (a, b) = (top, h - bottom);
    let m = b - a;
    if m < 8 {
        return None;
    }
    // Rows that occur often (blank lines) say nothing about the offset.
    let mut counts = std::collections::HashMap::new();
    for r in &prev[a..b] {
        *counts.entry(*r).or_insert(0u32) += 1;
    }
    let telling = |r: &u64| counts.get(r).is_some_and(|c| *c <= 2);
    let mut best = (0usize, 0usize);
    for shift in 1..m {
        let overlap = m - shift;
        if overlap < 8 {
            break;
        }
        let score = (0..overlap)
            .filter(|&i| prev[a + shift + i] == next[a + i] && telling(&next[a + i]))
            .count();
        if score > best.1 {
            best = (shift, score);
        }
    }
    let informative = prev[a..b].iter().filter(|r| telling(r)).count();
    (best.1 >= 6 && best.1 * 10 >= informative.min(m - best.0) * 3).then_some((best.0, top, bottom))
}

impl Stitcher {
    pub fn new(first: Vec<u32>, w: usize, h: usize) -> Stitcher {
        // Leave the right edge out of comparisons: that is where scroll
        // bars live, and their thumb moves with every step.
        let cmp_w = if w > 64 { w - 24 } else { w };
        Stitcher {
            w,
            h,
            cmp_w,
            prev_rows: row_hashes(&first, w, cmp_w),
            body: first.clone(),
            prev: first,
            footer: 0,
            moved: false,
        }
    }

    /// Rows collected so far.
    pub fn height(&self) -> usize {
        self.body.len() / self.w + self.footer
    }

    /// Add a frame. Returns how many new rows it brought.
    pub fn push(&mut self, frame: Vec<u32>) -> usize {
        if frame.len() != self.w * self.h {
            return 0;
        }
        let rows = row_hashes(&frame, self.w, self.cmp_w);
        let Some((shift, _top, bottom)) = find_shift(&self.prev_rows, &rows) else {
            return 0;
        };
        if shift == 0 {
            return 0;
        }
        let w = self.w;
        if !self.moved {
            // Now the footer is known: keep only what is above it.
            self.body.truncate((self.h - bottom) * w);
            self.footer = bottom;
            self.moved = true;
        }
        let end = self.h - self.footer.min(bottom);
        let start = end.saturating_sub(shift);
        let room = MAX_HEIGHT.saturating_sub(self.height());
        let take = (end - start).min(room);
        self.body
            .extend_from_slice(&frame[start * w..(start + take) * w]);
        self.prev = frame;
        self.prev_rows = rows;
        take
    }

    pub fn is_full(&self) -> bool {
        self.height() >= MAX_HEIGHT
    }

    pub fn finish(mut self) -> Image {
        if self.moved && self.footer > 0 {
            let w = self.w;
            let from = (self.h - self.footer) * w;
            self.body.extend_from_slice(&self.prev[from..]);
        }
        let h = (self.body.len() / self.w) as u32;
        Image {
            w: self.w as u32,
            h,
            rgba: crate::gfx::argb_to_rgba(&self.body),
        }
    }
}

/// Capture `local` (pixels of monitor `monitor`) while it scrolls, until
/// `stop` is set, the end is reached or the image is as tall as allowed.
/// `height` reports the progress.
pub fn capture(
    monitor: usize,
    native: RectI,
    local: RectI,
    stop: &AtomicBool,
    height: &AtomicU32,
) -> Result<Image, String> {
    let mut grabber = RegionGrabber::new(monitor, native, local, false)?;
    let (w, h) = (grabber.w as usize, grabber.h as usize);
    // Let the overlay disappear from the screen first.
    std::thread::sleep(Duration::from_millis(220));
    let mut frame = Vec::new();
    grabber.grab(&mut frame)?;
    let mut st = Stitcher::new(frame, w, h);
    height.store(st.height() as u32, Ordering::Relaxed);

    // Scroll by ourselves where the platform allows it.
    let center = (
        native.x + local.x + local.w / 2,
        native.y + local.y + local.h / 2,
    );
    let auto = platform::scroll_wheel_at(center, 0);
    let mut still = 0;
    let started = Instant::now();
    while !stop.load(Ordering::SeqCst) && !st.is_full() {
        if auto {
            platform::scroll_wheel_at(center, -2);
            std::thread::sleep(Duration::from_millis(170));
        } else {
            std::thread::sleep(Duration::from_millis(120));
        }
        let mut frame = Vec::new();
        grabber.grab(&mut frame)?;
        let added = st.push(frame);
        height.store(st.height() as u32, Ordering::Relaxed);
        if added == 0 {
            still += 1;
            // The page stopped moving: that was the end.
            if auto && still >= 4 {
                break;
            }
        } else {
            still = 0;
        }
        if started.elapsed() > Duration::from_secs(300) {
            break;
        }
    }
    Ok(st.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tall "page"; `view(y)` is what a 60-row window with a sticky
    /// 6-row header and 5-row footer shows when scrolled to `y`.
    fn page() -> (Vec<u32>, usize, usize) {
        let (w, h) = (80usize, 400usize);
        let px = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as u32, (i / w) as u32);
                0xFF00_0000 | (y.wrapping_mul(2_654_435_761) ^ x.wrapping_mul(40_503)) & 0x00FF_FFFF
            })
            .collect();
        (px, w, h)
    }

    fn view(page: &[u32], w: usize, y: usize) -> Vec<u32> {
        let (header, footer, rows) = (6usize, 5usize, 60usize);
        (0..rows)
            .flat_map(|r| {
                (0..w).map(move |x| {
                    if r < header {
                        0xFF10_2030 + (r * w + x) as u32
                    } else if r >= rows - footer {
                        0xFF80_0000 + (r * w + x) as u32 * 3
                    } else {
                        page[(y + r) * w + x]
                    }
                })
            })
            .collect()
    }

    #[test]
    fn stitching_rebuilds_the_page_once() {
        let (px, w, _) = page();
        let mut st = Stitcher::new(view(&px, w, 0), w, 60);
        let mut y = 0;
        for step in [13usize, 17, 0, 9, 20, 11] {
            y += step;
            st.push(view(&px, w, y));
        }
        let img = st.finish();
        // Header + every body row seen + footer, no row twice.
        assert_eq!(img.h as usize, 60 + y);
        let row = |img: &Image, r: usize| img.rgba[r * w * 4..(r + 1) * w * 4].to_vec();
        let expect = Image {
            w: w as u32,
            h: 60,
            rgba: crate::gfx::argb_to_rgba(&view(&px, w, y)),
        };
        // The last body row and the footer match the final view.
        assert_eq!(row(&img, img.h as usize - 1), row(&expect, 59));
        assert_eq!(row(&img, img.h as usize - 6), row(&expect, 54));
    }

    #[test]
    fn identical_frames_add_nothing() {
        let (px, w, _) = page();
        let mut st = Stitcher::new(view(&px, w, 30), w, 60);
        assert_eq!(st.push(view(&px, w, 30)), 0);
        assert_eq!(st.finish().h, 60);
    }

    #[test]
    fn shift_is_found_despite_blank_rows() {
        let mut prev: Vec<u64> = (0..100)
            .map(|i| if i % 3 == 0 { 7 } else { i as u64 * 1000 })
            .collect();
        prev[0] = 1;
        let next: Vec<u64> = (0..100)
            .map(|i| prev.get(i + 12).copied().unwrap_or(99_999 + i as u64))
            .collect();
        let (shift, _, _) = find_shift(&prev, &next).unwrap();
        assert_eq!(shift, 12);
    }
}
