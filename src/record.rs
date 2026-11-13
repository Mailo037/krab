//! Screen recording of a region.
//!
//! Frames are grabbed on a worker thread and streamed straight into an
//! encoder, so memory use stays flat no matter how long the recording runs.
//! With `ffmpeg` on the PATH the result is an H.264 MP4 - encoded on the
//! graphics card when it can, optionally with sound; without it Krab falls
//! back to an animated GIF, which needs no external tools.

use crate::audio;
use crate::capture::RegionGrabber;
use crate::geom::{Pt, RectI};
use crate::gfx::{theme, Canvas, Color};
use crate::output::unique_path;
use crate::platform;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Shared between the recording thread and the UI: stop, pause and the
/// recording clock (which stands still while paused).
pub struct Control {
    stop: AtomicBool,
    paused: AtomicBool,
    start: Instant,
    /// Pauses as offsets from `start`; the last one is open while paused.
    pauses: Mutex<Vec<(Duration, Option<Duration>)>>,
}

impl Default for Control {
    fn default() -> Self {
        Self::new()
    }
}

impl Control {
    pub fn new() -> Control {
        Self::started_at(Instant::now())
    }

    /// A clock that started at `start` (for previews of a running recording).
    pub fn started_at(start: Instant) -> Control {
        Control {
            stop: AtomicBool::new(false),
            paused: AtomicBool::new(false),
            start,
            pauses: Mutex::new(Vec::new()),
        }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }

    pub fn is_stopping(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::SeqCst)
    }

    pub fn set_paused(&self, on: bool) {
        let mut pauses = self.pauses.lock().unwrap();
        let now = self.start.elapsed();
        if on && !self.is_paused() {
            pauses.push((now, None));
        } else if !on && self.is_paused() {
            if let Some(last) = pauses.last_mut() {
                last.1 = Some(now);
            }
        }
        self.paused.store(on, Ordering::SeqCst);
    }

    /// Pause or resume; returns whether it is paused now.
    pub fn toggle_pause(&self) -> bool {
        let on = !self.is_paused();
        self.set_paused(on);
        on
    }

    /// Recorded time: wall time minus pauses.
    pub fn elapsed(&self) -> Duration {
        let pauses = self.pauses.lock().unwrap();
        let mut now = self.start.elapsed();
        let mut paused = Duration::ZERO;
        for (from, to) in pauses.iter() {
            match to {
                Some(to) => paused += *to - *from,
                None => now = now.min(*from),
            }
        }
        now.saturating_sub(paused)
    }

    /// Finished pauses in seconds since the start (wall time).
    pub fn pauses(&self) -> Vec<(f64, f64)> {
        let now = self.start.elapsed();
        self.pauses
            .lock()
            .unwrap()
            .iter()
            .map(|(a, b)| (a.as_secs_f64(), b.unwrap_or(now).as_secs_f64()))
            .collect()
    }
}

/// How to record.
#[derive(Clone, Debug)]
pub struct Settings {
    pub fps: u32,
    pub dir: PathBuf,
    /// Show the mouse pointer in the video.
    pub cursor: bool,
    /// Highlight the pointer and its clicks.
    pub clicks: bool,
    pub audio: audio::Source,
    /// "auto" uses a hardware encoder when one works, "software" never does.
    pub encoder: String,
}

pub struct Recorder {
    pub control: Arc<Control>,
}

impl Recorder {
    /// Start recording `local` (pixels, relative to monitor `monitor`).
    /// `on_done` runs on the worker thread once the file is finalized.
    pub fn start(
        monitor: usize,
        monitor_native: RectI,
        local: RectI,
        settings: Settings,
        on_done: impl FnOnce(Result<PathBuf, String>) + Send + 'static,
    ) -> Recorder {
        let control = Arc::new(Control::new());
        let ctl = control.clone();
        std::thread::spawn(move || {
            on_done(run(monitor, monitor_native, even(local), &settings, &ctl));
        });
        Recorder { control }
    }

    pub fn stop(&self) {
        self.control.stop();
    }

    pub fn is_stopping(&self) -> bool {
        self.control.is_stopping()
    }
}

/// H.264 with 4:2:0 chroma needs even dimensions.
fn even(r: RectI) -> RectI {
    RectI::new(r.x, r.y, (r.w & !1).max(2), (r.h & !1).max(2))
}

/// The H.264 encoders Krab knows how to drive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoder {
    Software,
    /// NVIDIA
    Nvenc,
    /// Intel Quick Sync
    Qsv,
    /// AMD
    Amf,
    /// Apple
    VideoToolbox,
}

impl Encoder {
    fn args(self) -> &'static [&'static str] {
        match self {
            Encoder::Software => &[
                "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p",
            ],
            Encoder::Nvenc => &[
                "-c:v",
                "h264_nvenc",
                "-preset",
                "p4",
                "-rc",
                "vbr",
                "-cq",
                "21",
                "-b:v",
                "0",
                "-pix_fmt",
                "yuv420p",
            ],
            Encoder::Qsv => &[
                "-c:v",
                "h264_qsv",
                "-preset",
                "faster",
                "-global_quality",
                "22",
                "-pix_fmt",
                "nv12",
            ],
            Encoder::Amf => &[
                "-c:v", "h264_amf", "-quality", "speed", "-rc", "cqp", "-qp_i", "20", "-qp_p",
                "22", "-pix_fmt", "yuv420p",
            ],
            Encoder::VideoToolbox => &[
                "-c:v",
                "h264_videotoolbox",
                "-b:v",
                "12M",
                "-pix_fmt",
                "yuv420p",
            ],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Encoder::Software => "x264 (CPU)",
            Encoder::Nvenc => "NVIDIA NVENC",
            Encoder::Qsv => "Intel Quick Sync",
            Encoder::Amf => "AMD AMF",
            Encoder::VideoToolbox => "Apple VideoToolbox",
        }
    }

    /// Hardware encoders worth trying on this platform, best first.
    fn candidates() -> &'static [Encoder] {
        if cfg!(windows) {
            &[Encoder::Nvenc, Encoder::Qsv, Encoder::Amf]
        } else if cfg!(target_os = "macos") {
            &[Encoder::VideoToolbox]
        } else {
            &[Encoder::Nvenc]
        }
    }
}

fn ffmpeg() -> Command {
    let mut cmd = Command::new("ffmpeg");
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    platform::hide_console(&mut cmd);
    cmd
}

/// Run `cmd`, giving up after `limit`.
fn succeeds_within(mut cmd: Command, limit: Duration) -> bool {
    let Ok(mut child) = cmd.spawn() else {
        return false;
    };
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// Whether `ffmpeg` can be run (checked once).
pub fn ffmpeg_available() -> bool {
    static AVAILABLE: OnceLock<bool> = OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let mut cmd = ffmpeg();
        cmd.arg("-version");
        succeeds_within(cmd, Duration::from_secs(5))
    })
}

/// The best encoder that actually works here, or `None` without ffmpeg.
/// Hardware encoders are listed by ffmpeg builds whether or not the
/// hardware is present, so each one is tried on a few blank frames. The
/// result is cached; [`warm_up`] gets it ready before it is needed.
pub fn best_encoder() -> Option<Encoder> {
    static BEST: OnceLock<Option<Encoder>> = OnceLock::new();
    *BEST.get_or_init(|| {
        if !ffmpeg_available() {
            return None;
        }
        let hw = Encoder::candidates().iter().copied().find(|enc| {
            let mut cmd = ffmpeg();
            cmd.args(["-hide_banner", "-loglevel", "error"])
                .args(["-f", "lavfi", "-i", "color=c=black:s=320x240:r=30"])
                .args(["-frames:v", "5"])
                .args(enc.args())
                .args(["-f", "null", "-"]);
            succeeds_within(cmd, Duration::from_secs(8))
        });
        Some(hw.unwrap_or(Encoder::Software))
    })
}

/// Find the encoder in the background (a recording is about to start).
pub fn warm_up() {
    std::thread::spawn(|| {
        best_encoder();
    });
}

fn choose_encoder(pref: &str, w: u32, h: u32) -> Option<Encoder> {
    let best = best_encoder()?;
    // Hardware encoders refuse very small frames.
    if pref.eq_ignore_ascii_case("software") || w < 160 || h < 128 {
        return Some(Encoder::Software);
    }
    Some(best)
}

fn run(
    monitor: usize,
    native: RectI,
    local: RectI,
    s: &Settings,
    control: &Arc<Control>,
) -> Result<PathBuf, String> {
    std::fs::create_dir_all(&s.dir).map_err(|e| e.to_string())?;
    let mut grabber = RegionGrabber::new(monitor, native, local, s.cursor)?;
    let (w, h) = (grabber.w, grabber.h);
    let origin = (native.x + local.x, native.y + local.y);
    let mut pointer = PointerMarks::new(s.clicks, origin);
    let mut grab = |frame: &mut Vec<u32>| {
        grabber.grab(frame)?;
        pointer.draw(frame, w, h);
        Ok(())
    };
    let fps = s.fps.max(1);
    let Some(enc) = choose_encoder(&s.encoder, w, h) else {
        let path = unique_path(&s.dir, "Krab", "gif");
        return encode_gif(&mut grab, w, h, fps.min(12), &path, control);
    };

    let path = unique_path(&s.dir, "Krab", "mp4");
    let audio = (s.audio != audio::Source::Off)
        .then(|| audio::Capture::start(s.audio, &s.dir, control.clone()));
    let video = match &audio {
        Some(_) => s
            .dir
            .join(format!(".krab-video-{}.mp4", std::process::id())),
        None => path.clone(),
    };
    let mut result = encode_mp4(&mut grab, w, h, fps, &video, control, enc);
    if let Err(e) = &result {
        // A hardware encoder that fails right away: carry on in software.
        if enc != Encoder::Software && e.frames < 2 * fps as u64 && !control.is_stopping() {
            eprintln!("krab: {} failed ({}), using x264", enc.label(), e.msg);
            result = encode_mp4(&mut grab, w, h, fps, &video, control, Encoder::Software);
        }
    }
    let tracks = audio.map(|a| a.finish()).unwrap_or_default();
    let cleanup = |tracks: &[audio::Track]| {
        for t in tracks {
            let _ = std::fs::remove_file(&t.path);
        }
    };
    if let Err(e) = result {
        cleanup(&tracks);
        let _ = std::fs::remove_file(&video);
        return Err(e.msg);
    }
    if video == path {
        return Ok(path);
    }
    if !tracks.is_empty() {
        match audio::mux(&video, &tracks, &path) {
            Ok(()) => {
                cleanup(&tracks);
                let _ = std::fs::remove_file(&video);
                return Ok(path);
            }
            Err(e) => eprintln!("krab: could not add the sound: {e}"),
        }
    }
    cleanup(&tracks);
    std::fs::rename(&video, &path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Where frames come from: fills the buffer with `w * h` `0xFFRRGGBB` pixels.
pub type FrameSource<'a> = &'a mut dyn FnMut(&mut Vec<u32>) -> Result<(), String>;

pub struct EncodeError {
    pub msg: String,
    /// Frames handed to the encoder before it failed.
    pub frames: u64,
}

/// Stream frames into ffmpeg until `control` says stop.
pub fn encode_mp4(
    grab: FrameSource,
    w: u32,
    h: u32,
    fps: u32,
    path: &Path,
    control: &Control,
    enc: Encoder,
) -> Result<PathBuf, EncodeError> {
    let fail = |msg: String, frames: u64| EncodeError { msg, frames };
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
    .args(enc.args())
    .args(["-movflags", "+faststart"])
    .arg(&path)
    .stdin(Stdio::piped())
    .stdout(Stdio::null())
    .stderr(Stdio::null());
    platform::hide_console(&mut cmd);
    let mut child = cmd
        .spawn()
        .map_err(|e| fail(format!("could not start ffmpeg: {e}"), 0))?;
    let mut pipe = child
        .stdin
        .take()
        .ok_or_else(|| fail("ffmpeg stdin unavailable".into(), 0))?;

    let frame_time = Duration::from_secs_f64(1.0 / fps as f64);
    // The stream's clock: recording time since this encoder started.
    let t0 = control.elapsed();
    let clock = || control.elapsed().saturating_sub(t0);
    let mut frame = Vec::new();
    let mut written: u64 = 0;
    let mut result = Ok(());
    while !control.is_stopping() {
        if control.is_paused() {
            std::thread::sleep(Duration::from_millis(15));
            continue;
        }
        if let Err(e) = grab(&mut frame) {
            result = Err(fail(e, written));
            break;
        }
        // The stream has a fixed frame rate: if grabbing fell behind, repeat
        // the frame so playback speed stays true to the recorded time.
        let due = (clock().as_secs_f64() * fps as f64) as u64 + 1;
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
            result = Err(fail(
                format!("ffmpeg stopped unexpectedly ({})", enc.label()),
                written,
            ));
            break;
        }
        let next = frame_time * written as u32;
        if let Some(wait) = next.checked_sub(clock()) {
            std::thread::sleep(wait);
        }
    }
    drop(pipe);
    let status = child.wait().map_err(|e| fail(e.to_string(), written))?;
    result?;
    if status.success() && path.exists() {
        Ok(path)
    } else {
        Err(fail(
            "ffmpeg could not encode the recording".into(),
            written,
        ))
    }
}

/// Encode frames as an animated GIF until `control` says stop.
///
/// Colour quantization is slow, so it runs on its own thread: the grabbing
/// loop keeps its pace and, when the encoder falls behind, frames are
/// skipped (the previous one simply stays up longer) instead of piling up.
pub fn encode_gif(
    grab: FrameSource,
    w: u32,
    h: u32,
    fps: u32,
    path: &Path,
    control: &Control,
) -> Result<PathBuf, String> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, Frame, RgbaImage};

    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let frame_time = Duration::from_secs_f64(1.0 / fps as f64);
    let (tx, rx) = mpsc::sync_channel::<(Vec<u32>, Duration)>(3);
    let encoder = std::thread::spawn(move || -> Result<(), String> {
        let mut enc = GifEncoder::new_with_speed(std::io::BufWriter::new(file), 30);
        enc.set_repeat(Repeat::Infinite)
            .map_err(|e| e.to_string())?;
        let mut put = |px: &[u32], delay: Duration| -> Result<(), String> {
            let img =
                RgbaImage::from_raw(w, h, crate::gfx::argb_to_rgba(px)).ok_or("bad frame size")?;
            let delay = Delay::from_saturating_duration(delay.max(Duration::from_millis(20)));
            enc.encode_frame(Frame::from_parts(img, 0, 0, delay))
                .map_err(|e| e.to_string())
        };
        // A frame's delay is only known once the next one arrives.
        let mut pending: Option<(Vec<u32>, Duration)> = None;
        for (px, t) in rx {
            if let Some((prev, pt)) = pending.take() {
                put(&prev, t.saturating_sub(pt))?;
            }
            pending = Some((px, t));
        }
        if let Some((prev, _)) = pending {
            put(&prev, frame_time)?;
        }
        Ok(())
    });

    let t0 = control.elapsed();
    let mut grabbed = 0u32;
    let mut result = Ok(());
    while !control.is_stopping() {
        if control.is_paused() {
            std::thread::sleep(Duration::from_millis(15));
            continue;
        }
        let tick = control.elapsed().saturating_sub(t0);
        let mut frame = Vec::with_capacity((w * h) as usize);
        if let Err(e) = grab(&mut frame) {
            result = Err(e);
            break;
        }
        grabbed += 1;
        match tx.try_send((frame, tick)) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {}
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
        let next = frame_time * grabbed;
        if let Some(wait) = next.checked_sub(control.elapsed().saturating_sub(t0)) {
            std::thread::sleep(wait);
        }
    }
    drop(tx);
    let encoded = encoder
        .join()
        .unwrap_or_else(|_| Err("the GIF encoder crashed".into()));
    result?;
    encoded?;
    Ok(path.to_path_buf())
}

/// Draws a halo around the pointer and a burst for every click, so viewers
/// can follow the mouse in tutorials.
struct PointerMarks {
    on: bool,
    /// Top-left of the recorded area in desktop coordinates.
    origin: (i32, i32),
    was_down: bool,
    released: Option<(Instant, (f32, f32))>,
}

impl PointerMarks {
    fn new(on: bool, origin: (i32, i32)) -> Self {
        Self {
            on,
            origin,
            was_down: false,
            released: None,
        }
    }

    fn draw(&mut self, frame: &mut [u32], w: u32, h: u32) {
        if !self.on || frame.len() != (w * h) as usize {
            return;
        }
        let Some((x, y)) = platform::cursor_pos() else {
            return;
        };
        let p = ((x - self.origin.0) as f32, (y - self.origin.1) as f32);
        let down = platform::mouse_down();
        if self.was_down && !down {
            self.released = Some((Instant::now(), p));
        }
        self.was_down = down;
        let mut c = Canvas::new(frame, w, h);
        let at = Pt::new(p.0, p.1);
        c.fill_circle(at, 20.0, Color::rgba(255, 214, 10, 70));
        if down {
            c.fill_circle(at, 14.0, theme::ACCENT.alpha(0.55));
            c.stroke_circle(at, 20.0, 2.5, theme::ACCENT);
        }
        // A ring that widens and fades after each click.
        if let Some((t, rp)) = self.released {
            let k = t.elapsed().as_secs_f32() / 0.4;
            if k >= 1.0 {
                self.released = None;
            } else {
                c.stroke_circle(
                    Pt::new(rp.0, rp.1),
                    20.0 + 18.0 * k,
                    3.0,
                    theme::ACCENT.alpha(1.0 - k),
                );
            }
        }
    }
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

    fn stop_after(ms: u64) -> Arc<Control> {
        let control = Arc::new(Control::new());
        let c = control.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(ms));
            c.stop();
        });
        control
    }

    #[test]
    fn paused_time_does_not_count() {
        let c = Control::new();
        std::thread::sleep(Duration::from_millis(30));
        c.set_paused(true);
        let at_pause = c.elapsed();
        std::thread::sleep(Duration::from_millis(60));
        assert_eq!(c.elapsed(), at_pause, "the clock stands still while paused");
        c.set_paused(false);
        std::thread::sleep(Duration::from_millis(20));
        let e = c.elapsed();
        assert!(
            e >= Duration::from_millis(45) && e < Duration::from_millis(90),
            "{e:?}"
        );
        let pauses = c.pauses();
        assert_eq!(pauses.len(), 1);
        assert!(pauses[0].1 - pauses[0].0 >= 0.055);
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
        let out = encode_mp4(
            &mut src,
            128,
            96,
            30,
            &path,
            &stop_after(700),
            Encoder::Software,
        )
        .map_err(|e| e.msg)
        .unwrap();
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
    fn pointer_marks_stay_inside_the_frame() {
        let (w, h) = (40u32, 30u32);
        let mut frame = vec![0xFF00_0000u32; (w * h) as usize];
        // Far away from the pointer: nothing may be drawn, nothing may panic.
        let mut marks = PointerMarks::new(true, (-100_000, -100_000));
        marks.draw(&mut frame, w, h);
        assert!(frame.iter().all(|p| *p == 0xFF00_0000));
    }

    #[test]
    fn elapsed_formatting() {
        assert_eq!(format_elapsed(Duration::from_secs(0)), "0:00");
        assert_eq!(format_elapsed(Duration::from_secs(75)), "1:15");
        assert_eq!(format_elapsed(Duration::from_secs(600)), "10:00");
    }
}
