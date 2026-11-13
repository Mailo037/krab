//! Screen capture front-end. The heavy lifting lives in `platform`.

use crate::geom::RectI;
use crate::platform;

/// One monitor's frozen contents.
pub struct Shot {
    /// Monitor bounds in the platform's native desktop units (physical pixels
    /// on Windows/X11, points on macOS).
    pub native: RectI,
    /// Pixel dimensions of `px`.
    pub w: u32,
    pub h: u32,
    /// `0xFFRRGGBB` pixels, row-major.
    pub px: Vec<u32>,
    /// UI scale factor of the monitor (1.0 = 96 dpi).
    pub scale: f32,
    pub primary: bool,
    pub name: String,
}

impl Shot {
    /// Pixels per native desktop unit.
    pub fn px_per_unit(&self) -> f32 {
        if self.native.w > 0 {
            self.w as f32 / self.native.w as f32
        } else {
            1.0
        }
    }

    /// Convert a rect in native desktop units to pixels relative to this shot.
    pub fn native_to_local(&self, r: RectI) -> RectI {
        let k = self.px_per_unit();
        let f = |v: i32| (v as f32 * k).round() as i32;
        RectI::new(
            f(r.x - self.native.x),
            f(r.y - self.native.y),
            f(r.w),
            f(r.h),
        )
    }

    /// Convert a pixel rect relative to this shot back to native desktop units.
    pub fn local_to_native(&self, r: RectI) -> RectI {
        let k = self.px_per_unit();
        let f = |v: i32| (v as f32 / k).round() as i32;
        RectI::new(
            f(r.x) + self.native.x,
            f(r.y) + self.native.y,
            f(r.w),
            f(r.h),
        )
    }
}

/// A top-level window, in native desktop units.
#[derive(Clone, Debug)]
pub struct WinInfo {
    pub rect: RectI,
    pub title: String,
    /// The platform's handle for the window (HWND, CGWindowID, X11 id).
    pub id: u64,
}

/// One window rendered on its own, without whatever lies on top of it.
///
/// Covers the window's `WinInfo::rect`. Pixels are `0xAARRGGBB` with
/// premultiplied alpha; alpha 0 marks pixels the window did not draw (rounded
/// corners), where the screen as captured shows through.
pub struct WindowImage {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u32>,
}

/// Render window `id` on its own. `None` when the platform cannot do that
/// for this window (Linux, hung or minimized windows, protected content).
pub fn grab_window(id: u64) -> Option<WindowImage> {
    let img = platform::capture_window(id)?;
    let drawn = img.px.iter().any(|p| p & 0x00FF_FFFF != 0);
    (img.w > 0 && img.h > 0 && img.px.len() == (img.w * img.h) as usize && drawn).then_some(img)
}

pub struct Capture {
    pub shots: Vec<Shot>,
    /// Top-most first.
    pub windows: Vec<WinInfo>,
    /// Pointer position in native desktop units, when the platform exposes it.
    pub cursor: Option<(i32, i32)>,
}

/// Grab every monitor plus the window list.
pub fn capture_all() -> Result<Capture, String> {
    // Windows are listed first: it is quick, and it must happen before our own
    // overlay exists.
    let windows = platform::list_windows();
    let cursor = platform::cursor_pos();
    let shots = platform::capture_monitors()?;
    if shots.is_empty() {
        return Err("no monitors found".into());
    }
    Ok(Capture {
        shots,
        windows,
        cursor,
    })
}

/// Repeatedly grabs one screen region (used for recordings).
pub struct RegionGrabber {
    inner: platform::Grabber,
    pub w: u32,
    pub h: u32,
}

impl RegionGrabber {
    /// `monitor` is the index of the shot the region was selected on and
    /// `local` the region in that shot's pixels.
    /// With `cursor`, the mouse pointer is drawn into every frame where the
    /// platform supports it.
    pub fn new(
        monitor: usize,
        monitor_native: RectI,
        local: RectI,
        cursor: bool,
    ) -> Result<Self, String> {
        let inner = platform::Grabber::new(monitor, monitor_native, local, cursor)?;
        Ok(Self {
            inner,
            w: local.w as u32,
            h: local.h as u32,
        })
    }

    /// Fills `out` with `w * h` pixels.
    pub fn grab(&mut self, out: &mut Vec<u32>) -> Result<(), String> {
        self.inner.grab(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shot(native: RectI, w: u32, h: u32) -> Shot {
        Shot {
            native,
            w,
            h,
            px: Vec::new(),
            scale: 1.0,
            primary: true,
            name: String::new(),
        }
    }

    #[test]
    fn native_local_roundtrip_on_scaled_monitor() {
        // A 2x display placed to the right of a 1440-point-wide one.
        let s = shot(RectI::new(1440, 0, 1280, 800), 2560, 1600);
        let win = RectI::new(1500, 100, 400, 300);
        let local = s.native_to_local(win);
        assert_eq!(local, RectI::new(120, 200, 800, 600));
        assert_eq!(s.local_to_native(local), win);
    }

    #[test]
    fn unscaled_monitor_is_a_plain_translation() {
        let s = shot(RectI::new(-1920, 0, 1920, 1080), 1920, 1080);
        assert_eq!(
            s.native_to_local(RectI::new(-1900, 10, 100, 50)),
            RectI::new(20, 10, 100, 50)
        );
    }
}
