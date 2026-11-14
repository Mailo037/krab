//! Screen capture front-end. The heavy lifting lives in `platform`.

use crate::geom::RectI;
use crate::platform;
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

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

/// Renders one window on its own (see [`grab_window`]).
pub type WindowGrabber = fn(u64) -> Option<WindowImage>;

/// At most this many windows are rendered in advance...
const PRECAPTURE_MAX: usize = 12;
/// ...holding at most this many times the pixels of all monitors together.
const PRECAPTURE_BUDGET: i64 = 3;
/// How long a pick waits for a window that is still being rendered before
/// rendering it again by itself.
const PRECAPTURE_WAIT: Duration = Duration::from_millis(1000);

/// Covered windows, rendered on their own in the background as soon as a
/// snip starts.
///
/// A window that other windows lie on is shown - and captured - rendered by
/// itself. Rendering it only once it is picked shows its content as it is by
/// then: a video, a chat or a progress bar has moved on and no longer matches
/// the frozen screen around it. The cache renders every window that could be
/// picked right away, the one under the pointer first, so a pick shows the
/// window as it was when the snip started. Clones share the images; the work
/// stops once the last clone is gone.
#[derive(Clone)]
pub struct WindowCache(Arc<CacheInner>);

struct CacheInner {
    state: Mutex<CacheState>,
    ready: Condvar,
}

#[derive(Default)]
struct CacheState {
    queue: VecDeque<u64>,
    busy: Option<u64>,
    /// `None` for windows the platform could not render.
    done: HashMap<u64, Option<Arc<WindowImage>>>,
}

impl std::fmt::Debug for WindowCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WindowCache")
    }
}

impl WindowCache {
    /// Start rendering the windows of `cap` that might be picked. `None`
    /// when there are none, or the platform cannot render windows by
    /// themselves.
    pub fn start(cap: &Capture) -> Option<WindowCache> {
        if !platform::RENDERS_WINDOWS {
            return None;
        }
        let ids = precapture_order(cap);
        (!ids.is_empty()).then(|| Self::with_grabber(ids, grab_window))
    }

    /// Render `ids` in this order with `grab` on a worker thread.
    pub(crate) fn with_grabber(ids: Vec<u64>, grab: WindowGrabber) -> WindowCache {
        let cache = WindowCache(Arc::new(CacheInner {
            state: Mutex::new(CacheState {
                queue: ids.into(),
                ..CacheState::default()
            }),
            ready: Condvar::new(),
        }));
        let inner = cache.0.clone();
        let spawned = std::thread::Builder::new()
            .name("krab-windows".into())
            .spawn(move || precapture(&inner, grab));
        if spawned.is_err() {
            // Nothing will render them: picks fall back at once.
            if let Ok(mut st) = cache.0.state.lock() {
                st.queue.clear();
            }
        }
        cache
    }

    /// Window `id` as it was rendered in advance. Waits while it is still
    /// queued or being rendered (moving it to the front of the queue).
    /// `None` when it was not planned, could not be rendered, or takes too
    /// long - the caller then renders it now.
    pub fn get(&self, id: u64) -> Option<Arc<WindowImage>> {
        let deadline = Instant::now() + PRECAPTURE_WAIT;
        let mut st = self.0.state.lock().ok()?;
        loop {
            if let Some(img) = st.done.get(&id) {
                return img.clone();
            }
            match st.queue.iter().position(|q| *q == id) {
                Some(0) => {}
                Some(i) => {
                    st.queue.remove(i);
                    st.queue.push_front(id);
                }
                None if st.busy == Some(id) => {}
                None => return None,
            }
            let left = deadline.checked_duration_since(Instant::now())?;
            st = self.0.ready.wait_timeout(st, left).ok()?.0;
        }
    }
}

fn precapture(inner: &Arc<CacheInner>, grab: WindowGrabber) {
    loop {
        let id = {
            let Ok(mut st) = inner.state.lock() else {
                return;
            };
            // Nobody is left to ask for the images: the snip is over.
            if Arc::strong_count(inner) == 1 {
                st.queue.clear();
                return;
            }
            let Some(id) = st.queue.pop_front() else {
                break;
            };
            st.busy = Some(id);
            id
        };
        let img = grab(id).map(Arc::new);
        let Ok(mut st) = inner.state.lock() else {
            return;
        };
        st.busy = None;
        st.done.insert(id, img);
        inner.ready.notify_all();
    }
    crate::trace::mark("covered windows rendered");
}

/// The windows worth rendering in advance, the one under the pointer first,
/// then top-most first. Only covered windows count - an uncovered one is
/// taken from the screen as captured - and only those that show on a
/// monitor somewhere, so that they can be picked at all.
fn precapture_order(cap: &Capture) -> Vec<u64> {
    let k = cap
        .shots
        .iter()
        .map(Shot::px_per_unit)
        .fold(1.0f32, f32::max);
    // Window shadows reach 16 logical pixels past the frame (as in the overlay).
    let shadow = cap
        .shots
        .iter()
        .map(|s| (16.0 * s.scale / s.px_per_unit()).ceil() as i32)
        .max()
        .unwrap_or(16);
    let mut budget = PRECAPTURE_BUDGET
        * cap
            .shots
            .iter()
            .map(|s| s.w as i64 * s.h as i64)
            .sum::<i64>();
    let under_cursor = cap
        .cursor
        .and_then(|(x, y)| cap.windows.iter().position(|w| w.rect.contains(x, y)));

    let wins = &cap.windows;
    let mut order: Vec<usize> = (0..wins.len()).collect();
    if let Some(i) = under_cursor {
        order.retain(|j| *j != i);
        order.insert(0, i);
    }
    let mut ids = Vec::new();
    for i in order {
        if ids.len() >= PRECAPTURE_MAX {
            break;
        }
        let r = wins[i].rect;
        let above = &wins[..i];
        let covered = above
            .iter()
            .any(|a| !a.rect.inflate(shadow).intersect(&r).is_empty());
        if !covered || !shows_somewhere(r, &cap.shots, above) {
            continue;
        }
        let px = (r.area() as f32 * k * k) as i64;
        if px > budget {
            continue;
        }
        budget -= px;
        ids.push(wins[i].id);
    }
    ids
}

/// Whether part of `r` lies on a monitor and is not hidden by `above`.
fn shows_somewhere(r: RectI, shots: &[Shot], above: &[WinInfo]) -> bool {
    let mut parts: Vec<RectI> = shots
        .iter()
        .map(|s| r.intersect(&s.native))
        .filter(|p| !p.is_empty())
        .collect();
    for a in above {
        parts = parts.iter().flat_map(|p| p.subtract(&a.rect)).collect();
        if parts.is_empty() || parts.len() > 64 {
            break;
        }
    }
    !parts.is_empty()
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

    fn win(id: u64, x: i32, y: i32, w: i32, h: i32) -> WinInfo {
        WinInfo {
            rect: RectI::new(x, y, w, h),
            title: String::new(),
            id,
        }
    }

    fn desktop(windows: Vec<WinInfo>, cursor: Option<(i32, i32)>) -> Capture {
        Capture {
            shots: vec![shot(RectI::new(0, 0, 1000, 800), 1000, 800)],
            windows,
            cursor,
        }
    }

    #[test]
    fn covered_windows_are_rendered_in_advance_the_one_under_the_pointer_first() {
        let cap = desktop(
            vec![
                win(1, 100, 100, 300, 200),  // on top: taken from the screen
                win(2, 350, 250, 300, 200),  // partly under 1
                win(3, 120, 120, 100, 100),  // entirely under 1: cannot be picked
                win(4, 600, 600, 100, 100),  // nothing on it
                win(5, 710, 690, 400, 400),  // under 4's shadow, partly off screen
                win(6, 1005, 690, 300, 100), // under 5, but off every monitor
                win(7, 0, 0, 1000, 800),     // a maximized window behind it all
            ],
            Some((5, 5)),
        );
        assert_eq!(precapture_order(&cap), vec![7, 2, 5]);
    }

    #[test]
    fn windows_beyond_the_memory_budget_are_left_to_render_on_pick() {
        // Big windows, each peeking out a little to the right of the one
        // above it, so all of them could be picked.
        let windows = (0..6)
            .map(|i| win(i + 1, 20 * i as i32, 0, 900, 800))
            .collect();
        // Three of them hold three monitors' worth of pixels.
        assert_eq!(precapture_order(&desktop(windows, None)), vec![2, 3, 4]);
    }

    static RENDERS: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    /// A window whose content moves on every time it is rendered.
    fn ticking_window(id: u64) -> Option<WindowImage> {
        let n = RENDERS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        (id != 9).then(|| WindowImage {
            w: 2,
            h: 1,
            px: vec![0xFF00_0000 | n; 2],
        })
    }

    #[test]
    fn a_pick_gets_the_window_as_it_was_rendered_at_the_start() {
        let cache = WindowCache::with_grabber(vec![1, 2, 9], ticking_window);
        let first = cache.get(2).expect("rendered");
        let again = cache.get(2).expect("still there");
        assert!(Arc::ptr_eq(&first, &again), "not rendered a second time");
        assert!(cache.get(1).is_some());
        // Planned but not renderable, and never planned: render on pick.
        assert!(cache.get(9).is_none());
        assert!(cache.get(42).is_none());
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
