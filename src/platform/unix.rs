//! macOS and Linux back-end, built on `xcap` (CoreGraphics on macOS,
//! X11 / Wayland portals on Linux).

use crate::capture::{Shot, WinInfo};
use crate::geom::RectI;
use crate::gfx::rgba_to_argb;
use xcap::{Monitor, Window};

pub fn init_process() {}

pub fn attach_console() {}

pub fn allow_foreground_handoff() {}

/// Only needed on Windows, see the Windows back-end.
pub fn cloak(_window: &winit::window::Window, _on: bool) {}

/// Only needed on Windows, see the Windows back-end.
pub fn suppress_caption_paint(_window: &winit::window::Window) {}

pub fn cursor_pos() -> Option<(i32, i32)> {
    None
}

pub fn message_cursor_pos() -> Option<(i32, i32)> {
    None
}

pub fn hide_console(_cmd: &mut std::process::Command) {}

pub fn capture_monitors() -> Result<Vec<Shot>, String> {
    let monitors = Monitor::all().map_err(|e| format!("could not list monitors: {e}"))?;
    let mut shots = Vec::with_capacity(monitors.len());
    for m in monitors {
        let img = m
            .capture_image()
            .map_err(|e| format!("screen capture failed: {e}"))?;
        let (w, h) = (img.width(), img.height());
        let native = RectI::new(
            m.x().unwrap_or(0),
            m.y().unwrap_or(0),
            m.width().unwrap_or(w) as i32,
            m.height().unwrap_or(h) as i32,
        );
        shots.push(Shot {
            native,
            w,
            h,
            px: rgba_to_argb(img.as_raw()),
            scale: m.scale_factor().unwrap_or(1.0),
            primary: m.is_primary().unwrap_or(false),
            name: m.name().unwrap_or_default(),
        });
    }
    Ok(shots)
}

/// Visible top-level windows, top-most first. Empty when the platform does
/// not expose them (e.g. most Wayland compositors).
pub fn list_windows() -> Vec<WinInfo> {
    let Ok(windows) = Window::all() else {
        return Vec::new();
    };
    let mut list: Vec<(i32, WinInfo)> = windows
        .iter()
        .filter_map(|w| {
            if w.is_minimized().unwrap_or(false) {
                return None;
            }
            let rect = RectI::new(
                w.x().ok()?,
                w.y().ok()?,
                w.width().ok()? as i32,
                w.height().ok()? as i32,
            );
            if rect.w <= 8 || rect.h <= 8 {
                return None;
            }
            Some((
                w.z().unwrap_or(0),
                WinInfo {
                    rect,
                    title: w.title().unwrap_or_default(),
                },
            ))
        })
        .collect();
    list.sort_by_key(|w| std::cmp::Reverse(w.0));
    list.into_iter().map(|(_, w)| w).collect()
}

pub struct Grabber {
    monitor: Monitor,
    local: RectI,
}

impl Grabber {
    pub fn new(monitor: usize, _monitor_native: RectI, local: RectI) -> Result<Self, String> {
        let monitor = Monitor::all()
            .map_err(|e| e.to_string())?
            .into_iter()
            .nth(monitor)
            .ok_or_else(|| "monitor disappeared".to_string())?;
        Ok(Self { monitor, local })
    }

    pub fn grab(&mut self, out: &mut Vec<u32>) -> Result<(), String> {
        let img = self.monitor.capture_image().map_err(|e| e.to_string())?;
        let (iw, ih) = (img.width() as i32, img.height() as i32);
        let raw = img.as_raw();
        let r = self.local;
        out.clear();
        out.reserve_exact((r.w * r.h) as usize);
        for y in r.y..r.bottom() {
            for x in r.x..r.right() {
                if x >= 0 && y >= 0 && x < iw && y < ih {
                    let i = ((y * iw + x) * 4) as usize;
                    out.push(
                        0xFF00_0000
                            | (raw[i] as u32) << 16
                            | (raw[i + 1] as u32) << 8
                            | raw[i + 2] as u32,
                    );
                } else {
                    out.push(0xFF00_0000);
                }
            }
        }
        Ok(())
    }
}

/// Only Windows has a system snipping tool bound to Print Screen.
pub fn print_screen_opens_snipping_tool() -> bool {
    false
}

pub fn disable_snipping_tool_on_print_screen() -> std::io::Result<()> {
    Ok(())
}
