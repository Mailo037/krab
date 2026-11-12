//! Screen recording of a region.
//!
//! Frames are grabbed on a worker thread and streamed straight into an
//! encoder, so memory use stays flat no matter how long the recording runs.
//! With `ffmpeg` on the PATH the result is an H.264 MP4; without it Krab
//! falls back to an animated GIF, which needs no external tools.

use crate::capture::RegionGrabber;
use crate::geom::RectI;
use crate::output::unique_path;
use crate::platform;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub struct Recorder {
    stop: Arc<AtomicBool>,
    pub started: Instant,
}

impl Recorder {
    /// Start recording `local` (pixels, relative to monitor `monitor`).
    /// `on_done` runs on the worker thread once the file is finalized.
    pub fn start(
        monitor: usize,
        monitor_native: RectI,
        local: RectI,
        fps: u32,
        dir: PathBuf,
        on_done: impl FnOnce(Result<PathBuf, String>) + Send + 'static,
    ) -> Recorder {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            on_done(run(
                monitor,
                monitor_native,
                even(local),
                fps.max(1),
                &dir,
                &flag,
            ));
        });
        Recorder {
            stop,
            started: Instant::now(),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn is_stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }
}

/// H.264 with 4:2:0 chroma needs even dimensions.
fn even(r: RectI) -> RectI {
    RectI::new(r.x, r.y, (r.w & !1).max(2), (r.h & !1).max(2))
}

pub fn ffmpeg_available() -> bool {
    let mut cmd = Command::new("ffmpeg");
    cmd.arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    platform::hide_console(&mut cmd);
    cmd.status().map(|s| s.success()).unwrap_or(false)
}

fn run(
    monitor: usize,
    native: RectI,
    local: RectI,
    fps: u32,
    dir: &Path,
    stop: &AtomicBool,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let mut grabber = RegionGrabber::new(monitor, native, local)?;
    let (w, h) = (grabber.w, grabber.h);
    let mut grab = |frame: &mut Vec<u32>| grabber.grab(frame);
    if ffmpeg_available() {
        encode_mp4(&mut grab, w, h, fps, &unique_path(dir, "Krab", "mp4"), stop)
    } else {
        encode_gif(
            &mut grab,
            w,
            h,
            fps.min(12),
            &unique_path(dir, "Krab", "gif"),
            stop,
        )
    }
}

/// Where frames come from: fills the buffer with `w * h` `0xFFRRGGBB` pixels.
pub type FrameSource<'a> = &'a mut dyn FnMut(&mut Vec<u32>) -> Result<(), String>;

/// Stream frames into ffmpeg until `stop` is set.
pub fn encode_mp4(
    grab: FrameSource,
    w: u32,
    h: u32,
    fps: u32,
    path: &Path,
    stop: &AtomicBool,
) -> Result<PathBuf, String> {
    let path = path.to_path_buf();
    let mut cmd = Command::new("ffmpeg");
    cmd.args([
        "-y",
        "-loglevel",
        "error",
        "-f",
        "rawvideo",
        "-pixel_format",
        "bgra",
    ])
    .args(["-video_size", &format!("{w}x{h}")])
    .args(["-framerate", &fps.to_string(), "-i", "-"])
    .args([
        "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p",
    ])
    .args(["-movflags", "+faststart"])
    .arg(&path)
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    platform::hide_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("could not start ffmpeg: {e}"))?;
    let mut pipe = child.stdin.take().ok_or("ffmpeg stdin unavailable")?;

    let frame_time = Duration::from_secs_f64(1.0 / fps as f64);
    let start = Instant::now();
    let mut frame = Vec::new();
    let mut written: u64 = 0;
    let mut result = Ok(());
    while !stop.load(Ordering::SeqCst) {
        if let Err(e) = grab(&mut frame) {
            result = Err(e);
            break;
        }
        // The stream has a fixed frame rate: if grabbing fell behind, repeat
        // the frame so playback speed stays true to wall-clock time.
        let due = (start.elapsed().as_secs_f64() * fps as f64) as u64 + 1;
        let copies = due.saturating_sub(written).clamp(1, fps as u64);
        let bytes: &[u8] = bytemuck::cast_slice(&frame);
        let mut failed = false;
        for _ in 0..copies {
            if pipe.write_all(bytes).is_err() {
                failed = true;
                break;
            }
            written += 1;
        }
        if failed {
            result = Err("ffmpeg stopped unexpectedly (is libx264 available?)".to_string());
            break;
        }
        let next = frame_time * written as u32;
        if let Some(wait) = next.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    drop(pipe);
    let status = child.wait().map_err(|e| e.to_string())?;
    result?;
    if status.success() && path.exists() {
        Ok(path)
    } else {
        Err("ffmpeg could not encode the recording".into())
    }
}

/// Encode frames as an animated GIF until `stop` is set.
pub fn encode_gif(
    grab: FrameSource,
    w: u32,
    h: u32,
    fps: u32,
    path: &Path,
    stop: &AtomicBool,
) -> Result<PathBuf, String> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, RgbaImage};

    let path = path.to_path_buf();
    let file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
    let mut enc = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 30);
    enc.set_repeat(Repeat::Infinite)
        .map_err(|e| e.to_string())?;

    let frame_time = Duration::from_secs_f64(1.0 / fps as f64);
    let mut frame = Vec::new();
    let mut last = Instant::now();
    let mut pending: Option<RgbaImage> = None;
    loop {
        let tick = Instant::now();
        let stopping = stop.load(Ordering::SeqCst);
        // A frame's delay is only known once the next one is taken, so each
        // frame is written one iteration late.
        if let Some(img) = pending.take() {
            let delay = Delay::from_saturating_duration(
                tick.duration_since(last).max(Duration::from_millis(20)),
            );
            enc.encode_frame(Frame::from_parts(img, 0, 0, delay))
                .map_err(|e| e.to_string())?;
        }
        if stopping {
            break;
        }
        last = tick;
        grab(&mut frame)?;
        let rgba = crate::gfx::argb_to_rgba(&frame);
        pending = RgbaImage::from_raw(w, h, rgba);
        if let Some(wait) = frame_time.checked_sub(tick.elapsed()) {
            std::thread::sleep(wait);
        }
    }
    drop(enc);
    Ok(path)
}

/// `m:ss` for the recording HUD.
pub fn format_elapsed(d: Duration) -> String {
    let s = d.as_secs();
    format!("{}:{:02}", s / 60, s % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_are_made_even() {
        assert_eq!(even(RectI::new(3, 5, 101, 77)), RectI::new(3, 5, 100, 76));
        assert_eq!(even(RectI::new(0, 0, 1, 1)), RectI::new(0, 0, 2, 2));
    }

    /// A moving bar, so consecutive frames differ.
    fn synthetic(w: u32, h: u32) -> impl FnMut(&mut Vec<u32>) -> Result<(), String> {
        let mut n = 0u32;
        move |frame: &mut Vec<u32>| {
            frame.clear();
            frame.extend((0..w * h).map(|i| {
                if (i % w + n * 7) % w < 20 {
                    0xFFF4_6623
                } else {
                    0xFF20_2228
                }
            }));
            n += 1;
            Ok(())
        }
    }

    fn stop_after(ms: u64) -> Arc<AtomicBool> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            flag.store(true, Ordering::SeqCst);
        });
        stop
    }

    #[test]
    fn gif_recording_has_several_decodable_frames() {
        use image::AnimationDecoder;
        let path = std::env::temp_dir().join(format!("krab-rec-test-{}.gif", std::process::id()));
        let mut src = synthetic(64, 48);
        let out = encode_gif(&mut src, 64, 48, 12, &path, &stop_after(500)).unwrap();
        let file = std::io::BufReader::new(std::fs::File::open(&out).unwrap());
        let frames = image::codecs::gif::GifDecoder::new(file)
            .unwrap()
            .into_frames()
            .collect_frames()
            .unwrap();
        std::fs::remove_file(&out).unwrap();
        assert!(frames.len() >= 3, "only {} frames", frames.len());
        assert_eq!(frames[0].buffer().dimensions(), (64, 48));
        assert_ne!(frames[0].buffer().as_raw(), frames[1].buffer().as_raw());
    }

    #[test]
    fn mp4_recording_produces_a_playable_file() {
        if !ffmpeg_available() {
            eprintln!("skipped: ffmpeg is not installed");
            return;
        }
        let path = std::env::temp_dir().join(format!("krab-rec-test-{}.mp4", std::process::id()));
        let mut src = synthetic(128, 96);
        let out = encode_mp4(&mut src, 128, 96, 30, &path, &stop_after(700)).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        std::fs::remove_file(&out).unwrap();
        assert!(
            bytes.len() > 500,
            "suspiciously small: {} bytes",
            bytes.len()
        );
        assert_eq!(&bytes[4..8], b"ftyp", "not an MP4 container");
        // +faststart moves the index to the front; its presence means ffmpeg
        // finalized the file instead of being cut off.
        assert!(bytes.windows(4).any(|w| w == b"moov"));
    }

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "0:00");
        assert_eq!(format_elapsed(Duration::from_secs(75)), "1:15");
        assert_eq!(format_elapsed(Duration::from_secs(600)), "10:00");
    }
}
