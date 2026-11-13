//! The windowing layer: turns captures into overlay windows, routes input to
//! [`Overlay`], and carries out what the user decided.

use crate::capture::{self, Capture, Shot};
use crate::config::Config;
use crate::geom::RectI;
use crate::hotkey::{self, Hotkeys, Slot};
use crate::hud::Hud;
use crate::ipc::{Message, Request};
use crate::output::{self, Image};
use crate::overlay::{
    self, Button, CursorKind, Finish, Kind, Mode, Mods, Options, Outcome, Overlay, Shared,
};
use crate::record::Recorder;
use crate::settings::{self, Settings};
use crate::trace;
use crate::update;
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
    /// A global shortcut was pressed (its id).
    Hotkey(u32),
    Menu(String),
    TrayClick,
    RecordingDone(Result<PathBuf, String>),
    TextExtracted(Result<String, String>),
    /// An update check finished; `true` when the user asked for it.
    Update(Result<update::Outcome, String>, bool),
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
    /// This background instance replaces one that installed an update.
    pub updated: bool,
    /// Open the settings window right away.
    pub open_settings: bool,
    /// Krab was just installed as an app: welcome the user.
    pub installed: bool,
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

// Toasts are native notifications on Linux, so only one anchor is used there.
#[cfg_attr(target_os = "linux", allow(dead_code))]
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
    tray: Option<tray::Tray>,
    hotkeys: Option<Hotkeys>,
    hotkey_status: [hotkey::Status; 3],
    settings: Option<SettingsWin>,
    /// Modifier keys, as the settings window sees them.
    key_mods: hotkey::Mods,
}

struct SettingsWin {
    window: Rc<Window>,
    surface: Surface,
    ui: Settings,
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
        tray: None,
        hotkeys: None,
        hotkey_status: [
            hotkey::Status::Off,
            hotkey::Status::Off,
            hotkey::Status::Off,
        ],
        settings: None,
        key_mods: hotkey::Mods::default(),
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
    let window = el.create_window(attrs).ok()?;
    crate::platform::suppress_caption_paint(&window);
    Some(Rc::new(window))
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
            && self.settings.is_none()
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
                // Render first, then show and blit while cloaked: the overlay
                // appears with its first frame already in place.
                crate::platform::cloak(&win.window, true);
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
            crate::platform::cloak(&win.window, false);
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
            let thumb = img.map(|i| crate::hud::thumbnail(i, 320, 220));
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
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut attrs = Window::default_attributes()
            .with_title("Krab")
            .with_decorations(false)
            .with_resizable(false)
            .with_window_level(WindowLevel::AlwaysOnTop)
            .with_active(false)
            .with_visible(false)
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
        crate::platform::suppress_caption_paint(&window);
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
        // Created hidden: show it cloaked and uncloak it with its first frame
        // in place, so no white bar or empty window flashes up first.
        crate::platform::cloak(&win.window, true);
        win.window.set_visible(true);
        draw_hud(&mut win);
        crate::platform::cloak(&win.window, false);
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
            "settings" | "config" => self.open_settings(el),
            "update" => self.check_for_updates(),
            "quit" => el.exit(),
            _ => {}
        }
    }

    /// Look for a new version shortly after start and then once a day.
    fn start_update_checks(&self) {
        let enabled = self.cfg.auto_update && update::enabled_here();
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            update::cleanup();
            crate::install::refresh();
            if !enabled {
                return;
            }
            // Stay out of the way while the desktop is still starting up.
            std::thread::sleep(Duration::from_secs(20));
            loop {
                if update::due() {
                    update::mark_checked();
                    let result = update::check_and_install();
                    let installed = matches!(result, Ok(update::Outcome::Updated { .. }));
                    let _ = proxy.send_event(UserEvent::Update(result, false));
                    if installed {
                        return;
                    }
                }
                std::thread::sleep(Duration::from_secs(60 * 60));
            }
        });
    }

    fn check_for_updates(&mut self) {
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            update::mark_checked();
            let _ = proxy.send_event(UserEvent::Update(update::check_and_install(), true));
        });
    }

    fn start_daemon(&mut self, el: &ActiveEventLoop) {
        let proxy = self.proxy.clone();
        if let Err(e) = crate::ipc::listen(move |msg| {
            let _ = proxy.send_event(UserEvent::Message(msg));
        }) {
            // Another instance owns the background role; finish what this one
            // was started for and leave.
            eprintln!("krab: could not listen for requests: {e}");
            self.launch.daemon = false;
            return;
        }
        self.start_update_checks();

        let mut hotkeys = Hotkeys::new();
        self.hotkey_status = hotkeys.apply(&self.cfg);
        self.hotkeys = Some(hotkeys);
        let p = self.proxy.clone();
        global_hotkey::GlobalHotKeyEvent::set_event_handler(Some(
            move |e: global_hotkey::GlobalHotKeyEvent| {
                if e.state() == global_hotkey::HotKeyState::Pressed {
                    let _ = p.send_event(UserEvent::Hotkey(e.id()));
                }
            },
        ));

        let snip_key = self.active_snip_hotkey();
        match tray::Tray::create(
            snip_key.as_deref(),
            self.cfg.delay_seconds,
            self.proxy.clone(),
        ) {
            Ok(t) => self.tray = Some(t),
            Err(e) => eprintln!("krab: tray unavailable: {e}"),
        }

        if self.launch.installed {
            // A tray app is expected to be there after logging in.
            let _ = crate::platform::autostart::set(true);
            let place = if cfg!(windows) {
                "Find it in the Start menu and the tray"
            } else if cfg!(target_os = "macos") {
                "Find it in Applications and the menu bar"
            } else {
                "Find it in your app menu and the tray"
            };
            self.toast(el, "Krab is installed".into(), place.into(), None, None);
            self.launch.open_settings = true;
        } else if self.launch.updated {
            self.toast(
                el,
                format!("Krab was updated to {}", env!("CARGO_PKG_VERSION")),
                "The new version is running".into(),
                None,
                None,
            );
        } else if !self.launch.quiet {
            let detail = match snip_key {
                Some(h) => format!("Press {} to take a snip", hotkey::display(&h)),
                None if self.tray.is_some() => "Click the tray icon to take a snip".into(),
                None => "Run krab to take a snip".into(),
            };
            self.toast(el, "Krab is running".into(), detail, None, None);
        }
        if self.launch.open_settings {
            self.open_settings(el);
        }
    }

    /// The first working "new snip" shortcut, as written in the config.
    fn active_snip_hotkey(&self) -> Option<String> {
        [Slot::Snip, Slot::SnipAlt]
            .into_iter()
            .zip(&self.hotkey_status)
            .find(|(_, st)| **st == hotkey::Status::Active)
            .map(|(slot, _)| slot.get(&self.cfg).to_string())
    }

    fn reapply_hotkeys(&mut self) {
        if let Some(h) = &mut self.hotkeys {
            self.hotkey_status = h.apply(&self.cfg);
        } else {
            // A one-shot instance registers nothing; still report typos.
            let mut h = Hotkeys::new();
            self.hotkey_status = h.apply(&self.cfg);
            h.suspend();
        }
        let snip_key = self.active_snip_hotkey();
        if let Some(t) = &self.tray {
            t.set_hotkey(snip_key.as_deref());
        }
        if let Some(sw) = &mut self.settings {
            sw.ui.status = self.hotkey_status.clone();
            sw.ui.invalidate();
        }
    }

    // ---- settings window ---------------------------------------------------

    fn open_settings(&mut self, el: &ActiveEventLoop) {
        if let Some(sw) = &self.settings {
            sw.window.set_minimized(false);
            sw.window.focus_window();
            return;
        }
        let (w, h) = settings::SIZE;
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut attrs = Window::default_attributes()
            .with_title("Krab Settings")
            .with_inner_size(winit::dpi::LogicalSize::new(w, h))
            // Krab draws its own title bar.
            .with_decorations(false)
            .with_resizable(false)
            .with_visible(false)
            .with_window_icon(
                winit::window::Icon::from_rgba(crate::brand::mark_rgba(64, true), 64, 64).ok(),
            );
        #[cfg(windows)]
        {
            use winit::platform::windows::CornerPreference;
            use winit::platform::windows::WindowAttributesExtWindows;
            attrs = attrs
                .with_undecorated_shadow(true)
                .with_corner_preference(CornerPreference::Round)
                .with_taskbar_icon(
                    winit::window::Icon::from_rgba(crate::brand::mark_rgba(64, true), 64, 64).ok(),
                )
                .with_drag_and_drop(false);
        }
        let Ok(window) = el.create_window(attrs) else {
            return;
        };
        crate::platform::suppress_caption_paint(&window);
        let window = Rc::new(window);
        let Some(context) = self.context(el) else {
            return;
        };
        let Ok(surface) = softbuffer::Surface::new(context, window.clone()) else {
            return;
        };
        let ui = Settings::new(
            self.cfg.clone(),
            self.hotkey_status.clone(),
            crate::platform::autostart::is_enabled(),
            crate::platform::print_screen_opens_snipping_tool(),
        );
        let mut sw = SettingsWin {
            window,
            surface,
            ui,
        };
        // Created hidden: show it cloaked and uncloak it with its first frame
        // in place, so no white title bar flashes up first.
        crate::platform::cloak(&sw.window, true);
        sw.window.set_visible(true);
        draw_settings(&mut sw);
        crate::platform::cloak(&sw.window, false);
        sw.window.focus_window();
        sw.window.request_redraw();
        self.settings = Some(sw);
        if self.hotkeys.is_none() {
            self.reapply_hotkeys();
        }
    }

    fn close_settings(&mut self) {
        if let Some(sw) = self.settings.take() {
            if sw.ui.is_recording() {
                self.reapply_hotkeys();
            }
        }
    }

    fn settings_event(&mut self, event: WindowEvent) {
        let Some(sw) = self.settings.as_mut() else {
            return;
        };
        let scale = sw.window.scale_factor() as f32;
        let mut actions = Vec::new();
        match event {
            WindowEvent::RedrawRequested => {
                draw_settings(sw);
                return;
            }
            WindowEvent::CloseRequested => {
                self.close_settings();
                return;
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                sw.ui.invalidate();
            }
            WindowEvent::CursorMoved { position, .. } => {
                sw.ui
                    .pointer_moved(position.x as f32 / scale, position.y as f32 / scale);
                sw.window.set_cursor(if sw.ui.hovering_control() {
                    CursorIcon::Pointer
                } else {
                    CursorIcon::Default
                });
            }
            WindowEvent::CursorLeft { .. } => sw.ui.pointer_left(),
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                if sw.ui.press().is_some() {
                    let _ = sw.window.drag_window();
                }
                return;
            }
            WindowEvent::MouseInput {
                state: ElementState::Released,
                button: MouseButton::Left,
                ..
            } => actions = sw.ui.click(),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 48.0,
                };
                sw.ui.scroll(lines);
            }
            WindowEvent::ModifiersChanged(m) => {
                let st = m.state();
                self.key_mods = hotkey::Mods {
                    ctrl: st.control_key(),
                    alt: st.alt_key(),
                    shift: st.shift_key(),
                    meta: st.super_key(),
                };
                sw.ui.modifiers(self.key_mods);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                use winit::keyboard::{KeyCode, PhysicalKey};
                let PhysicalKey::Code(code) = event.physical_key else {
                    return;
                };
                // Windows reports only the release of Print Screen.
                let pressed = event.state == ElementState::Pressed
                    || (code == KeyCode::PrintScreen && !event.repeat);
                if !pressed || event.repeat {
                    return;
                }
                if !sw.ui.is_recording() && code == KeyCode::Escape {
                    self.close_settings();
                    return;
                }
                actions = sw.ui.key(code, self.key_mods);
            }
            _ => return,
        }
        if sw.ui.take_dirty() {
            sw.window.request_redraw();
        }
        for action in actions {
            self.settings_action(action);
        }
    }

    fn settings_action(&mut self, action: settings::Action) {
        use settings::Action;
        match action {
            Action::Close => {
                self.close_settings();
                return;
            }
            Action::Minimize => {
                if let Some(sw) = &self.settings {
                    sw.window.set_minimized(true);
                }
            }
            Action::DragWindow => {
                if let Some(sw) = &self.settings {
                    let _ = sw.window.drag_window();
                }
            }
            Action::Save => {
                if let Some(sw) = &self.settings {
                    self.cfg = sw.ui.cfg.clone();
                }
                if let Err(e) = self.cfg.save() {
                    eprintln!("krab: could not save the settings: {e}");
                }
                self.reapply_hotkeys();
            }
            Action::SuspendHotkeys => {
                if let Some(h) = &mut self.hotkeys {
                    h.suspend();
                }
            }
            Action::ResumeHotkeys => self.reapply_hotkeys(),
            Action::PickFolder => {
                let current = self.cfg.save_dir();
                let mut dialog = rfd::FileDialog::new().set_title("Save snips to");
                if current.is_dir() {
                    dialog = dialog.set_directory(&current);
                }
                if let Some(dir) = dialog.pick_folder() {
                    if let Some(sw) = &mut self.settings {
                        sw.ui.cfg.save_dir = dir.display().to_string();
                    }
                    self.settings_action(Action::Save);
                }
            }
            Action::OpenConfigFile => {
                if let Ok(path) = Config::ensure_file() {
                    output::open_path(&path);
                }
            }
            Action::CheckForUpdates => self.check_for_updates(),
            Action::SetAutostart(on) => {
                if let Err(e) = crate::platform::autostart::set(on) {
                    eprintln!("krab: autostart: {e}");
                }
                if let Some(sw) = &mut self.settings {
                    sw.ui.autostart = crate::platform::autostart::is_enabled();
                }
            }
            Action::DisableWindowsSnipping => {
                let result = crate::platform::disable_snipping_tool_on_print_screen();
                if let Some(sw) = &mut self.settings {
                    sw.ui.windows_snipping = crate::platform::print_screen_opens_snipping_tool();
                    sw.ui.update_note = match result {
                        Ok(()) => "Print Screen now belongs to Krab".into(),
                        Err(e) => format!("Could not change the Windows setting: {e}"),
                    };
                }
            }
        }
        if let Some(sw) = &mut self.settings {
            sw.ui.invalidate();
            sw.window.request_redraw();
        }
    }
}

impl App {
    fn update_result(
        &mut self,
        el: &ActiveEventLoop,
        result: Result<update::Outcome, String>,
        manual: bool,
    ) {
        let in_settings = self.settings.is_some();
        match result {
            Ok(update::Outcome::Updated { exe, .. }) => {
                // Restarts into the new version as soon as nothing is
                // in progress; the new instance confirms the update.
                update::request_restart(exe);
            }
            Ok(update::Outcome::UpToDate(v)) if manual && !in_settings => self.toast(
                el,
                "Krab is up to date".into(),
                format!("Version {v} is the newest"),
                None,
                None,
            ),
            Ok(_) => {}
            Err(e) if manual && !in_settings => {
                self.toast(el, "Update failed".into(), e, None, None)
            }
            Err(e) => eprintln!("krab: update check failed: {e}"),
        }
    }
}

fn draw_settings(sw: &mut SettingsWin) {
    let size = sw.window.inner_size();
    let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
        return;
    };
    if sw.surface.resize(w, h).is_err() {
        return;
    }
    let Ok(mut buf) = sw.surface.buffer_mut() else {
        return;
    };
    let scale = sw.window.scale_factor() as f32;
    sw.ui.render(&mut buf, size.width, size.height, scale);
    let _ = buf.present();
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
            UserEvent::Message(Message::Settings) => self.open_settings(el),
            UserEvent::Message(Message::Updated(path)) => {
                let path = std::path::PathBuf::from(path);
                let same = |p: &Path| p.canonicalize().ok();
                let mine = std::env::current_exe().ok().and_then(|p| same(&p));
                // On Linux the running file was replaced, so compare names
                // without the " (deleted)" the kernel appends.
                let mine = mine.or_else(|| {
                    std::env::current_exe()
                        .ok()
                        .map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(" (deleted)")))
                });
                if mine.is_some() && mine == same(&path).or(Some(path.clone())) {
                    update::request_restart(path);
                }
            }
            UserEvent::TrayClick => self.request(el, Request::default()),
            UserEvent::Hotkey(id) => {
                let slot = self.hotkeys.as_ref().and_then(|h| h.slot(id));
                let req = match slot {
                    Some(Slot::Record) => Request {
                        kind: Kind::Record,
                        ..Request::default()
                    },
                    Some(_) => Request::default(),
                    None => return,
                };
                self.request(el, req);
            }
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
            UserEvent::Update(result, manual) => {
                if let Some(sw) = &mut self.settings {
                    sw.ui.update_note = match &result {
                        Ok(update::Outcome::Updated { to, .. }) => {
                            format!("Version {to} installed - restarts when this window closes")
                        }
                        Ok(update::Outcome::UpToDate(v)) => format!("Version {v} is the newest"),
                        Err(e) => format!("Update failed: {e}"),
                    };
                    sw.ui.invalidate();
                    sw.window.request_redraw();
                }
                self.update_result(el, result, manual);
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
        } else if self.settings.as_ref().is_some_and(|s| s.window.id() == id) {
            self.settings_event(event);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if let Some((kind, img)) = self.pending.take() {
            self.deliver(el, kind, img);
        }
        self.tick(el);
        if (!self.launch.daemon || update::restart_requested().is_some()) && self.idle() {
            el.exit();
            return;
        }
        el.set_control_flow(match self.next_wakeup() {
            Some(t) => ControlFlow::WaitUntil(t),
            None => ControlFlow::Wait,
        });
    }
}

/// The tray icon (Windows notification area, macOS menu bar, Linux
/// StatusNotifierItem) and its menu.
mod tray {
    use super::UserEvent;
    use winit::event_loop::EventLoopProxy;

    /// Menu entries: (id, label). `None` is a separator.
    fn entries(snip_hotkey: Option<&str>, delay: u32) -> Vec<Option<(&'static str, String)>> {
        let snip = match snip_hotkey {
            Some(h) if cfg!(windows) => format!("New snip\t{}", crate::hotkey::display(h)),
            Some(h) => format!("New snip ({})", crate::hotkey::display(h)),
            None => "New snip".into(),
        };
        vec![
            Some(("snip", snip)),
            None,
            Some(("rect", "Rectangle".into())),
            Some(("window", "Window".into())),
            Some(("full", "Full screen".into())),
            Some(("freeform", "Freeform".into())),
            None,
            Some(("delay", format!("Snip in {delay} seconds"))),
            Some(("record", "Record screen".into())),
            None,
            Some(("folder", "Open screenshots folder".into())),
            Some(("settings", "Settings...".into())),
            Some(("update", "Check for updates".into())),
            None,
            Some(("quit", "Quit Krab".into())),
        ]
    }

    #[cfg(any(windows, target_os = "macos"))]
    pub struct Tray {
        icon: tray_icon::TrayIcon,
        snip: tray_icon::menu::MenuItem,
    }

    #[cfg(any(windows, target_os = "macos"))]
    impl Tray {
        pub fn create(
            snip_hotkey: Option<&str>,
            delay: u32,
            proxy: EventLoopProxy<UserEvent>,
        ) -> Result<Tray, String> {
            use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
            use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            let menu = Menu::new();
            let mut snip = None;
            for entry in entries(snip_hotkey, delay) {
                match entry {
                    None => menu.append(&PredefinedMenuItem::separator()),
                    Some((id, label)) => {
                        let item = MenuItem::with_id(id, &label, true, None);
                        if id == "snip" {
                            snip = Some(item.clone());
                        }
                        menu.append(&item)
                    }
                }
                .map_err(|e| e.to_string())?;
            }

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
            let icon = TrayIconBuilder::new()
                .with_menu(Box::new(menu))
                .with_menu_on_left_click(false)
                .with_tooltip(tooltip(snip_hotkey))
                .with_icon(icon)
                .build()
                .map_err(|e| e.to_string())?;
            Ok(Tray {
                icon,
                snip: snip.ok_or("menu without snip entry")?,
            })
        }

        /// Show the current snip shortcut in the menu and tooltip.
        pub fn set_hotkey(&self, snip_hotkey: Option<&str>) {
            if let Some(Some((_, label))) = entries(snip_hotkey, 0).into_iter().next() {
                self.snip.set_text(label);
            }
            let _ = self.icon.set_tooltip(Some(tooltip(snip_hotkey)));
        }
    }

    fn tooltip(snip_hotkey: Option<&str>) -> String {
        match snip_hotkey {
            Some(h) => format!("Krab - {} to snip", crate::hotkey::display(h)),
            None => "Krab".to_string(),
        }
    }

    #[cfg(target_os = "linux")]
    pub struct Tray {
        handle: ksni::blocking::Handle<Sni>,
    }

    #[cfg(target_os = "linux")]
    pub struct Sni {
        proxy: EventLoopProxy<UserEvent>,
        snip_hotkey: Option<String>,
        delay: u32,
    }

    #[cfg(target_os = "linux")]
    impl ksni::Tray for Sni {
        fn id(&self) -> String {
            "krab".into()
        }

        fn title(&self) -> String {
            "Krab".into()
        }

        fn category(&self) -> ksni::Category {
            ksni::Category::ApplicationStatus
        }

        fn icon_pixmap(&self) -> Vec<ksni::Icon> {
            [22u32, 32, 48, 64]
                .iter()
                .map(|&size| {
                    // StatusNotifierItem wants ARGB32 in network byte order.
                    let rgba = crate::brand::mark_rgba(size, false);
                    let data = rgba
                        .chunks_exact(4)
                        .flat_map(|p| [p[3], p[0], p[1], p[2]])
                        .collect();
                    ksni::Icon {
                        width: size as i32,
                        height: size as i32,
                        data,
                    }
                })
                .collect()
        }

        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                title: tooltip(self.snip_hotkey.as_deref()),
                ..Default::default()
            }
        }

        fn activate(&mut self, _x: i32, _y: i32) {
            let _ = self.proxy.send_event(UserEvent::TrayClick);
        }

        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::StandardItem;
            entries(self.snip_hotkey.as_deref(), self.delay)
                .into_iter()
                .map(|e| match e {
                    None => ksni::MenuItem::Separator,
                    Some((id, label)) => StandardItem {
                        label,
                        activate: Box::new(move |t: &mut Sni| {
                            let _ = t.proxy.send_event(UserEvent::Menu(id.to_string()));
                        }),
                        ..Default::default()
                    }
                    .into(),
                })
                .collect()
        }
    }

    #[cfg(target_os = "linux")]
    impl Tray {
        pub fn create(
            snip_hotkey: Option<&str>,
            delay: u32,
            proxy: EventLoopProxy<UserEvent>,
        ) -> Result<Tray, String> {
            use ksni::blocking::TrayMethods;
            let sni = Sni {
                proxy,
                snip_hotkey: snip_hotkey.map(str::to_string),
                delay,
            };
            // Desktops without a tray (plain GNOME) simply do not show it.
            let handle = sni
                .assume_sni_available(true)
                .spawn()
                .map_err(|e| e.to_string())?;
            Ok(Tray { handle })
        }

        pub fn set_hotkey(&self, snip_hotkey: Option<&str>) {
            let h = snip_hotkey.map(str::to_string);
            self.handle.update(move |t| t.snip_hotkey = h);
        }
    }
}
