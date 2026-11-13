//! Sound for screen recordings.
//!
//! Each source (what the computer plays, the microphone) is captured into its
//! own WAV file while the picture is being encoded; once the recording stops
//! the files are mixed into the MP4 by ffmpeg. Windows captures through WASAPI
//! directly (the speakers via loopback), macOS and Linux let ffmpeg read the
//! microphone (AVFoundation) or the PulseAudio / PipeWire monitor.

use crate::record::Control;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Which sounds a recording includes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Off,
    /// What the computer plays.
    System,
    Mic,
    Both,
}

impl Source {
    pub fn parse(s: &str) -> Source {
        match s.trim().to_ascii_lowercase().as_str() {
            "system" | "desktop" | "speakers" => Source::System,
            "mic" | "microphone" => Source::Mic,
            "both" | "all" => Source::Both,
            _ => Source::Off,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Off => "off",
            Source::System => "system",
            Source::Mic => "mic",
            Source::Both => "both",
        }
    }

    fn wants_system(self) -> bool {
        matches!(self, Source::System | Source::Both)
    }

    fn wants_mic(self) -> bool {
        matches!(self, Source::Mic | Source::Both)
    }
}

/// Audio capture running alongside a recording.
pub struct Capture {
    streams: Vec<backend::Stream>,
    control: Arc<Control>,
}

/// One captured source.
pub struct Track {
    pub path: PathBuf,
    /// Stretches (seconds from the start) recorded while the recording was
    /// paused, to be cut out when mixing.
    pub cuts: Vec<(f64, f64)>,
}

impl Capture {
    /// Start capturing `source` into WAV files in `dir`. Sources that are
    /// unavailable are skipped; the recording goes on without them.
    pub fn start(source: Source, dir: &Path, control: Arc<Control>) -> Capture {
        let mut streams = Vec::new();
        let stamp = std::process::id();
        if source.wants_system() {
            let path = dir.join(format!(".krab-audio-{stamp}-system.wav"));
            match backend::Stream::start(true, path, control.clone()) {
                Ok(s) => streams.push(s),
                Err(e) => eprintln!("krab: system sound unavailable: {e}"),
            }
        }
        if source.wants_mic() {
            let path = dir.join(format!(".krab-audio-{stamp}-mic.wav"));
            match backend::Stream::start(false, path, control.clone()) {
                Ok(s) => streams.push(s),
                Err(e) => eprintln!("krab: microphone unavailable: {e}"),
            }
        }
        Capture { streams, control }
    }

    /// Stop and return the files that received sound.
    pub fn finish(self) -> Vec<Track> {
        let cuts = if backend::DROPS_PAUSES {
            Vec::new()
        } else {
            self.control.pauses()
        };
        self.streams
            .into_iter()
            .filter_map(|s| s.finish())
            .filter(|p| {
                let ok = std::fs::metadata(p).is_ok_and(|m| m.len() > 44 + 4096);
                if !ok {
                    let _ = std::fs::remove_file(p);
                }
                ok
            })
            .map(|path| Track {
                path,
                cuts: cuts.clone(),
            })
            .collect()
    }
}

/// The ffmpeg filter graph that trims pauses out of each track and mixes
/// them into one stream labelled `[a]`.
pub fn filter_graph(tracks: &[Track]) -> String {
    let mut graph = String::new();
    for (i, t) in tracks.iter().enumerate() {
        let n = i + 1;
        if t.cuts.is_empty() {
            graph.push_str(&format!("[{n}:a]anull[t{n}];"));
        } else {
            let keep: Vec<String> = t
                .cuts
                .iter()
                .map(|(a, b)| format!("between(t,{a:.3},{b:.3})"))
                .collect();
            graph.push_str(&format!(
                "[{n}:a]aselect='not({})',asetpts=N/SR/TB[t{n}];",
                keep.join("+")
            ));
        }
    }
    let labels: String = (1..=tracks.len()).map(|n| format!("[t{n}]")).collect();
    if tracks.len() > 1 {
        graph.push_str(&format!(
            "{labels}amix=inputs={}:duration=longest:normalize=0[a]",
            tracks.len()
        ));
    } else {
        graph.push_str(&format!("{labels}anull[a]"));
    }
    graph
}

/// Mix `tracks` into `video`, writing `out`. The inputs are left in place.
pub fn mux(video: &Path, tracks: &[Track], out: &Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-loglevel", "error", "-i"]).arg(video);
    for t in tracks {
        cmd.arg("-i").arg(&t.path);
    }
    cmd.args(["-filter_complex", &filter_graph(tracks)])
        .args(["-map", "0:v", "-map", "[a]"]);
    cmd.args(["-c:v", "copy", "-c:a", "aac", "-b:a", "160k", "-shortest"])
        .args(["-movflags", "+faststart"])
        .arg(out)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    crate::platform::hide_console(&mut cmd);
    let output = cmd.output().map_err(|e| e.to_string())?;
    if output.status.success() && out.exists() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// A WAV file written incrementally; the sizes are filled in at the end.
pub struct WavWriter {
    file: std::io::BufWriter<std::fs::File>,
    data_len: u64,
}

impl WavWriter {
    /// `format`: 1 = integer PCM, 3 = IEEE float.
    pub fn create(
        path: &Path,
        format: u16,
        channels: u16,
        rate: u32,
        bits: u16,
    ) -> std::io::Result<WavWriter> {
        use std::io::Write;
        let mut file = std::io::BufWriter::new(std::fs::File::create(path)?);
        let block = channels * bits / 8;
        file.write_all(b"RIFF\0\0\0\0WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&format.to_le_bytes())?;
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&rate.to_le_bytes())?;
        file.write_all(&(rate * block as u32).to_le_bytes())?;
        file.write_all(&block.to_le_bytes())?;
        file.write_all(&bits.to_le_bytes())?;
        file.write_all(b"data\0\0\0\0")?;
        Ok(WavWriter { file, data_len: 0 })
    }

    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        self.data_len += bytes.len() as u64;
        self.file.write_all(bytes)
    }

    pub fn finish(self) -> std::io::Result<()> {
        use std::io::{Seek, SeekFrom, Write};
        let mut f = self.file.into_inner().map_err(|e| e.into_error())?;
        let data = self.data_len.min(u32::MAX as u64 - 36) as u32;
        f.seek(SeekFrom::Start(4))?;
        f.write_all(&(36 + data).to_le_bytes())?;
        f.seek(SeekFrom::Start(40))?;
        f.write_all(&data.to_le_bytes())?;
        f.flush()
    }
}

#[cfg(windows)]
mod backend {
    //! WASAPI in shared mode: loopback on the default output for system
    //! sound, the default input for the microphone.

    use super::WavWriter;
    use crate::record::Control;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use windows::Win32::Media::Audio::{
        eCapture, eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDeviceEnumerator,
        MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
        AUDCLNT_STREAMFLAGS_LOOPBACK,
    };
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
        COINIT_MULTITHREADED,
    };

    /// Sound arriving while paused is dropped right away.
    pub const DROPS_PAUSES: bool = true;

    pub struct Stream {
        stop: Arc<AtomicBool>,
        thread: std::thread::JoinHandle<Option<PathBuf>>,
    }

    impl Stream {
        pub fn start(system: bool, path: PathBuf, control: Arc<Control>) -> Result<Stream, String> {
            let stop = Arc::new(AtomicBool::new(false));
            let flag = stop.clone();
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let thread = std::thread::spawn(move || {
                let result = unsafe { run(system, &path, &control, &flag, &ready_tx) };
                match result {
                    Ok(()) => Some(path),
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                        let _ = std::fs::remove_file(&path);
                        None
                    }
                }
            });
            match ready_rx.recv_timeout(Duration::from_secs(3)) {
                Ok(Ok(())) => Ok(Stream { stop, thread }),
                Ok(Err(e)) => Err(e),
                Err(_) => {
                    stop.store(true, Ordering::SeqCst);
                    Err("the audio device did not respond".into())
                }
            }
        }

        pub fn finish(self) -> Option<PathBuf> {
            self.stop.store(true, Ordering::SeqCst);
            self.thread.join().ok().flatten()
        }
    }

    /// (format tag, channels, rate, bits) from a WAVEFORMATEX(TENSIBLE).
    unsafe fn describe(fmt: *const u8) -> (u16, u16, u32, u16) {
        let rd16 = |o: usize| u16::from_le_bytes([*fmt.add(o), *fmt.add(o + 1)]);
        let rd32 = |o: usize| {
            u32::from_le_bytes([
                *fmt.add(o),
                *fmt.add(o + 1),
                *fmt.add(o + 2),
                *fmt.add(o + 3),
            ])
        };
        let mut tag = rd16(0);
        let (channels, rate, bits) = (rd16(2), rd32(4), rd16(14));
        if tag == 0xFFFE && rd16(16) >= 22 {
            // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts with the tag.
            tag = rd32(24) as u16;
        }
        (tag, channels, rate, bits)
    }

    unsafe fn run(
        system: bool,
        path: &std::path::Path,
        control: &Control,
        stop: &AtomicBool,
        ready: &std::sync::mpsc::Sender<Result<(), String>>,
    ) -> Result<(), String> {
        let e = |e: windows::core::Error| e.message();
        let com = CoInitializeEx(None, COINIT_MULTITHREADED).is_ok();
        let result = (|| {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).map_err(e)?;
            let device = enumerator
                .GetDefaultAudioEndpoint(if system { eRender } else { eCapture }, eConsole)
                .map_err(e)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None).map_err(e)?;
            let fmt = client.GetMixFormat().map_err(e)?;
            let (tag, channels, rate, bits) = describe(fmt as *const u8);
            let flags = if system {
                AUDCLNT_STREAMFLAGS_LOOPBACK
            } else {
                0
            };
            // A one second buffer leaves plenty of room between polls.
            let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 10_000_000, 0, fmt, None);
            CoTaskMemFree(Some(fmt as *const _));
            init.map_err(e)?;
            if tag != 1 && tag != 3 {
                return Err(format!("unsupported sample format {tag}"));
            }
            let capture: IAudioCaptureClient = client.GetService().map_err(e)?;
            let mut wav =
                WavWriter::create(path, tag, channels, rate, bits).map_err(|e| e.to_string())?;
            let frame_bytes = (channels as usize * bits as usize) / 8;
            client.Start().map_err(e)?;
            let _ = ready.send(Ok(()));

            let mut written: u64 = 0;
            let zeros = vec![0u8; frame_bytes * 4800];
            while !stop.load(Ordering::SeqCst) {
                std::thread::sleep(Duration::from_millis(10));
                loop {
                    let n = capture.GetNextPacketSize().map_err(e)?;
                    if n == 0 {
                        break;
                    }
                    let mut data = std::ptr::null_mut();
                    let mut frames = 0u32;
                    let mut bflags = 0u32;
                    capture
                        .GetBuffer(&mut data, &mut frames, &mut bflags, None, None)
                        .map_err(e)?;
                    let len = frames as usize * frame_bytes;
                    if !control.is_paused() {
                        if bflags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                            write_silence(&mut wav, &zeros, len)?;
                        } else {
                            wav.write(std::slice::from_raw_parts(data, len))
                                .map_err(|e| e.to_string())?;
                        }
                        written += frames as u64;
                    }
                    capture.ReleaseBuffer(frames).map_err(e)?;
                }
                // Loopback delivers nothing while nothing plays: fill the gap
                // with silence so the sound stays in step with the picture.
                if !control.is_paused() {
                    let due = (control.elapsed().as_secs_f64() * rate as f64) as u64;
                    let lag = rate as u64 / 10;
                    if due > written + lag {
                        let missing = due - written - lag / 2;
                        write_silence(&mut wav, &zeros, missing as usize * frame_bytes)?;
                        written += missing;
                    }
                }
            }
            let _ = client.Stop();
            wav.finish().map_err(|e| e.to_string())
        })();
        if com {
            CoUninitialize();
        }
        result
    }

    fn write_silence(wav: &mut WavWriter, zeros: &[u8], mut len: usize) -> Result<(), String> {
        while len > 0 {
            let n = len.min(zeros.len());
            wav.write(&zeros[..n]).map_err(|e| e.to_string())?;
            len -= n;
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod backend {
    //! ffmpeg reads the device: AVFoundation on macOS, PulseAudio (also
    //! served by PipeWire) on Linux.

    use crate::record::Control;
    use std::io::Write;
    use std::path::PathBuf;
    use std::process::{Child, Command, Stdio};
    use std::sync::Arc;

    /// ffmpeg keeps recording while paused; the pauses are cut out later.
    pub const DROPS_PAUSES: bool = false;

    pub struct Stream {
        child: Child,
        path: PathBuf,
    }

    impl Stream {
        pub fn start(
            system: bool,
            path: PathBuf,
            _control: Arc<Control>,
        ) -> Result<Stream, String> {
            let input: [&str; 4] = if cfg!(target_os = "macos") {
                if system {
                    return Err("macOS does not let apps record the system sound".into());
                }
                ["-f", "avfoundation", "-i", ":default"]
            } else if system {
                ["-f", "pulse", "-i", "@DEFAULT_MONITOR@"]
            } else {
                ["-f", "pulse", "-i", "default"]
            };
            let child = Command::new("ffmpeg")
                .args(["-y", "-loglevel", "error"])
                .args(input)
                .args(["-c:a", "pcm_s16le"])
                .arg(&path)
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .map_err(|e| e.to_string())?;
            Ok(Stream { child, path })
        }

        pub fn finish(mut self) -> Option<PathBuf> {
            // "q" asks ffmpeg to finish the file properly.
            if let Some(stdin) = self.child.stdin.as_mut() {
                let _ = stdin.write_all(b"q");
            }
            drop(self.child.stdin.take());
            let ok = self.child.wait().is_ok_and(|s| s.success());
            ok.then_some(self.path)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_names_roundtrip() {
        for s in [Source::Off, Source::System, Source::Mic, Source::Both] {
            assert_eq!(Source::parse(s.as_str()), s);
        }
        assert_eq!(Source::parse("Microphone"), Source::Mic);
        assert_eq!(Source::parse("nonsense"), Source::Off);
    }

    #[test]
    fn filter_graph_trims_pauses_and_mixes() {
        let one = [Track {
            path: "a.wav".into(),
            cuts: Vec::new(),
        }];
        assert_eq!(filter_graph(&one), "[1:a]anull[t1];[t1]anull[a]");
        let two = [
            Track {
                path: "a.wav".into(),
                cuts: vec![(1.0, 2.5)],
            },
            Track {
                path: "b.wav".into(),
                cuts: Vec::new(),
            },
        ];
        let g = filter_graph(&two);
        assert!(g.contains("[1:a]aselect='not(between(t,1.000,2.500))',asetpts=N/SR/TB[t1]"));
        assert!(g.ends_with("[t1][t2]amix=inputs=2:duration=longest:normalize=0[a]"));
    }

    #[test]
    fn wav_header_is_valid() {
        let path = std::env::temp_dir().join(format!("krab-wav-test-{}.wav", std::process::id()));
        let mut w = WavWriter::create(&path, 1, 2, 48_000, 16).unwrap();
        w.write(&[0u8; 4 * 480]).unwrap();
        w.finish().unwrap();
        let b = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(&b[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 36 + 1920);
        assert_eq!(&b[36..40], b"data");
        assert_eq!(u32::from_le_bytes(b[40..44].try_into().unwrap()), 1920);
        assert_eq!(b.len(), 44 + 1920);
    }
}

#[cfg(all(test, windows))]
mod device_tests {
    use super::*;

    /// Needs an audio output device: `cargo test --lib loopback -- --ignored`
    #[test]
    #[ignore]
    fn loopback_capture_writes_a_wav_in_step_with_the_clock() {
        let dir = std::env::temp_dir();
        let control = Arc::new(Control::new());
        let cap = Capture::start(Source::System, &dir, control.clone());
        std::thread::sleep(std::time::Duration::from_millis(1500));
        let elapsed = control.elapsed().as_secs_f64();
        let tracks = cap.finish();
        assert_eq!(tracks.len(), 1, "no system sound track");
        let bytes = std::fs::read(&tracks[0].path).unwrap();
        std::fs::remove_file(&tracks[0].path).unwrap();
        let rate = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as f64;
        let block = u16::from_le_bytes(bytes[32..34].try_into().unwrap()) as f64;
        let secs = (bytes.len() - 44) as f64 / block / rate;
        // Silence is padded in, so the track is as long as the recording.
        assert!(
            (secs - elapsed).abs() < 0.25,
            "{secs:.2}s of sound for {elapsed:.2}s"
        );
    }
}
