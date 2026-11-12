//! The windowing layer: turns captures into overlay windows, routes input to
//! [`Overlay`], and carries out what the user decided.

use crate::capture::{self, Capture, Shot};
use crate::config::Config;
use crate::geom::RectI;
use crate::hud::{self, Hud};
use crate::ipc::{Message, Request};
use crate::output::{self, Image};
use crate::overlay::{
    self, Button, CursorKind, Finish, Kind, Mode, Mods, Options, Outcome, Overlay, Shared,
};
use crate::record::Recorder;
use crate::trace;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{
    ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy, OwnedDisplayHandle,
};
use winit::keyboard::{Key as WinitKey, NamedKey};
use winit::monitor::MonitorHandle;
use winit::window::{CursorIcon, Window, WindowId, WindowLevel};

type Surface = softbuffer::Surface<OwnedDisplayHandle, Rc<Window>>;

pub enum UserEvent {
    Captured(Box<Result<Capture, String>>, Request),
    Message(Message),
    Hotkey,
    Menu(String),
    TrayClick,
    RecordingDone(Result<PathBuf, String>),
    TextExtracted(Result<String, String>),
}

/// How this process was started.
pub struct Launch {
    /// Stay alive in the background (tray icon, hotkey, IPC).
    pub daemon: bool,
    /// A capture already running on another thread; it reports back once it
    /// receives the event-loop proxy through this channel.
    pub pending_capture: Option<mpsc::Sender<EventLoopProxy<UserEvent>>>,
    /// A request to start once the event loop runs (delayed one-shot snips).
    pub initial: Option<Request>,
    pub output: Option<PathBuf>,
    pub quiet: bool,
    pub exit_after: Option<Duration>,
}

struct OverlayWin {
    window: Rc<Window>,
    surface: Surface,
    overlay: Overlay,
    /// Index of the monitor in capture order, and its native bounds.
    index: usize,
    native: RectI,
    monitor: Option<MonitorHandle>,
    cursor: CursorKind,
    presented: bool,
}

struct Session {
    wins: Vec<OverlayWin>,
}

/// A hidden, ready-made overlay window for one monitor. Creating a window is
/// the slowest part of opening the overlay, so windows are made ahead of
/// time (while the screen is still being captured) and reused between snips.
struct PoolWin {
    window: Rc<Window>,
    monitor: MonitorHandle,
}

/// Platforms where a window can exist without being shown. Wayland cannot
/// hide a mapped window, so Linux creates overlay windows on demand instead.
const POOL_WINDOWS: bool = cfg!(any(windows, target_os = "macos"));

fn monitor_key(m: &MonitorHandle) -> (i32, i32, u32, u32) {
    (
        m.position().x,
        m.position().y,
        m.size().width,
        m.size().height,
    )
}

#[derive(Clone, Copy)]
enum Anchor {
    TopCenter,
    BottomRight,
}

struct HudWin {
    window: Rc<Window>,
    surface: Surface,
    hud: Hud,
    scale: f32,
    expires: Option<Instant>,
    hovered: bool,
}

pub struct App {
    cfg: Config,
    launch: Launch,
    proxy: EventLoopProxy<UserEvent>,
    context: Option<softbuffer::Context<OwnedDisplayHandle>>,
    session: Option<Session>,
    pool: Vec<PoolWin>,
    huds: Vec<HudWin>,
    mods: Mods,
    /// A capture thread is running.
    capturing: bool,
    /// Countdown before a delayed capture.
    delay: Option<(Instant, Request)>,
    /// Short pause that lets menus and our own windows disappear first.
    settle: Option<(Instant, Request)>,
    recorder: Option<Recorder>,
    /// Background jobs (text extraction) still running.
    busy: usize,
    /// A finished snip waiting to be copied / saved.
    pending: Option<(Finish, Image)>,
    last_monitor: Option<MonitorHandle>,
    /// Testing aid: when to cancel the open overlay automatically.
    session_deadline: Option<Instant>,
    started: bool,
    #[cfg(any(windows, target_os = "macos"))]
    tray: Option<tray::Tray>,
}

pub fn run(cfg: Config, launch: Launch) -> Result<(), String> {
    let mut builder = EventLoop::<UserEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        // A menu-bar style app: no Dock icon.
        builder.with_activation_policy(ActivationPolicy::Accessory);
    }
    let event_loop = builder
        .build()
        .map_err(|e| format!("could not start the event loop: {e}"))?;
    trace::mark("event loop ready");
    let proxy = event_loop.create_proxy();
    let capturing = match &launch.pending_capture {
        Some(tx) => tx.send(proxy.clone()).is_ok(),
        None => false,
    };
    let mut app = App {
        cfg,
        launch,
        proxy,
        context: None,
        session: None,
        pool: Vec::new(),
        huds: Vec::new(),
        mods: Mods::default(),
        capturing,
        delay: None,
        settle: None,
        recorder: None,
        busy: 0,
        pending: None,
        last_monitor: None,
        session_deadline: None,
        started: false,
        #[cfg(any(windows, target_os = "macos"))]
        tray: None,
    };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())
}

fn cursor_icon(kind: CursorKind) -> CursorIcon {
    match kind {
        CursorKind::Default => CursorIcon::Default,
        CursorKind::Crosshair => CursorIcon::Crosshair,
        CursorKind::Pointer => CursorIcon::Pointer,
        CursorKind::Move => CursorIcon::Move,
        CursorKind::Text => CursorIcon::Text,
        CursorKind::ResizeNs => CursorIcon::NsResize,
        CursorKind::ResizeEw => CursorIcon::EwResize,
        CursorKind::ResizeNesw => CursorIcon::NeswResize,
        CursorKind::ResizeNwse => CursorIcon::NwseResize,
    }
}

/// Find the winit monitor a shot was taken from.
fn match_monitor(monitors: &[MonitorHandle], shot: &Shot, index: usize) -> Option<MonitorHandle> {
    let k = shot.px_per_unit();
    let scaled = (
        (shot.native.x as f32 * k).round() as i32,
        (shot.native.y as f32 * k).round() as i32,
    );
    let raw = (shot.native.x, shot.native.y);
    let pos = |m: &MonitorHandle| (m.position().x, m.position().y);
    let size_ok = |m: &MonitorHandle| m.size().width == shot.w && m.size().height == shot.h;
    monitors
        .iter()
        .find(|m| size_ok(m) && (pos(m) == raw || pos(m) == scaled))
        .or_else(|| monitors.iter().find(|m| pos(m) == raw || pos(m) == scaled))
        .or_else(|| {
            monitors
                .iter()
                .find(|m| !shot.name.is_empty() && m.name().as_deref() == Some(shot.name.as_str()))
        })
        .or_else(|| monitors.get(index))
        .cloned()
}

/// Create a hidden, borderless, always-on-top window covering `monitor`
/// (or the given native bounds when the monitor could not be identified).
fn create_overlay_window(
    el: &ActiveEventLoop,
    monitor: Option<&MonitorHandle>,
    native: RectI,
    size: (u32, u32),
) -> Option<Rc<Window>> {
    let mut attrs = Window::default_attributes()
        .with_title("Krab")
        .with_decorations(false)
        .with_resizable(false)
        .with_window_level(WindowLevel::AlwaysOnTop)
        .with_visible(false);
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowAttributesExtWindows;
        // No drag-and-drop target: skips OLE initialization on the hot path.
        attrs = attrs.with_skip_taskbar(true).with_drag_and_drop(false);
    }
    // macOS full screen would animate into a new Space; "simple full screen"
    // (applied when the window is shown) covers the menu bar without that.
    if !cfg!(target_os = "macos") && monitor.is_some() {
        attrs = attrs.with_fullscreen(Some(winit::window::Fullscreen::Borderless(
            monitor.cloned(),
        )));
    } else {
        let pos = monitor
            .map(|m| m.position())
            .unwrap_or(PhysicalPosition::new(native.x, native.y));
        let size = monitor
            .map(|m| m.size())
            .unwrap_or(PhysicalSize::new(size.0, size.1));
        attrs = attrs.with_position(pos).with_inner_size(size);
    }
    #[cfg(target_os = "macos")]
    {
        use winit::platform::macos::WindowAttributesExtMacOS;
        attrs = attrs.with_has_shadow(false);
    }
    el.create_window(attrs).ok().map(Rc::new)
}

fn draw_overlay(win: &mut OverlayWin) {
    let size = win.window.inner_size();
    let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
        return;
    };
    if win.surface.resize(w, h).is_err() {
        return;
    }
    let Ok(mut buf) = win.surface.buffer_mut() else {
        return;
    };
    win.overlay.render(&mut buf, size.width, size.height);
    let _ = buf.present();
    if !win.presented {
        win.presented = true;
        trace::mark("overlay on screen");
    }
}

fn draw_hud(win: &mut HudWin) {
    let size = win.window.inner_size();
    let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
        return;
    };
    if win.surface.resize(w, h).is_err() {
        return;
    }
    let Ok(mut buf) = win.surface.buffer_mut() else {
        return;
    };
    win.hud
        .render(&mut buf, size.width, size.height, win.scale, win.hovered);
    let _ = buf.present();
}

/// "Screenshots" from ".../Pictures/Screenshots/Krab_x.png".
fn folder_name(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

impl App {
    fn context(
        &mut self,
        el: &ActiveEventLoop,
    ) -> Option<&softbuffer::Context<OwnedDisplayHandle>> {
        if self.context.is_none() {
            self.context = softbuffer::Context::new(el.owned_display_handle()).ok();
        }
        self.context.as_ref()
    }

    fn idle(&self) -> bool {
        self.session.is_none()
            && self.huds.is_empty()
            && !self.capturing
            && self.delay.is_none()
            && self.settle.is_none()
            && self.recorder.is_none()
            && self.busy == 0
            && self.pending.is_none()
    }

    /// Make sure a hidden overlay window exists for every connected monitor.
    fn sync_pool(&mut self, el: &ActiveEventLoop) {
        if !POOL_WINDOWS {
            return;
        }
        let monitors: Vec<MonitorHandle> = el.available_monitors().collect();
        self.pool.retain(|p| {
            monitors
                .iter()
                .any(|m| monitor_key(m) == monitor_key(&p.monitor))
        });
        for m in monitors {
            if self
                .pool
                .iter()
                .any(|p| monitor_key(&p.monitor) == monitor_key(&m))
            {
                continue;
            }
            let size = (m.size().width, m.size().height);
            let native = RectI::new(m.position().x, m.position().y, size.0 as i32, size.1 as i32);
            if let Some(window) = create_overlay_window(el, Some(&m), native, size) {
                self.pool.push(PoolWin { window, monitor: m });
            }
        }
        trace::mark("overlay windows ready");
    }

    // ---- capture lifecycle -------------------------------------------------

    /// Entry point for every "take a snip" trigger.
    fn request(&mut self, el: &ActiveEventLoop, req: Request) {
        trace::mark("snip requested");
        if let Some(rec) = &self.recorder {
            // The hotkey doubles as "stop recording".
            if !rec.is_stopping() {
                self.stop_recording();
            }
            return;
        }
        if self.session.is_some() || self.capturing || self.delay.is_some() || self.settle.is_some()
        {
            return;
        }
        if req.delay > 0 {
            let secs = req.delay;
            self.begin_delay(el, secs, Request { delay: 0, ..req });
            return;
        }
        self.capturing = true;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = capture::capture_all();
            let _ = proxy.send_event(UserEvent::Captured(Box::new(result), req));
        });
    }

    fn open_session(&mut self, el: &ActiveEventLoop, cap: Capture, req: Request) {
        trace::mark("capture received");
        // Our own toasts must not sit on top of the overlay.
        self.huds.retain(|h| !matches!(h.hud, Hud::Toast { .. }));

        self.sync_pool(el);
        let monitors: Vec<MonitorHandle> = el.available_monitors().collect();
        let mode = req
            .mode
            .or_else(|| Mode::parse(&self.cfg.default_mode))
            .unwrap_or(Mode::Rect);
        let shared = Shared {
            mode,
            kind: req.kind,
            delay: self.cfg.delay_seconds,
        };
        let mut opts = Options::from_config(&self.cfg);
        if let Some(q) = req.quick {
            opts.quick_copy = q;
        }
        let cursor = cap.cursor;
        let Capture { shots, windows, .. } = cap;
        let mut wins = Vec::with_capacity(shots.len());
        let mut focus = 0;

        for (index, shot) in shots.into_iter().enumerate() {
            let monitor = match_monitor(&monitors, &shot, index);
            let native = shot.native;
            let (w, h) = (shot.w, shot.h);
            let under_cursor = match cursor {
                Some((x, y)) => native.contains(x, y),
                None => shot.primary,
            };
            let overlay = Overlay::new(shot, &windows, opts.clone(), shared);

            // Prefer a pre-made window for this monitor.
            let pooled = monitor.as_ref().and_then(|m| {
                let key = monitor_key(m);
                self.pool
                    .iter()
                    .position(|p| monitor_key(&p.monitor) == key)
                    .map(|i| self.pool.swap_remove(i).window)
            });
            let window = match pooled {
                Some(window) => window,
                None => match create_overlay_window(el, monitor.as_ref(), native, (w, h)) {
                    Some(window) => window,
                    None => continue,
                },
            };
            let Some(context) = self.context(el) else {
                continue;
            };
            let Ok(surface) = softbuffer::Surface::new(context, window.clone()) else {
                continue;
            };
            if under_cursor {
                focus = wins.len();
            }
            wins.push(OverlayWin {
                window,
                surface,
                overlay,
                index,
                native,
                monitor,
                cursor: CursorKind::Default,
                presented: false,
            });
        }
        if wins.is_empty() {
            self.toast(
                el,
                "Krab could not open the overlay".into(),
                String::new(),
                None,
                None,
            );
            return;
        }
        trace::mark("windows created");

        if mode == Mode::Fullscreen {
            wins[focus].overlay.select_all();
        }
        self.last_monitor = wins[focus].monitor.clone();
        for (i, win) in wins.iter_mut().enumerate() {
            win.overlay.set_mods(self.mods);
            if cfg!(windows) {
                // Render first, then show and blit: the overlay appears with
                // its first frame already in place.
                let size = win.window.inner_size();
                if let (Some(w), Some(h)) =
                    (NonZeroU32::new(size.width), NonZeroU32::new(size.height))
                {
                    if win.surface.resize(w, h).is_ok() {
                        if let Ok(mut buf) = win.surface.buffer_mut() {
                            win.overlay.render(&mut buf, size.width, size.height);
                            win.window.set_visible(true);
                            let _ = buf.present();
                            win.presented = true;
                        }
                    }
                }
            }
            win.window.set_visible(true);
            #[cfg(target_os = "macos")]
            {
                use winit::platform::macos::WindowExtMacOS;
                if !win.window.simple_fullscreen() {
                    win.window.set_simple_fullscreen(true);
                }
            }
            if i == focus {
                win.window.focus_window();
            }
            win.window.request_redraw();
        }
        trace::mark("overlay on screen");
        self.session = Some(Session { wins });
        self.session_deadline = self.launch.exit_after.map(|d| Instant::now() + d);
        self.after_input(el, focus);
    }

    fn close_session(&mut self) {
        if let Some(session) = self.session.take() {
            for win in session.wins {
                win.window.set_visible(false);
                // Keep the window for the next snip; its pixel buffers (the
                // surface) are dropped so an idle Krab stays small.
                if let (true, Some(monitor)) = (POOL_WINDOWS && self.launch.daemon, win.monitor) {
                    self.pool.push(PoolWin {
                        window: win.window,
                        monitor,
                    });
                }
            }
        }
    }

    /// Bookkeeping after any input reached overlay `idx`.
    fn after_input(&mut self, el: &ActiveEventLoop, idx: usize) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.wins[idx].overlay.take_started_selection() {
            // Only one monitor holds a selection at a time.
            for (i, win) in session.wins.iter_mut().enumerate() {
                if i != idx && win.overlay.has_selection() {
                    win.overlay.clear_selection();
                }
            }
        }
        let shared = session.wins[idx].overlay.shared();
        let outcome = session.wins[idx].overlay.take_outcome();
        for win in &mut session.wins {
            win.overlay.set_shared(shared);
            let kind = win.overlay.cursor_kind();
            if kind != win.cursor {
                win.cursor = kind;
                win.window.set_cursor(cursor_icon(kind));
            }
            if win.overlay.take_dirty() {
                win.window.request_redraw();
            }
        }
        if let Some(outcome) = outcome {
            self.handle_outcome(el, idx, outcome);
        }
    }

    fn handle_outcome(&mut self, el: &ActiveEventLoop, idx: usize, outcome: Outcome) {
        match outcome {
            Outcome::CopyText(text) => {
                let _ = output::copy_text(&text);
            }
            Outcome::Cancel => self.close_session(),
            Outcome::Finish(kind, img) => {
                self.close_session();
                // Delivered from `about_to_wait`: showing a native dialog (a
                // nested message loop) from inside a window-event callback
                // leaves the event loop unable to shut down afterwards.
                self.pending = Some((kind, img));
            }
            Outcome::Delay(secs) => {
                let shared = self.session.as_ref().map(|s| s.wins[idx].overlay.shared());
                self.close_session();
                let req = Request {
                    mode: shared.map(|s| s.mode),
                    kind: shared.map(|s| s.kind).unwrap_or(Kind::Snip),
                    delay: 0,
                    quick: None,
                };
                self.begin_delay(el, secs, req);
            }
            Outcome::Record(rect) => {
                let Some((index, native, monitor)) = self.session.as_ref().map(|s| {
                    (
                        s.wins[idx].index,
                        s.wins[idx].native,
                        s.wins[idx].monitor.clone(),
                    )
                }) else {
                    return;
                };
                self.close_session();
                self.last_monitor = monitor;
                self.start_recording(el, index, native, rect);
            }
        }
    }

    // ---- results -----------------------------------------------------------

    fn deliver(&mut self, el: &ActiveEventLoop, kind: Finish, img: Image) {
        let quality = self.cfg.jpeg_quality;
        match kind {
            Finish::Copy => {
                let mut title = String::new();
                let mut detail = String::new();
                let mut action = None;
                if self.cfg.copy_to_clipboard {
                    match output::copy_image(&img) {
                        Ok(()) => title = "Copied to clipboard".into(),
                        Err(e) => {
                            title = "Could not copy to the clipboard".into();
                            detail = e;
                        }
                    }
                }
                let saved = match &self.launch.output {
                    Some(path) => Some(img.save(path, quality).map(|_| path.clone())),
                    None if self.cfg.auto_save => Some(output::auto_save(&img, &self.cfg)),
                    None => None,
                };
                match saved {
                    Some(Ok(path)) => {
                        if title.is_empty() {
                            title = "Screenshot saved".into();
                        }
                        if detail.is_empty() {
                            detail = format!("Saved to {} - click to open", folder_name(&path));
                        }
                        action = Some(path);
                    }
                    Some(Err(e)) => detail = format!("Not saved: {e}"),
                    None => {}
                }
                if title.is_empty() {
                    title = "Snip taken".into();
                }
                self.toast(el, title, detail, Some(&img), action);
            }
            Finish::SaveAs => {
                // Copy first so the snip survives a cancelled dialog.
                let copied = self.cfg.copy_to_clipboard && output::copy_image(&img).is_ok();
                match output::save_as(&img, &self.cfg) {
                    Ok(Some(path)) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.toast(
                            el,
                            "Screenshot saved".into(),
                            format!("{name} - click to open"),
                            Some(&img),
                            Some(path),
                        );
                    }
                    Ok(None) if copied => {
                        self.toast(
                            el,
                            "Copied to clipboard".into(),
                            "Not saved to a file".into(),
                            Some(&img),
                            None,
                        );
                    }
                    Ok(None) => {}
                    Err(e) => self.toast(el, "Could not save".into(), e, Some(&img), None),
                }
            }
            Finish::OpenExternal => {
                let saved = if self.cfg.auto_save {
                    output::auto_save(&img, &self.cfg)
                } else {
                    let path = output::unique_path(&std::env::temp_dir(), "Krab", "png");
                    img.save(&path, quality).map(|_| path)
                };
                match saved {
                    Ok(path) => {
                        output::open_path(&path);
                        let detail = format!("Saved to {}", folder_name(&path));
                        self.toast(
                            el,
                            "Opened in your default app".into(),
                            detail,
                            Some(&img),
                            Some(path),
                        );
                    }
                    Err(e) => self.toast(el, "Could not save".into(), e, Some(&img), None),
                }
            }
            Finish::ExtractText => {
                self.busy += 1;
                let proxy = self.proxy.clone();
                std::thread::spawn(move || {
                    let _ =
                        proxy.send_event(UserEvent::TextExtracted(crate::ocr::extract_text(&img)));
                });
            }
        }
    }

    fn toast(
        &mut self,
        el: &ActiveEventLoop,
        title: String,
        detail: String,
        img: Option<&Image>,
        action: Option<PathBuf>,
    ) {
        if !self.cfg.notifications {
            return;
        }
        #[cfg(target_os = "linux")]
        {
            // Desktop notifications are the native idiom here (and Wayland
            // does not let applications position their own windows).
            let _ = (el, img);
            let mut n = notify_rust::Notification::new();
            n.appname("Krab")
                .summary(&title)
                .body(&detail)
                .icon("camera-photo")
                .timeout(3500);
            if let Some(path) = action.as_deref().and_then(Path::to_str) {
                n.image_path(path);
            }
            let _ = n.show();
        }
        #[cfg(not(target_os = "linux"))]
        {
            self.huds.retain(|h| !matches!(h.hud, Hud::Toast { .. }));
            let thumb = img.map(|i| hud::thumbnail(i, 320, 220));
            let hud = Hud::Toast {
                title,
                detail,
                thumb,
                action,
            };
            self.show_hud(
                el,
                hud,
                Anchor::BottomRight,
                Some(Duration::from_millis(3200)),
            );
        }
    }

    fn show_hud(&mut self, el: &ActiveEventLoop, hud: Hud, anchor: Anchor, ttl: Option<Duration>) {
        let monitor = match anchor {
            Anchor::TopCenter => self.last_monitor.clone().or_else(|| el.primary_monitor()),
            Anchor::BottomRight => el.primary_monitor(),
        }
        .or_else(|| el.available_monitors().next());
        let (origin, area, scale) = match &monitor {
            Some(m) => (m.position(), m.size(), m.scale_factor() as f32),
            None => (
                PhysicalPosition::new(0, 0),
                PhysicalSize::new(1280, 720),
                1.0,
            ),
        };
        let (lw, lh) = hud.size();
        let (w, h) = ((lw * scale).round() as u32, (lh * scale).round() as u32);
        let (x, y) = match anchor {
            Anchor::TopCenter => (
                origin.x + (area.width as i32 - w as i32) / 2,
                origin.y + (18.0 * scale) as i32,
            ),
            Anchor::BottomRight => (
                origin.x + area.width as i32 - w as i32 - (20.0 * scale) as i32,
                origin.y + area.height as i32 - h as i32 - (68.0 * scale) as i32,
            ),
        };
        let mut attrs = Window::default_attributes()
            .with_title("Krab")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_active(false)
            .with_inner_size(PhysicalSize::new(w, h))
            .with_position(PhysicalPosition::new(x, y));
        #[cfg(windows)]
        {
            use winit::platform::windows::{CornerPreference, WindowAttributesExtWindows};
            attrs = attrs
                .with_skip_taskbar(true)
                .with_corner_preference(CornerPreference::Round);
        }
        let Ok(window) = el.create_window(attrs) else {
            return;
        };
        let window = Rc::new(window);
        if matches!(hud, Hud::Recording { .. }) {
            // Keep the indicator out of the recording itself.
            window.set_content_protected(true);
        }
        let Some(context) = self.context(el) else {
            return;
        };
        let Ok(surface) = softbuffer::Surface::new(context, window.clone()) else {
            return;
        };
        let mut win = HudWin {
            window,
            surface,
            hud,
            scale,
            expires: ttl.map(|d| Instant::now() + d),
            hovered: false,
        };
        draw_hud(&mut win);
        win.window.request_redraw();
        self.huds.push(win);
    }

    // ---- delay -------------------------------------------------------------

    fn begin_delay(&mut self, el: &ActiveEventLoop, secs: u32, req: Request) {
        let deadline = Instant::now() + Duration::from_secs(secs as u64);
        self.delay = Some((deadline, req));
        self.show_hud(el, Hud::Countdown { deadline }, Anchor::TopCenter, None);
    }

    // ---- recording ---------------------------------------------------------

    fn start_recording(&mut self, el: &ActiveEventLoop, index: usize, native: RectI, rect: RectI) {
        let proxy = self.proxy.clone();
        let recorder = Recorder::start(
            index,
            native,
            rect,
            self.cfg.record_fps,
            self.cfg.video_dir(),
            move |result| {
                let _ = proxy.send_event(UserEvent::RecordingDone(result));
            },
        );
        let started = recorder.started;
        self.recorder = Some(recorder);
        self.show_hud(
            el,
            Hud::Recording {
                started,
                saving: false,
            },
            Anchor::TopCenter,
            None,
        );
    }

    fn stop_recording(&mut self) {
        if let Some(rec) = &self.recorder {
            rec.stop();
        }
        for win in &mut self.huds {
            if let Hud::Recording { saving, .. } = &mut win.hud {
                *saving = true;
                win.window.request_redraw();
            }
        }
    }

    // ---- window events -----------------------------------------------------

    fn overlay_event(&mut self, el: &ActiveEventLoop, idx: usize, event: WindowEvent) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        // Keys go to whichever monitor holds the selection, even if another
        // overlay window has keyboard focus.
        let key_target = session
            .wins
            .iter()
            .position(|w| w.overlay.has_selection())
            .unwrap_or(idx);
        let mut target = idx;
        match event {
            WindowEvent::RedrawRequested => {
                draw_overlay(&mut session.wins[idx]);
                return;
            }
            WindowEvent::CloseRequested => {
                self.close_session();
                return;
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                session.wins[idx].window.request_redraw();
                return;
            }
            WindowEvent::CursorMoved { position, .. } => {
                session.wins[idx]
                    .overlay
                    .pointer_moved(position.x as f32, position.y as f32);
            }
            WindowEvent::CursorLeft { .. } => session.wins[idx].overlay.pointer_left(),
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    MouseButton::Left => Button::Left,
                    MouseButton::Right => Button::Right,
                    _ => return,
                };
                let win = &mut session.wins[idx];
                // Use the position that belongs to this click (see
                // `message_cursor_pos`), not the last move event seen.
                if let (Some((sx, sy)), Ok(origin)) = (
                    crate::platform::message_cursor_pos(),
                    win.window.inner_position(),
                ) {
                    win.overlay
                        .pointer_moved((sx - origin.x) as f32, (sy - origin.y) as f32);
                }
                win.overlay
                    .pointer_button(button, state == ElementState::Pressed);
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32,
                };
                session.wins[idx].overlay.scroll(dy);
            }
            WindowEvent::ModifiersChanged(m) => {
                let st = m.state();
                self.mods = Mods {
                    shift: st.shift_key(),
                    ctrl: st.control_key() || st.super_key(),
                    alt: st.alt_key(),
                };
                for win in &mut session.wins {
                    win.overlay.set_mods(self.mods);
                }
                return;
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                target = key_target;
                let ov = &mut session.wins[target].overlay;
                let key = match &event.logical_key {
                    WinitKey::Named(NamedKey::Escape) => Some(overlay::Key::Escape),
                    WinitKey::Named(NamedKey::Enter) => Some(overlay::Key::Enter),
                    WinitKey::Named(NamedKey::Backspace) => Some(overlay::Key::Backspace),
                    WinitKey::Named(NamedKey::Delete) => Some(overlay::Key::Delete),
                    WinitKey::Named(NamedKey::ArrowLeft) => Some(overlay::Key::Left),
                    WinitKey::Named(NamedKey::ArrowRight) => Some(overlay::Key::Right),
                    WinitKey::Named(NamedKey::ArrowUp) => Some(overlay::Key::Up),
                    WinitKey::Named(NamedKey::ArrowDown) => Some(overlay::Key::Down),
                    WinitKey::Character(s) => s.chars().next().map(overlay::Key::Char),
                    _ => None,
                };
                let control_key = matches!(
                    key,
                    Some(overlay::Key::Escape | overlay::Key::Enter | overlay::Key::Backspace)
                );
                if ov.is_typing() && !self.mods.ctrl && !control_key {
                    if let Some(text) = &event.text {
                        ov.text_input(text);
                    }
                } else if let Some(key) = key {
                    ov.key(key);
                }
            }
            _ => return,
        }
        self.after_input(el, target);
    }

    fn hud_event(&mut self, idx: usize, event: WindowEvent) {
        let win = &mut self.huds[idx];
        match event {
            WindowEvent::RedrawRequested => draw_hud(win),
            WindowEvent::CursorEntered { .. } | WindowEvent::CursorMoved { .. } => {
                if !win.hovered {
                    win.hovered = true;
                    win.window.request_redraw();
                }
            }
            WindowEvent::CursorLeft { .. } => {
                win.hovered = false;
                win.window.request_redraw();
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => match &win.hud {
                Hud::Toast { action, .. } => {
                    if let Some(path) = action {
                        output::open_path(path);
                    }
                    self.huds.remove(idx);
                }
                Hud::Countdown { .. } => {
                    self.delay = None;
                    self.huds.remove(idx);
                }
                Hud::Recording { saving, .. } => {
                    if !saving {
                        self.stop_recording();
                    }
                }
            },
            _ => {}
        }
    }

    // ---- timers ------------------------------------------------------------

    fn tick(&mut self, el: &ActiveEventLoop) {
        let now = Instant::now();
        if self.session_deadline.is_some_and(|t| now >= t) {
            self.session_deadline = None;
            self.close_session();
        }
        if self.delay.as_ref().is_some_and(|(t, _)| now >= *t) {
            if let Some((_, req)) = self.delay.take() {
                self.huds
                    .retain(|h| !matches!(h.hud, Hud::Countdown { .. }));
                self.settle = Some((now + Duration::from_millis(160), req));
            }
        }
        if self.settle.as_ref().is_some_and(|(t, _)| now >= *t) {
            if let Some((_, req)) = self.settle.take() {
                self.request(el, req);
            }
        }
        self.huds
            .retain(|h| h.expires.is_none_or(|t| now < t || h.hovered));
        for win in &self.huds {
            if win.hud.animated() {
                win.window.request_redraw();
            }
        }
        if let Some(session) = self.session.as_mut() {
            for win in &mut session.wins {
                win.overlay.tick();
                if win.overlay.take_dirty() {
                    win.window.request_redraw();
                }
            }
        }
    }

    fn next_wakeup(&self) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut consider = |t: Instant| next = Some(next.map_or(t, |n| n.min(t)));
        if let Some(t) = self.session_deadline {
            consider(t);
        }
        if let Some((t, _)) = &self.delay {
            consider(*t);
        }
        if let Some((t, _)) = &self.settle {
            consider(*t);
        }
        for win in &self.huds {
            if let Some(t) = win.expires {
                consider(if win.hovered {
                    now + Duration::from_millis(400)
                } else {
                    t
                });
            }
            if win.hud.animated() {
                consider(now + Duration::from_millis(200));
            }
        }
        if let Some(session) = &self.session {
            for win in &session.wins {
                if let Some(t) = win.overlay.next_wakeup() {
                    consider(t);
                }
            }
        }
        next
    }

    fn menu(&mut self, el: &ActiveEventLoop, id: &str) {
        let settle = |app: &mut App, req: Request| {
            // Give the tray menu time to fade out before the screen is frozen.
            app.settle = Some((Instant::now() + Duration::from_millis(220), req));
        };
        match id {
            "snip" => settle(self, Request::default()),
            "rect" | "window" | "full" | "freeform" => settle(
                self,
                Request {
                    mode: Mode::parse(id),
                    ..Request::default()
                },
            ),
            "delay" => self.request(
                el,
                Request {
                    delay: self.cfg.delay_seconds,
                    ..Request::default()
                },
            ),
            "record" => settle(
                self,
                Request {
                    kind: Kind::Record,
                    ..Request::default()
                },
            ),
            "folder" => {
                let dir = self.cfg.save_dir();
                let _ = std::fs::create_dir_all(&dir);
                output::open_path(&dir);
            }
            "config" => {
                if let Ok(path) = Config::ensure_file() {
                    output::open_path(&path);
                }
            }
            "autostart" => {
                let enable = !crate::platform::autostart::is_enabled();
                let _ = crate::platform::autostart::set(enable);
                #[cfg(any(windows, target_os = "macos"))]
                if let Some(tray) = &self.tray {
                    tray.autostart
                        .set_checked(crate::platform::autostart::is_enabled());
                }
            }
            "quit" => el.exit(),
            _ => {}
        }
    }

    fn start_daemon(&mut self, el: &ActiveEventLoop) {
        let proxy = self.proxy.clone();
        if let Err(e) = crate::ipc::listen(move |msg| {
            let _ = proxy.send_event(UserEvent::Message(msg));
        }) {
            eprintln!("krab: could not listen for requests: {e}");
        }
        #[cfg(any(windows, target_os = "macos"))]
        {
            match tray::Tray::create(&self.cfg, self.proxy.clone()) {
                Ok(tray) => {
                    if !self.launch.quiet {
                        let detail = match tray.hotkeys.first() {
                            Some(h) => format!("Press {h} to take a snip"),
                            None => "Click the tray icon to take a snip".into(),
                        };
                        self.toast(el, "Krab is running".into(), detail, None, None);
                    }
                    self.tray = Some(tray);
                }
                Err(e) => eprintln!("krab: tray unavailable: {e}"),
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            let _ = el;
            eprintln!("krab: running in the background. Bind `krab` to a keyboard shortcut in your desktop settings.");
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.started {
            return;
        }
        self.started = true;
        // Build the overlay windows now, in parallel with the screen capture.
        if self.launch.daemon || self.capturing {
            self.sync_pool(el);
        }
        if self.launch.daemon {
            self.start_daemon(el);
        }
        if let Some(req) = self.launch.initial.take() {
            self.request(el, req);
        }
    }

    fn user_event(&mut self, el: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Captured(result, req) => {
                self.capturing = false;
                match *result {
                    Ok(cap) => self.open_session(el, cap, req),
                    Err(e) => {
                        eprintln!("krab: {e}");
                        self.toast(el, "Screen capture failed".into(), e, None, None);
                    }
                }
            }
            UserEvent::Message(Message::Snip(req)) => self.request(el, req),
            UserEvent::Message(Message::Quit) => el.exit(),
            UserEvent::Message(Message::Ping) => {}
            UserEvent::Hotkey | UserEvent::TrayClick => self.request(el, Request::default()),
            UserEvent::Menu(id) => self.menu(el, &id),
            UserEvent::RecordingDone(result) => {
                self.recorder = None;
                self.huds
                    .retain(|h| !matches!(h.hud, Hud::Recording { .. }));
                match result {
                    Ok(path) => {
                        let name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        self.toast(
                            el,
                            "Recording saved".into(),
                            format!("{name} - click to open"),
                            None,
                            Some(path),
                        );
                    }
                    Err(e) => self.toast(el, "Recording failed".into(), e, None, None),
                }
            }
            UserEvent::TextExtracted(result) => {
                self.busy = self.busy.saturating_sub(1);
                match result {
                    Ok(text) if !text.trim().is_empty() => {
                        let _ = output::copy_text(&text);
                        let preview: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
                        self.toast(el, "Text copied to clipboard".into(), preview, None, None);
                    }
                    Ok(_) => self.toast(
                        el,
                        "No text found".into(),
                        "Nothing was copied".into(),
                        None,
                        None,
                    ),
                    Err(e) => self.toast(el, "Could not read text".into(), e, None, None),
                }
            }
        }
    }

    fn window_event(&mut self, el: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(idx) = self
            .session
            .as_ref()
            .and_then(|s| s.wins.iter().position(|w| w.window.id() == id))
        {
            self.overlay_event(el, idx, event);
        } else if let Some(idx) = self.huds.iter().position(|h| h.window.id() == id) {
            self.hud_event(idx, event);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some((kind, img)) = self.pending.take() {
            self.deliver(el, kind, img);
        }
        self.tick(el);
        if !self.launch.daemon && self.idle() {
            el.exit();
            return;
        }
        el.set_control_flow(match self.next_wakeup() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

#[cfg(any(windows, target_os = "macos"))]
mod tray {
    use super::UserEvent;
    use crate::config::Config;
    use global_hotkey::hotkey::HotKey;
    use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState};
    use std::str::FromStr;
    use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
    use tray_icon::{
        Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
    };
    use winit::event_loop::EventLoopProxy;

    pub struct Tray {
        _icon: TrayIcon,
        _manager: Option<GlobalHotKeyManager>,
        pub autostart: CheckMenuItem,
        /// Hotkeys that were registered successfully, as written in the config.
        pub hotkeys: Vec<String>,
    }

    impl Tray {
        pub fn create(cfg: &Config, proxy: EventLoopProxy<UserEvent>) -> Result<Tray, String> {
            // Global hotkeys first, so the menu can show which one is live.
            let manager = GlobalHotKeyManager::new().ok();
            let mut hotkeys = Vec::new();
            if let Some(manager) = &manager {
                for spec in [&cfg.hotkey, &cfg.hotkey_alt] {
                    let spec = spec.trim();
                    if spec.is_empty() {
                        continue;
                    }
                    match HotKey::from_str(spec) {
                        Ok(hk) if manager.register(hk).is_ok() => hotkeys.push(spec.to_string()),
                        Ok(_) => {
                            eprintln!("krab: hotkey '{spec}' is already in use by another program")
                        }
                        Err(e) => eprintln!("krab: cannot parse hotkey '{spec}': {e}"),
                    }
                }
            }
            let p = proxy.clone();
            GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
                if e.state() == HotKeyState::Pressed {
                    let _ = p.send_event(UserEvent::Hotkey);
                }
            }));

            let item = |id: &str, text: &str| MenuItem::with_id(id, text, true, None);
            let snip_label = match hotkeys.first() {
                Some(h) => format!("New snip\t{h}"),
                None => "New snip".to_string(),
            };
            let autostart = CheckMenuItem::with_id(
                "autostart",
                "Start with system",
                true,
                crate::platform::autostart::is_enabled(),
                None,
            );
            let menu = Menu::new();
            menu.append_items(&[
                &item("snip", &snip_label),
                &PredefinedMenuItem::separator(),
                &item("rect", "Rectangle"),
                &item("window", "Window"),
                &item("full", "Full screen"),
                &item("freeform", "Freeform"),
                &PredefinedMenuItem::separator(),
                &item("delay", &format!("Snip in {} seconds", cfg.delay_seconds)),
                &item("record", "Record screen"),
                &PredefinedMenuItem::separator(),
                &item("folder", "Open screenshots folder"),
                &item("config", "Settings..."),
                &autostart,
                &PredefinedMenuItem::separator(),
                &item("quit", "Quit Krab"),
            ])
            .map_err(|e| e.to_string())?;

            let p = proxy.clone();
            MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
                let _ = p.send_event(UserEvent::Menu(e.id.0.clone()));
            }));
            let p = proxy;
            TrayIconEvent::set_event_handler(Some(move |e: TrayIconEvent| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = e
                {
                    let _ = p.send_event(UserEvent::TrayClick);
                }
            }));

            let size = 64;
            let icon = Icon::from_rgba(crate::brand::mark_rgba(size, false), size, size)
                .map_err(|e| e.to_string())?;
            let tooltip = match hotkeys.first() {
                Some(h) => format!("Krab - {h} to snip"),
                None => "Krab".to_string(),
            };
            let tray = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip(tooltip)
                .with_icon(icon)
                .build()
                .map_err(|e| e.to_string())?;
            Ok(Tray {
                _icon: tray,
                _manager: manager,
                autostart,
                hotkeys,
            })
        }
    }
}
