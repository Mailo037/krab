//! The settings window: shortcuts, capture behaviour and app options.
//!
//! Like the overlay it is a pure state machine - pointer and key events in,
//! pixels and [`Action`]s out - so it can be tested without a window. Every
//! change is applied immediately; there is no OK / Cancel.

use crate::config::Config;
use crate::geom::{Pt, RectF};
use crate::gfx::icons::{self, Icon};
use crate::gfx::{text, theme, Canvas, Color};
use crate::hotkey::{self, Mods, Recorded, Slot, Status};
use crate::hud::ellipsize;
use winit::keyboard::KeyCode;

/// Logical size of the window.
pub const SIZE: (f32, f32) = (540.0, 640.0);

const PAD: f32 = 22.0;
const HEADER: f32 = 42.0;
const SECTION_GAP: f32 = 24.0;
const CARD_RADIUS: f32 = 12.0;

const CARD: Color = Color::rgb(0x25, 0x26, 0x2C);
const CONTROL: Color = Color::rgba(255, 255, 255, 16);
const CONTROL_HOVER: Color = Color::rgba(255, 255, 255, 30);
const OK: Color = Color::rgb(0x4C, 0xD9, 0x64);

/// What the app has to do after an input.
#[derive(Clone, Debug, PartialEq)]
pub enum Action {
    /// `cfg` changed: write it and apply it.
    Save,
    /// A shortcut is being recorded: release the global shortcuts meanwhile.
    SuspendHotkeys,
    /// Recording was cancelled: register the shortcuts again.
    ResumeHotkeys,
    PickFolder,
    OpenConfigFile,
    CheckForUpdates,
    SetAutostart(bool),
    DisableWindowsSnipping,
    /// The title bar was grabbed: let the system move the window.
    DragWindow,
    Minimize,
    Close,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Toggle {
    Copy,
    AutoSave,
    Notifications,
    Border,
    Loupe,
    Precapture,
    Background,
    Autostart,
    AutoUpdate,
    Frame,
    History,
    RecordCursor,
    RecordClicks,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Choice {
    AfterCapture,
    Format,
    Mode,
    ColorFormat,
    Audio,
    Encoder,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hit {
    Shortcut(Slot),
    Clear(Slot),
    Toggle(Toggle),
    Choice(Choice, usize),
    ChangeFolder,
    OpenConfig,
    CheckUpdates,
    DisableWinSnip,
    Minimize,
    Close,
}

const AFTER_CAPTURE: [(&str, &str); 2] = [("edit", "Annotate"), ("copy", "Copy at once")];
const FORMATS: [(&str, &str); 4] = [
    ("png", "PNG"),
    ("jpg", "JPG"),
    ("bmp", "BMP"),
    ("gif", "GIF"),
];
const MODES: [(&str, &str); 4] = [
    ("rect", "Area"),
    ("window", "Window"),
    ("full", "Screen"),
    ("freeform", "Freeform"),
];
const COLOR_FORMATS: [(&str, &str); 3] = [("hex", "HEX"), ("rgb", "RGB"), ("hsl", "HSL")];
const AUDIO: [(&str, &str); 4] = [
    ("off", "Off"),
    ("system", "Computer"),
    ("mic", "Mic"),
    ("both", "Both"),
];
const ENCODERS: [(&str, &str); 2] = [("auto", "Graphics card"), ("software", "CPU")];

impl Choice {
    fn options(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Choice::AfterCapture => &AFTER_CAPTURE,
            Choice::Format => &FORMATS,
            Choice::Mode => &MODES,
            Choice::ColorFormat => &COLOR_FORMATS,
            Choice::Audio => &AUDIO,
            Choice::Encoder => &ENCODERS,
        }
    }

    fn value(self, cfg: &Config) -> &str {
        match self {
            Choice::AfterCapture => {
                if cfg.quick_copy() {
                    "copy"
                } else {
                    "edit"
                }
            }
            Choice::Format => &cfg.file_format,
            Choice::Mode => &cfg.default_mode,
            Choice::ColorFormat => &cfg.color_format,
            Choice::Audio => &cfg.record_audio,
            Choice::Encoder => &cfg.record_encoder,
        }
    }

    fn set(self, cfg: &mut Config, v: &str) {
        match self {
            Choice::AfterCapture => cfg.after_capture = v.into(),
            Choice::Format => cfg.file_format = v.into(),
            Choice::Mode => cfg.default_mode = v.into(),
            Choice::ColorFormat => cfg.color_format = v.into(),
            Choice::Audio => cfg.record_audio = v.into(),
            Choice::Encoder => cfg.record_encoder = v.into(),
        }
    }
}

impl Toggle {
    fn label(self) -> &'static str {
        match self {
            Toggle::Copy => "Copy to clipboard",
            Toggle::AutoSave => "Save every snip automatically",
            Toggle::Notifications => "Show a confirmation",
            Toggle::Border => "Draw a border around snips",
            Toggle::Loupe => {
                if cfg!(target_os = "macos") {
                    "Colour magnifier (tap Option to toggle)"
                } else {
                    "Colour magnifier (tap Alt to toggle)"
                }
            }
            Toggle::Precapture => "Capture covered windows as the snip starts",
            Toggle::Background => {
                if cfg!(target_os = "macos") {
                    "Keep running in the menu bar"
                } else {
                    "Keep running in the tray"
                }
            }
            Toggle::Autostart => "Start when I log in",
            Toggle::Frame => "Put snips on a background",
            Toggle::History => "Remember recent snips in the tray",
            Toggle::RecordCursor => "Show the mouse pointer",
            Toggle::RecordClicks => "Highlight the pointer and clicks",
            Toggle::AutoUpdate => "Install updates automatically",
        }
    }
}

/// One line of the window.
enum Row {
    Shortcut(Slot),
    WindowsSnipping,
    Choice(Choice, &'static str),
    Toggle(Toggle),
    Folder,
    About,
}

impl Row {
    fn height(&self) -> f32 {
        match self {
            Row::Shortcut(_) | Row::WindowsSnipping | Row::Folder | Row::About => 58.0,
            Row::Choice(..) => 52.0,
            Row::Toggle(_) => 46.0,
        }
    }
}

pub struct Settings {
    pub cfg: Config,
    /// Result of registering each shortcut (in [`Slot::ALL`] order).
    pub status: crate::hotkey::Statuses,
    pub autostart: bool,
    /// Windows grabs Print Screen for its own Snipping Tool.
    pub windows_snipping: bool,
    /// Shown next to "Check for updates".
    pub update_note: String,
    recording: Option<Slot>,
    pending: String,
    reject: Option<&'static str>,
    scroll: f32,
    /// Logical pointer position in window coordinates.
    pointer: Option<Pt>,
    view: (f32, f32),
    hits: Vec<(RectF, Hit)>,
    /// Window buttons in the title bar (not scrolled).
    title_hits: Vec<(RectF, Hit)>,
    content_h: f32,
    dirty: bool,
}

impl Settings {
    pub fn new(
        cfg: Config,
        status: crate::hotkey::Statuses,
        autostart: bool,
        windows_snipping: bool,
    ) -> Self {
        Settings {
            cfg,
            status,
            autostart,
            windows_snipping,
            update_note: String::new(),
            recording: None,
            pending: String::new(),
            reject: None,
            scroll: 0.0,
            pointer: None,
            view: SIZE,
            hits: Vec::new(),
            title_hits: Vec::new(),
            content_h: 0.0,
            dirty: true,
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recording.is_some()
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn invalidate(&mut self) {
        self.dirty = true;
    }

    /// Whether the pointer is over something clickable.
    pub fn hovering_control(&self) -> bool {
        self.hover().is_some()
    }

    fn rows(&self) -> Vec<(&'static str, Vec<Row>)> {
        let mut shortcuts: Vec<Row> = Slot::ALL.iter().map(|s| Row::Shortcut(*s)).collect();
        let uses_print = Slot::ALL
            .iter()
            .any(|s| s.get(&self.cfg).eq_ignore_ascii_case("PrintScreen"));
        if self.windows_snipping && uses_print {
            shortcuts.push(Row::WindowsSnipping);
        }
        let mut snipping = vec![
            Row::Choice(Choice::Mode, "Start with"),
            Row::Toggle(Toggle::Loupe),
        ];
        if crate::platform::RENDERS_WINDOWS {
            snipping.push(Row::Toggle(Toggle::Precapture));
        }
        snipping.push(Row::Choice(Choice::ColorFormat, "Colour picker copies"));
        vec![
            ("Shortcuts", shortcuts),
            (
                "After a snip",
                vec![
                    Row::Choice(Choice::AfterCapture, "When an area is selected"),
                    Row::Toggle(Toggle::Copy),
                    Row::Toggle(Toggle::AutoSave),
                    Row::Folder,
                    Row::Choice(Choice::Format, "File format"),
                    Row::Toggle(Toggle::Notifications),
                    Row::Toggle(Toggle::Border),
                    Row::Toggle(Toggle::Frame),
                    Row::Toggle(Toggle::History),
                ],
            ),
            ("Snipping", snipping),
            (
                "Recording",
                vec![
                    Row::Choice(Choice::Audio, "Sound"),
                    Row::Toggle(Toggle::RecordCursor),
                    Row::Toggle(Toggle::RecordClicks),
                    Row::Choice(Choice::Encoder, "Encode with"),
                ],
            ),
            (
                "App",
                vec![
                    Row::Toggle(Toggle::Background),
                    Row::Toggle(Toggle::Autostart),
                    Row::Toggle(Toggle::AutoUpdate),
                    Row::About,
                ],
            ),
        ]
    }

    fn toggle_value(&self, t: Toggle) -> bool {
        let c = &self.cfg;
        match t {
            Toggle::Copy => c.copy_to_clipboard,
            Toggle::AutoSave => c.auto_save,
            Toggle::Notifications => c.notifications,
            Toggle::Border => c.border,
            Toggle::Loupe => c.loupe,
            Toggle::Precapture => c.precapture_windows,
            Toggle::Background => c.run_in_background,
            Toggle::Autostart => self.autostart,
            Toggle::AutoUpdate => c.auto_update,
            Toggle::Frame => c.frame,
            Toggle::History => c.history,
            Toggle::RecordCursor => c.record_cursor,
            Toggle::RecordClicks => c.record_clicks,
        }
    }

    fn max_scroll(&self) -> f32 {
        (self.content_h - (self.view.1 - HEADER)).max(0.0)
    }

    // ---- input -------------------------------------------------------------

    fn hover(&self) -> Option<Hit> {
        let p = self.pointer?;
        let hits = if p.y < HEADER {
            &self.title_hits
        } else {
            &self.hits
        };
        hits.iter()
            .rev()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| *h)
    }

    /// Pointer position in logical window coordinates.
    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        let before = self.hover();
        self.pointer = Some(Pt::new(x, y));
        if self.hover() != before {
            self.dirty = true;
        }
    }

    pub fn pointer_left(&mut self) {
        if self.hover().is_some() {
            self.dirty = true;
        }
        self.pointer = None;
    }

    pub fn scroll(&mut self, lines: f32) {
        let before = self.scroll;
        self.scroll = (self.scroll - lines * 48.0).clamp(0.0, self.max_scroll());
        if self.scroll != before {
            self.dirty = true;
        }
    }

    /// Left button pressed: a press on the title bar (outside its buttons)
    /// starts moving the window.
    pub fn press(&mut self) -> Option<Action> {
        let p = self.pointer?;
        (p.y < HEADER && self.hover().is_none()).then_some(Action::DragWindow)
    }

    /// Left click (on release).
    pub fn click(&mut self) -> Vec<Action> {
        let hit = self.hover();
        let mut out = Vec::new();
        // Clicking anywhere else ends a recording.
        if self.recording.is_some() && !matches!(hit, Some(Hit::Shortcut(_))) {
            self.stop_recording();
            out.push(Action::ResumeHotkeys);
        }
        let Some(hit) = hit else {
            return out;
        };
        self.dirty = true;
        match hit {
            Hit::Shortcut(slot) => {
                if self.recording == Some(slot) {
                    self.stop_recording();
                    out.push(Action::ResumeHotkeys);
                } else {
                    let was = self.recording.is_some();
                    self.recording = Some(slot);
                    self.pending.clear();
                    self.reject = None;
                    if !was {
                        out.push(Action::SuspendHotkeys);
                    }
                }
            }
            Hit::Clear(slot) => {
                slot.set(&mut self.cfg, String::new());
                out.push(Action::Save);
            }
            Hit::Toggle(Toggle::Autostart) => {
                self.autostart = !self.autostart;
                out.push(Action::SetAutostart(self.autostart));
            }
            Hit::Toggle(t) => {
                let c = &mut self.cfg;
                let v = match t {
                    Toggle::Copy => &mut c.copy_to_clipboard,
                    Toggle::AutoSave => &mut c.auto_save,
                    Toggle::Notifications => &mut c.notifications,
                    Toggle::Border => &mut c.border,
                    Toggle::Loupe => &mut c.loupe,
                    Toggle::Precapture => &mut c.precapture_windows,
                    Toggle::Background => &mut c.run_in_background,
                    Toggle::AutoUpdate => &mut c.auto_update,
                    Toggle::Frame => &mut c.frame,
                    Toggle::History => &mut c.history,
                    Toggle::RecordCursor => &mut c.record_cursor,
                    Toggle::RecordClicks => &mut c.record_clicks,
                    Toggle::Autostart => unreachable!(),
                };
                *v = !*v;
                out.push(Action::Save);
            }
            Hit::Choice(choice, i) => {
                let v = choice.options()[i].0;
                if choice.value(&self.cfg) != v {
                    choice.set(&mut self.cfg, v);
                    out.push(Action::Save);
                }
            }
            Hit::ChangeFolder => out.push(Action::PickFolder),
            Hit::OpenConfig => out.push(Action::OpenConfigFile),
            Hit::CheckUpdates => {
                self.update_note = "Checking...".into();
                out.push(Action::CheckForUpdates);
            }
            Hit::DisableWinSnip => out.push(Action::DisableWindowsSnipping),
            Hit::Minimize => out.push(Action::Minimize),
            Hit::Close => out.push(Action::Close),
        }
        out
    }

    fn stop_recording(&mut self) {
        self.recording = None;
        self.pending.clear();
        self.reject = None;
        self.dirty = true;
    }

    /// A key went down (or, for Print Screen, up: Windows only reports the
    /// release of that key to applications).
    pub fn key(&mut self, code: KeyCode, mods: Mods) -> Vec<Action> {
        let Some(slot) = self.recording else {
            return Vec::new();
        };
        self.dirty = true;
        if !mods.any() {
            match code {
                KeyCode::Escape => {
                    self.stop_recording();
                    return vec![Action::ResumeHotkeys];
                }
                KeyCode::Backspace | KeyCode::Delete => {
                    self.stop_recording();
                    slot.set(&mut self.cfg, String::new());
                    return vec![Action::Save];
                }
                _ => {}
            }
        }
        match hotkey::record(code, mods) {
            Recorded::Pending(p) => {
                self.pending = p;
                self.reject = None;
                Vec::new()
            }
            Recorded::Rejected(why) => {
                self.pending.clear();
                self.reject = Some(why);
                Vec::new()
            }
            Recorded::Done(spec) => {
                self.stop_recording();
                slot.set(&mut self.cfg, spec);
                vec![Action::Save]
            }
        }
    }

    /// Modifier state changed while recording (shows "Ctrl + Shift + ...").
    pub fn modifiers(&mut self, mods: Mods) {
        if self.recording.is_some() {
            let p = match hotkey::record(KeyCode::ShiftLeft, mods) {
                Recorded::Pending(p) => p,
                _ => String::new(),
            };
            if p != self.pending {
                self.pending = p;
                self.dirty = true;
            }
        }
    }

    // ---- drawing -----------------------------------------------------------

    pub fn render(&mut self, buf: &mut [u32], w: u32, h: u32, scale: f32) {
        if w == 0 || h == 0 || buf.len() != (w * h) as usize {
            return;
        }
        self.view = (w as f32 / scale, h as f32 / scale);
        self.scroll = self.scroll.min(self.max_scroll());
        buf.fill(theme::PANEL_SOLID.to_u32());
        let mut c = Canvas::new(buf, w, h);
        let hover = self.hover();
        let mut hits = Vec::new();
        let mut p = Painter {
            c: &mut c,
            s: scale,
            hover,
        };

        let x = PAD;
        let cw = self.view.0 - 2.0 * PAD;
        let mut y = HEADER + 6.0 - self.scroll;
        let top = y + self.scroll;
        for (title, rows) in self.rows() {
            p.text(title, x + 4.0, y + 12.0, 12.0, theme::TEXT_DIM);
            y += 22.0;
            let card_h: f32 = rows.iter().map(Row::height).sum();
            let card = RectF::new(x, y, cw, card_h);
            p.c.fill_rrect(p.r(card), CARD_RADIUS * scale, CARD);

            let mut ry = y;
            for (i, row) in rows.iter().enumerate() {
                let rh = row.height();
                if i > 0 {
                    p.c.fill_rect(
                        p.r(RectF::new(x + 16.0, ry, cw - 32.0, 1.0 / scale)),
                        theme::SEPARATOR,
                    );
                }
                self.draw_row(&mut p, row, RectF::new(x, ry, cw, rh), &mut hits);
                ry += rh;
            }
            y += card_h + SECTION_GAP;
        }
        self.content_h = y + self.scroll - top + 4.0;
        self.hits = hits;

        // Fixed header on top of the scrolled content.
        let (vw, _) = self.view;
        p.c.fill_rect(p.r(RectF::new(0.0, 0.0, vw, HEADER)), theme::PANEL_SOLID);
        if self.scroll > 0.5 {
            p.c.fill_rect(
                p.r(RectF::new(0.0, HEADER - 1.0, vw, 1.0)),
                theme::SEPARATOR,
            );
        }
        // Custom title bar: the whole strip drags the window; minimise and
        // close sit on the right.
        crate::brand::draw_mark(p.c, p.r(RectF::new(14.0, 10.0, 22.0, 22.0)), true);
        p.text("Krab Settings", 44.0, 26.0, 13.0, theme::TEXT);
        let close = RectF::new(vw - 8.0 - 32.0, 7.0, 32.0, 28.0);
        let min = RectF::new(close.x - 2.0 - 32.0, 7.0, 32.0, 28.0);
        for (r, hit) in [(min, Hit::Minimize), (close, Hit::Close)] {
            if p.hover == Some(hit) {
                let col = if hit == Hit::Close {
                    theme::DANGER
                } else {
                    CONTROL_HOVER
                };
                p.c.fill_rrect(p.r(r), 7.0 * scale, col);
            }
        }
        let c = min.center();
        p.c.line(
            p.pt(Pt::new(c.x - 5.0, c.y)),
            p.pt(Pt::new(c.x + 5.0, c.y)),
            1.4 * scale,
            theme::TEXT,
        );
        icons::draw(
            p.c,
            Icon::Close,
            p.pt(close.center()),
            14.0 * scale,
            theme::TEXT,
        );
        self.title_hits = vec![(min, Hit::Minimize), (close, Hit::Close)];

        // Scroll indicator.
        let max = self.max_scroll();
        if max > 0.0 {
            let track = self.view.1 - HEADER - 12.0;
            let thumb = (track * track / (track + max)).max(30.0);
            let ty = HEADER + 6.0 + (track - thumb) * (self.scroll / max);
            p.c.fill_rrect(
                p.r(RectF::new(vw - 7.0, ty, 4.0, thumb)),
                2.0 * scale,
                Color::rgba(255, 255, 255, 40),
            );
        }
    }

    fn draw_row(&self, p: &mut Painter, row: &Row, r: RectF, hits: &mut Vec<(RectF, Hit)>) {
        let lx = r.x + 16.0;
        let right = r.right() - 16.0;
        let cy = r.y + r.h / 2.0;
        match row {
            Row::Shortcut(slot) => {
                let i = Slot::ALL.iter().position(|s| s == slot).unwrap_or(0);
                p.text(slot.label(), lx, cy - 3.0, 13.5, theme::TEXT);
                let recording = self.recording == Some(*slot);
                let (msg, col) = if recording {
                    match self.reject {
                        Some(why) => (why.to_string(), theme::DANGER),
                        None => ("Esc cancels, Backspace clears".into(), theme::TEXT_DIM),
                    }
                } else {
                    let st = &self.status[i];
                    let col = if st.is_problem() {
                        theme::DANGER
                    } else if *st == Status::Active {
                        OK
                    } else {
                        theme::TEXT_DIM
                    };
                    (st.message().to_string(), col)
                };

                let spec = slot.get(&self.cfg);
                let chip_w = 178.0;
                let clear = RectF::new(right - 28.0, cy - 14.0, 28.0, 28.0);
                let chip = RectF::new(clear.x - 6.0 - chip_w, cy - 16.0, chip_w, 32.0);
                let fit = ellipsize(&msg, 11.5 * p.s, (chip.x - lx - 12.0) * p.s);
                p.text(&fit, lx, cy + 15.0, 11.5, col);
                let hit = Hit::Shortcut(*slot);
                // Tinted fills instead of outlines: orange while listening,
                // red when the shortcut does not work.
                if recording {
                    p.c.fill_rrect(p.r(chip), 8.0 * p.s, theme::ACCENT.alpha(0.22));
                } else if self.status[i].is_problem() {
                    p.c.fill_rrect(p.r(chip), 8.0 * p.s, theme::DANGER.alpha(0.16));
                } else {
                    p.control(chip, 8.0, hit);
                }
                let (label, col) = if recording {
                    if self.pending.is_empty() {
                        ("Press a shortcut".to_string(), theme::ACCENT_HOVER)
                    } else {
                        (
                            format!(
                                "{} ...",
                                hotkey::display(self.pending.trim_end_matches('+'))
                            ),
                            theme::TEXT,
                        )
                    }
                } else if spec.trim().is_empty() {
                    ("Click to set".to_string(), theme::TEXT_DIM)
                } else {
                    (hotkey::display(spec), theme::TEXT)
                };
                p.centered(&label, chip, 13.0, col);
                hits.push((chip, hit));
                if !spec.trim().is_empty() && !recording {
                    let hit = Hit::Clear(*slot);
                    if p.hover == Some(hit) {
                        p.c.fill_circle(p.pt(clear.center()), 14.0 * p.s, CONTROL_HOVER);
                    }
                    icons::draw(
                        p.c,
                        Icon::Close,
                        p.pt(clear.center()),
                        15.0 * p.s,
                        theme::TEXT_DIM,
                    );
                    hits.push((clear, hit));
                }
            }
            Row::WindowsSnipping => {
                p.text(
                    "Windows may open its own Snipping Tool",
                    lx,
                    cy - 3.0,
                    13.5,
                    theme::TEXT,
                );
                p.text(
                    "when Print Screen is pressed",
                    lx,
                    cy + 15.0,
                    11.5,
                    theme::TEXT_DIM,
                );
                let b = p.button("Turn that off", right, cy, Hit::DisableWinSnip);
                hits.push(b);
            }
            Row::Choice(choice, label) => {
                p.text(label, lx, cy + 5.0, 13.5, theme::TEXT);
                let face = text::ui();
                let opts = choice.options();
                let em = 12.5;
                let widths: Vec<f32> = opts
                    .iter()
                    .map(|(_, l)| face.measure(l, em * p.s) / p.s + 24.0)
                    .collect();
                let total: f32 = widths.iter().sum::<f32>() + 6.0;
                let seg = RectF::new(right - total, cy - 16.0, total, 32.0);
                p.c.fill_rrect(p.r(seg), 9.0 * p.s, CONTROL);
                let current = choice.value(&self.cfg);
                let mut sx = seg.x + 3.0;
                for ((value, l), w) in opts.iter().zip(&widths) {
                    let cell = RectF::new(sx, seg.y + 3.0, *w, 26.0);
                    let idx = opts.iter().position(|o| o.0 == *value).unwrap_or(0);
                    let hit = Hit::Choice(*choice, idx);
                    let on = current == *value;
                    if on {
                        p.c.fill_rrect(p.r(cell), 7.0 * p.s, theme::ACCENT);
                    } else if p.hover == Some(hit) {
                        p.c.fill_rrect(p.r(cell), 7.0 * p.s, CONTROL);
                    }
                    p.centered(l, cell, em, if on { Color::WHITE } else { theme::TEXT });
                    hits.push((cell, hit));
                    sx += w;
                }
            }
            Row::Toggle(t) => {
                p.text(t.label(), lx, cy + 5.0, 13.5, theme::TEXT);
                let on = self.toggle_value(*t);
                let track = RectF::new(right - 40.0, cy - 12.0, 40.0, 24.0);
                let col = if on {
                    theme::ACCENT
                } else {
                    Color::rgba(255, 255, 255, 46)
                };
                p.c.fill_rrect(p.r(track), 12.0 * p.s, col);
                let kx = if on {
                    track.right() - 12.0
                } else {
                    track.x + 12.0
                };
                p.c.fill_circle(p.pt(Pt::new(kx, cy)), 9.0 * p.s, Color::WHITE);
                // The whole row is clickable.
                hits.push((r, Hit::Toggle(*t)));
                if p.hover == Some(Hit::Toggle(*t)) {
                    p.c.fill_rect(p.r(r.inflate(-1.0)), Color::rgba(255, 255, 255, 5));
                }
            }
            Row::Folder => {
                p.text("Save to", lx, cy - 3.0, 13.5, theme::TEXT);
                let b = p.button("Change...", right, cy, Hit::ChangeFolder);
                let path = self.cfg.save_dir().display().to_string();
                let max = b.0.x - lx - 16.0;
                let fit = ellipsize(&path, 11.5 * p.s, max * p.s);
                p.text(&fit, lx, cy + 15.0, 11.5, theme::TEXT_DIM);
                hits.push(b);
            }
            Row::About => {
                let b2 = p.button("Check for updates", right, cy, Hit::CheckUpdates);
                let b1 = p.button("Open config file", b2.0.x - 8.0, cy, Hit::OpenConfig);
                let note = if self.update_note.is_empty() {
                    format!("Version {}", env!("CARGO_PKG_VERSION"))
                } else {
                    self.update_note.clone()
                };
                let max = b1.0.x - lx - 12.0;
                p.text(
                    &ellipsize(&note, 12.5 * p.s, max * p.s),
                    lx,
                    cy + 5.0,
                    12.5,
                    theme::TEXT_DIM,
                );
                hits.push(b1);
                hits.push(b2);
            }
        }
    }
}

/// Logical-coordinate drawing helpers.
struct Painter<'a, 'b> {
    c: &'a mut Canvas<'b>,
    s: f32,
    hover: Option<Hit>,
}

impl Painter<'_, '_> {
    fn r(&self, r: RectF) -> RectF {
        RectF::new(r.x * self.s, r.y * self.s, r.w * self.s, r.h * self.s)
    }

    fn pt(&self, p: Pt) -> Pt {
        Pt::new(p.x * self.s, p.y * self.s)
    }

    fn text(&mut self, t: &str, x: f32, baseline: f32, em: f32, col: Color) {
        text::ui().draw(self.c, t, x * self.s, baseline * self.s, em * self.s, col);
    }

    fn centered(&mut self, t: &str, r: RectF, em: f32, col: Color) {
        let c = r.center();
        text::ui().draw_centered(self.c, t, c.x * self.s, c.y * self.s, em * self.s, col);
    }

    fn control(&mut self, r: RectF, radius: f32, hit: Hit) {
        let col = if self.hover == Some(hit) {
            CONTROL_HOVER
        } else {
            CONTROL
        };
        self.c.fill_rrect(self.r(r), radius * self.s, col);
    }

    /// A button whose right edge is at `right`, vertically centred on `cy`.
    fn button(&mut self, label: &str, right: f32, cy: f32, hit: Hit) -> (RectF, Hit) {
        let w = text::ui().measure(label, 12.5 * self.s) / self.s + 28.0;
        let r = RectF::new(right - w, cy - 16.0, w, 32.0);
        self.control(r, 8.0, hit);
        self.centered(label, r, 12.5, theme::TEXT);
        (r, hit)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> Settings {
        let cfg = Config {
            hotkey: "PrintScreen".into(),
            hotkey_alt: "Alt+Shift+S".into(),
            hotkey_record: String::new(),
            ..Config::default()
        };
        Settings::new(
            cfg,
            [Status::Active, Status::Taken, Status::Off, Status::Off],
            false,
            false,
        )
    }

    fn frame(s: &mut Settings) {
        let (w, h) = (SIZE.0 as u32, SIZE.1 as u32);
        let mut buf = vec![0u32; (w * h) as usize];
        s.render(&mut buf, w, h, 1.0);
    }

    fn click_on(s: &mut Settings, hit: Hit) -> Vec<Action> {
        frame(s);
        let r = s
            .hits
            .iter()
            .find(|(_, h)| *h == hit)
            .map(|(r, _)| *r)
            .unwrap_or_else(|| panic!("{hit:?} not on screen"));
        s.pointer_moved(r.center().x, r.center().y);
        s.click()
    }

    #[test]
    fn a_shortcut_is_recorded_from_key_presses() {
        let mut s = settings();
        assert_eq!(
            click_on(&mut s, Hit::Shortcut(Slot::Record)),
            vec![Action::SuspendHotkeys]
        );
        assert!(s.is_recording());
        let mods = Mods {
            alt: true,
            shift: true,
            ..Mods::default()
        };
        assert!(s.key(KeyCode::ShiftLeft, mods).is_empty());
        assert_eq!(s.key(KeyCode::KeyR, mods), vec![Action::Save]);
        assert_eq!(s.cfg.hotkey_record, "Alt+Shift+R");
        assert!(!s.is_recording());
    }

    #[test]
    fn escape_cancels_and_backspace_clears() {
        let mut s = settings();
        click_on(&mut s, Hit::Shortcut(Slot::Snip));
        assert_eq!(
            s.key(KeyCode::Escape, Mods::default()),
            vec![Action::ResumeHotkeys]
        );
        assert_eq!(s.cfg.hotkey, "PrintScreen");
        click_on(&mut s, Hit::Shortcut(Slot::Snip));
        assert_eq!(
            s.key(KeyCode::Backspace, Mods::default()),
            vec![Action::Save]
        );
        assert_eq!(s.cfg.hotkey, "");
    }

    #[test]
    fn a_taken_shortcut_can_be_cleared() {
        let mut s = settings();
        assert_eq!(
            click_on(&mut s, Hit::Clear(Slot::SnipAlt)),
            vec![Action::Save]
        );
        assert_eq!(s.cfg.hotkey_alt, "");
    }

    #[test]
    fn plain_letters_are_rejected_without_leaving_recording() {
        let mut s = settings();
        click_on(&mut s, Hit::Shortcut(Slot::Record));
        assert!(s.key(KeyCode::KeyR, Mods::default()).is_empty());
        assert!(s.is_recording());
        assert!(s.reject.is_some());
    }

    #[test]
    fn toggles_and_choices_change_the_config() {
        let mut s = settings();
        let before = s.cfg.loupe;
        assert_eq!(
            click_on(&mut s, Hit::Toggle(Toggle::Loupe)),
            vec![Action::Save]
        );
        assert_eq!(s.cfg.loupe, !before);
        assert_eq!(
            click_on(&mut s, Hit::Choice(Choice::Format, 1)),
            vec![Action::Save]
        );
        assert_eq!(s.cfg.file_format, "jpg");
        assert_eq!(
            click_on(&mut s, Hit::Toggle(Toggle::Autostart)),
            vec![Action::SetAutostart(true)]
        );
    }

    #[test]
    fn capturing_covered_windows_in_advance_can_be_switched_off() {
        let mut s = settings();
        frame(&mut s);
        let shown = s
            .hits
            .iter()
            .any(|(_, h)| *h == Hit::Toggle(Toggle::Precapture));
        assert_eq!(shown, crate::platform::RENDERS_WINDOWS);
        if shown {
            assert!(s.cfg.precapture_windows);
            assert_eq!(
                click_on(&mut s, Hit::Toggle(Toggle::Precapture)),
                vec![Action::Save]
            );
            assert!(!s.cfg.precapture_windows);
        }
    }

    #[test]
    fn the_whole_page_can_be_scrolled_into_view() {
        let mut s = settings();
        frame(&mut s);
        assert!(s.max_scroll() > 0.0, "content is taller than the window");
        s.scroll(-100.0);
        frame(&mut s);
        // The last row's buttons are reachable after scrolling down.
        assert!(s
            .hits
            .iter()
            .any(|(r, h)| *h == Hit::CheckUpdates && r.bottom() <= SIZE.1));
    }

    #[test]
    fn windows_snipping_hint_only_with_print_screen() {
        let mut s = settings();
        s.windows_snipping = true;
        assert_eq!(
            click_on(&mut s, Hit::DisableWinSnip),
            vec![Action::DisableWindowsSnipping]
        );
        s.cfg.hotkey = "Alt+Shift+A".into();
        frame(&mut s);
        assert!(!s.hits.iter().any(|(_, h)| *h == Hit::DisableWinSnip));
    }
}
