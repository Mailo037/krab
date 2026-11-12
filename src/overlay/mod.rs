//! The capture overlay: a frozen screenshot with a selection and markup
//! tools on top.
//!
//! This module is deliberately window-system agnostic. It consumes abstract
//! input events and renders into any `u32` pixel buffer, which makes the whole
//! interaction model unit-testable and lets the README images be produced by
//! the real renderer.

mod layout;
mod render;

use crate::annotate::{text_size, Annotation, Shape, Source, Tool};
use crate::capture::{Shot, WinInfo};
use crate::config::Config;
use crate::geom::{Pt, RectF, RectI};
use crate::gfx::{theme, Canvas, Color};
use crate::output::Image;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Rect,
    Freeform,
    Window,
    Fullscreen,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Mode> {
        match s.to_ascii_lowercase().as_str() {
            "rect" | "rectangle" | "region" | "area" => Some(Mode::Rect),
            "freeform" | "free" | "lasso" => Some(Mode::Freeform),
            "window" | "win" => Some(Mode::Window),
            "full" | "fullscreen" | "screen" | "monitor" => Some(Mode::Fullscreen),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Rect => "rect",
            Mode::Freeform => "freeform",
            Mode::Window => "window",
            Mode::Fullscreen => "full",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Snip,
    Record,
}

/// State mirrored across the overlays of all monitors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shared {
    pub mode: Mode,
    pub kind: Kind,
    /// Seconds used by the delay button.
    pub delay: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Btn {
    Kind(Kind),
    Mode(Mode),
    Delay,
    Cancel,
    Tool(Tool),
    Undo,
    Redo,
    Ocr,
    Open,
    Save,
    Copy,
    Color(u8),
    Width(u8),
    Record,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Escape,
    Enter,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Char(char),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    /// Ctrl, or Cmd on macOS.
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CursorKind {
    Default,
    Crosshair,
    Pointer,
    Move,
    Text,
    ResizeNs,
    ResizeEw,
    ResizeNesw,
    ResizeNwse,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    Copy,
    SaveAs,
    OpenExternal,
    ExtractText,
}

pub enum Outcome {
    Cancel,
    Finish(Finish, Image),
    /// Close, wait this many seconds, then start over with a fresh capture.
    Delay(u32),
    /// Start recording this area (pixels, relative to the monitor).
    Record(RectI),
    /// Put text on the clipboard; the overlay stays open.
    CopyText(String),
}

/// Per-session settings derived from the user's config.
#[derive(Clone, Debug)]
pub struct Options {
    pub quick_copy: bool,
    pub loupe: bool,
    pub border: Option<(Color, u32)>,
    pub color: Color,
}

impl Options {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            quick_copy: cfg.quick_copy(),
            loupe: cfg.loupe,
            border: cfg.border.then(|| (cfg.border_color(), cfg.border_width)),
            color: cfg.annotation_color(),
        }
    }
}

impl Default for Options {
    fn default() -> Self {
        Self {
            quick_copy: false,
            loupe: true,
            border: None,
            color: theme::PALETTE[0],
        }
    }
}

struct Mask {
    poly: Vec<Pt>,
    rect: RectI,
    /// Coverage, `rect.w * rect.h` bytes.
    a: Vec<u8>,
}

struct Selection {
    rect: RectF,
    mask: Option<Mask>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Handle {
    N,
    S,
    E,
    W,
    Ne,
    Nw,
    Se,
    Sw,
}

enum Drag {
    None,
    NewRect { start: Pt },
    Lasso { pts: Vec<Pt> },
    Move { grab: Pt, orig: RectF },
    Resize { handle: Handle, orig: RectF },
    Draw { start: Pt },
    Erase { checkpointed: bool },
    Button(Btn),
}

struct TextEdit {
    pos: Pt,
    text: String,
}

pub struct Overlay {
    px: Vec<u32>,
    w: u32,
    h: u32,
    scale: f32,
    opts: Options,
    /// Window rects in local pixels, top-most first.
    windows: Vec<RectI>,
    ui: Shared,

    sel: Option<Selection>,
    tool: Tool,
    color: Color,
    width_idx: usize,
    annotations: Vec<Annotation>,
    undo: Vec<Vec<Annotation>>,
    redo: Vec<Vec<Annotation>>,
    draft: Option<Annotation>,
    text_edit: Option<TextEdit>,

    drag: Drag,
    cursor: Pt,
    cursor_inside: bool,
    mods: Mods,
    hover_btn: Option<Btn>,
    hover_window: Option<RectI>,
    last_click: Option<(Instant, Pt)>,

    outcome: Option<Outcome>,
    flash: Option<(String, Instant)>,
    dirty: bool,
    started_selection: bool,
}

const FLASH_TIME: Duration = Duration::from_millis(1500);
const DOUBLE_CLICK: Duration = Duration::from_millis(400);

impl Overlay {
    pub fn new(shot: Shot, windows: &[WinInfo], opts: Options, shared: Shared) -> Self {
        let bounds = RectI::new(0, 0, shot.w as i32, shot.h as i32);
        let local: Vec<RectI> = windows
            .iter()
            .map(|w| shot.native_to_local(w.rect).intersect(&bounds))
            .filter(|r| r.w > 8 && r.h > 8)
            .collect();
        let color = opts.color;
        Self {
            w: shot.w,
            h: shot.h,
            scale: shot.scale.max(0.5),
            px: shot.px,
            opts,
            windows: local,
            ui: shared,
            sel: None,
            tool: Tool::Select,
            color,
            width_idx: 1,
            annotations: Vec::new(),
            undo: Vec::new(),
            redo: Vec::new(),
            draft: None,
            text_edit: None,
            drag: Drag::None,
            cursor: Pt::new(-1.0, -1.0),
            cursor_inside: false,
            mods: Mods::default(),
            hover_btn: None,
            hover_window: None,
            last_click: None,
            outcome: None,
            flash: None,
            dirty: true,
            started_selection: false,
        }
    }

    // ---- accessors used by the window layer -------------------------------

    pub fn size(&self) -> (u32, u32) {
        (self.w, self.h)
    }

    pub fn shared(&self) -> Shared {
        self.ui
    }

    pub fn set_shared(&mut self, s: Shared) {
        if self.ui != s {
            self.ui = s;
            self.dirty = true;
        }
    }

    pub fn has_selection(&self) -> bool {
        self.sel.is_some() || !matches!(self.drag, Drag::None | Drag::Button(_))
    }

    /// True once after this overlay began (or received) a selection, so the
    /// window layer can reset the overlays on other monitors.
    pub fn take_started_selection(&mut self) -> bool {
        std::mem::take(&mut self.started_selection)
    }

    pub fn take_outcome(&mut self) -> Option<Outcome> {
        self.outcome.take()
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }

    pub fn is_typing(&self) -> bool {
        self.text_edit.is_some()
    }

    pub fn tool(&self) -> Tool {
        self.tool
    }

    pub fn annotation_count(&self) -> usize {
        self.annotations.len()
    }

    pub fn selection(&self) -> Option<RectI> {
        self.sel.as_ref().map(|s| s.rect.round())
    }

    /// When the overlay needs a redraw without input (transient messages).
    pub fn next_wakeup(&self) -> Option<Instant> {
        self.flash.as_ref().map(|(_, t)| *t + FLASH_TIME)
    }

    pub fn tick(&mut self) {
        if let Some((_, t)) = &self.flash {
            if t.elapsed() >= FLASH_TIME {
                self.flash = None;
                self.dirty = true;
            }
        }
    }

    fn bounds(&self) -> RectF {
        RectF::new(0.0, 0.0, self.w as f32, self.h as f32)
    }

    fn stroke_width(&self) -> f32 {
        theme::WIDTHS[self.width_idx] * self.scale
    }

    fn text_em(&self) -> f32 {
        (11.0 + theme::WIDTHS[self.width_idx] * 2.6) * self.scale
    }

    // ---- selection ---------------------------------------------------------

    /// Drop the selection and everything drawn on it.
    pub fn clear_selection(&mut self) {
        self.sel = None;
        self.annotations.clear();
        self.undo.clear();
        self.redo.clear();
        self.draft = None;
        self.text_edit = None;
        self.drag = Drag::None;
        self.tool = Tool::Select;
        self.dirty = true;
    }

    /// Select the whole monitor.
    pub fn select_all(&mut self) {
        self.set_selection(self.bounds(), None);
    }

    fn set_selection(&mut self, rect: RectF, mask: Option<Mask>) {
        self.annotations.clear();
        self.undo.clear();
        self.redo.clear();
        self.tool = Tool::Select;
        self.sel = Some(Selection {
            rect: rect.intersect(&self.bounds()).round().to_f(),
            mask,
        });
        self.started_selection = true;
        self.dirty = true;
        if self.ui.kind == Kind::Snip && self.opts.quick_copy {
            self.finish(Finish::Copy);
        }
    }

    fn finish_lasso(&mut self, pts: Vec<Pt>) {
        if pts.len() < 3 {
            return;
        }
        let mut bb = RectF::new(pts[0].x, pts[0].y, 0.0, 0.0);
        for p in &pts {
            bb = bb.union(&RectF::new(p.x, p.y, 0.0, 0.0));
        }
        let rect = bb.intersect(&self.bounds()).round();
        if rect.w < 5 || rect.h < 5 {
            return;
        }
        // Rasterize the lasso once; coverage doubles as the output alpha.
        let mut tmp = vec![0u32; (rect.w * rect.h) as usize];
        let mut pb = tiny_skia::PathBuilder::new();
        pb.move_to(pts[0].x, pts[0].y);
        for p in &pts[1..] {
            pb.line_to(p.x, p.y);
        }
        pb.close();
        if let Some(path) = pb.finish() {
            Canvas::new(&mut tmp, rect.w as u32, rect.h as u32)
                .with_offset(-rect.x as f32, -rect.y as f32)
                .fill_path(&path, Color::WHITE);
        }
        let a: Vec<u8> = tmp.iter().map(|p| (p >> 24) as u8).collect();
        if a.iter().all(|v| *v == 0) {
            return;
        }
        self.set_selection(rect.to_f(), Some(Mask { poly: pts, rect, a }));
    }

    fn handle_points(r: RectF) -> [(Handle, Pt); 8] {
        let (x0, y0, x1, y1) = (r.x, r.y, r.right(), r.bottom());
        let (cx, cy) = (r.x + r.w / 2.0, r.y + r.h / 2.0);
        [
            (Handle::Nw, Pt::new(x0, y0)),
            (Handle::N, Pt::new(cx, y0)),
            (Handle::Ne, Pt::new(x1, y0)),
            (Handle::E, Pt::new(x1, cy)),
            (Handle::Se, Pt::new(x1, y1)),
            (Handle::S, Pt::new(cx, y1)),
            (Handle::Sw, Pt::new(x0, y1)),
            (Handle::W, Pt::new(x0, cy)),
        ]
    }

    /// Whether the selection can currently be moved / resized.
    fn adjustable(&self) -> bool {
        match &self.sel {
            Some(s) => {
                s.mask.is_none() && (self.ui.kind == Kind::Record || self.tool == Tool::Select)
            }
            None => false,
        }
    }

    fn handle_at(&self, p: Pt) -> Option<Handle> {
        let sel = self.sel.as_ref()?;
        if !self.adjustable() {
            return None;
        }
        let reach = 9.0 * self.scale;
        let r = sel.rect;
        if let Some((h, _)) = Self::handle_points(r)
            .into_iter()
            .find(|(_, hp)| hp.dist(p) <= reach)
        {
            return Some(h);
        }
        // Edges between the handles.
        let edge = 5.0 * self.scale;
        let in_x = p.x >= r.x && p.x <= r.right();
        let in_y = p.y >= r.y && p.y <= r.bottom();
        if in_x && (p.y - r.y).abs() <= edge {
            Some(Handle::N)
        } else if in_x && (p.y - r.bottom()).abs() <= edge {
            Some(Handle::S)
        } else if in_y && (p.x - r.x).abs() <= edge {
            Some(Handle::W)
        } else if in_y && (p.x - r.right()).abs() <= edge {
            Some(Handle::E)
        } else {
            None
        }
    }

    fn resized(&self, handle: Handle, orig: RectF, p: Pt) -> RectF {
        let b = self.bounds();
        let p = Pt::new(p.x.clamp(b.x, b.right()), p.y.clamp(b.y, b.bottom()));
        let (mut x0, mut y0, mut x1, mut y1) = (orig.x, orig.y, orig.right(), orig.bottom());
        match handle {
            Handle::N | Handle::Ne | Handle::Nw => y0 = p.y,
            Handle::S | Handle::Se | Handle::Sw => y1 = p.y,
            _ => {}
        }
        match handle {
            Handle::W | Handle::Nw | Handle::Sw => x0 = p.x,
            Handle::E | Handle::Ne | Handle::Se => x1 = p.x,
            _ => {}
        }
        RectF::from_points(Pt::new(x0, y0), Pt::new(x1, y1))
            .round()
            .to_f()
    }

    fn window_at(&self, p: Pt) -> Option<RectI> {
        self.windows
            .iter()
            .copied()
            .find(|r| r.contains(p.x as i32, p.y as i32))
    }

    // ---- history -----------------------------------------------------------

    fn checkpoint(&mut self) {
        self.undo.push(self.annotations.clone());
        self.redo.clear();
    }

    pub fn undo(&mut self) {
        self.commit_text();
        if let Some(prev) = self.undo.pop() {
            self.redo
                .push(std::mem::replace(&mut self.annotations, prev));
            self.dirty = true;
        }
    }

    pub fn redo(&mut self) {
        if let Some(next) = self.redo.pop() {
            self.undo
                .push(std::mem::replace(&mut self.annotations, next));
            self.dirty = true;
        }
    }

    fn commit_text(&mut self) {
        if let Some(edit) = self.text_edit.take() {
            if !edit.text.trim().is_empty() {
                self.checkpoint();
                self.annotations.push(Annotation {
                    shape: Shape::Text {
                        pos: edit.pos,
                        text: edit.text,
                        size: self.text_em(),
                    },
                    color: self.color,
                    width: self.stroke_width(),
                });
            }
            self.dirty = true;
        }
    }

    // ---- output ------------------------------------------------------------

    fn finish(&mut self, kind: Finish) {
        self.commit_text();
        if let Some(img) = self.compose() {
            self.outcome = Some(Outcome::Finish(kind, img));
        }
    }

    /// Render the selection with all annotations to a standalone image.
    pub fn compose(&self) -> Option<Image> {
        let sel = self.sel.as_ref()?;
        let r = sel
            .rect
            .round()
            .intersect(&RectI::new(0, 0, self.w as i32, self.h as i32));
        if r.is_empty() {
            return None;
        }
        let (w, h) = (r.w as usize, r.h as usize);
        let mut out = Vec::with_capacity(w * h);
        for y in r.y..r.bottom() {
            let row = y as usize * self.w as usize;
            out.extend_from_slice(&self.px[row + r.x as usize..row + r.right() as usize]);
        }
        {
            let src = Source {
                px: &self.px,
                w: self.w,
                h: self.h,
            };
            let mut c =
                Canvas::new(&mut out, w as u32, h as u32).with_offset(-r.x as f32, -r.y as f32);
            for a in &self.annotations {
                a.draw(&mut c, &src, self.scale);
            }
            if let (Some((color, bw)), None) = (self.opts.border, &sel.mask) {
                let bw = bw as f32;
                let inner =
                    RectF::new(r.x as f32, r.y as f32, r.w as f32, r.h as f32).inflate(-bw / 2.0);
                c.stroke_rrect(inner, 0.0, bw, color);
            }
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for (i, p) in out.iter().enumerate() {
            let a = match &sel.mask {
                Some(m) if m.rect == r => m.a[i],
                _ => 255,
            };
            rgba.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, a]);
        }
        Some(Image {
            w: w as u32,
            h: h as u32,
            rgba,
        })
    }

    // ---- input: pointer ----------------------------------------------------

    pub fn set_mods(&mut self, mods: Mods) {
        self.mods = mods;
    }

    pub fn pointer_left(&mut self) {
        if self.cursor_inside {
            self.cursor_inside = false;
            self.hover_btn = None;
            self.hover_window = None;
            self.dirty = true;
        }
    }

    pub fn pointer_moved(&mut self, x: f32, y: f32) {
        let p = Pt::new(x, y);
        self.cursor = p;
        self.cursor_inside = true;
        self.dirty = true;

        let b = self.bounds();
        let clamped = Pt::new(p.x.clamp(b.x, b.right()), p.y.clamp(b.y, b.bottom()));
        match self.drag {
            Drag::None => {}
            Drag::Button(_) => {
                self.hover_btn = self.bars().hit(p);
                return;
            }
            Drag::NewRect { .. } => return,
            Drag::Lasso { ref mut pts } => {
                if pts.last().is_none_or(|l| l.dist(clamped) >= 1.5) {
                    pts.push(clamped);
                }
                return;
            }
            Drag::Move { grab, orig } => {
                let (dx, dy) = (p.x - grab.x, p.y - grab.y);
                let nx = (orig.x + dx).clamp(0.0, (b.w - orig.w).max(0.0));
                let ny = (orig.y + dy).clamp(0.0, (b.h - orig.h).max(0.0));
                let rect = RectF::new(nx.round(), ny.round(), orig.w, orig.h);
                if let Some(sel) = &mut self.sel {
                    sel.rect = rect;
                }
                return;
            }
            Drag::Resize { handle, orig } => {
                let rect = self.resized(handle, orig, p);
                if let Some(sel) = &mut self.sel {
                    sel.rect = rect;
                }
                return;
            }
            Drag::Draw { start } => {
                self.update_draft(start, p);
                return;
            }
            Drag::Erase { .. } => {
                self.erase_at(p);
                return;
            }
        }

        let bars = self.bars();
        self.hover_btn = bars.hit(p);
        self.hover_window = None;
        if self.sel.is_none() && self.ui.mode == Mode::Window && !bars.contains(p) {
            self.hover_window = self.window_at(p);
        }
    }

    pub fn pointer_button(&mut self, button: Button, pressed: bool) {
        self.dirty = true;
        match (button, pressed) {
            (Button::Left, true) => self.press(),
            (Button::Left, false) => self.release(),
            (Button::Right, true) => self.step_back(),
            (Button::Right, false) => {}
        }
    }

    fn press(&mut self) {
        let p = self.cursor;
        let bars = self.bars();
        if let Some(btn) = bars.hit(p) {
            if self.btn_enabled(btn) {
                self.drag = Drag::Button(btn);
            }
            return;
        }
        if bars.contains(p) {
            return;
        }
        self.commit_text();
        if self.sel.is_none() {
            self.press_selecting(p);
        } else {
            self.press_editing(p);
        }
    }

    fn press_selecting(&mut self, p: Pt) {
        match self.ui.mode {
            Mode::Rect => {
                self.drag = Drag::NewRect { start: p };
                self.started_selection = true;
            }
            Mode::Freeform => {
                self.drag = Drag::Lasso { pts: vec![p] };
                self.started_selection = true;
            }
            Mode::Window => {
                let rect = self
                    .window_at(p)
                    .map(|r| r.to_f())
                    .unwrap_or_else(|| self.bounds());
                self.set_selection(rect, None);
            }
            Mode::Fullscreen => self.select_all(),
        }
    }

    fn is_double_click(&mut self, p: Pt) -> bool {
        let now = Instant::now();
        let hit = self.last_click.is_some_and(|(t, lp)| {
            now.duration_since(t) <= DOUBLE_CLICK && lp.dist(p) <= 5.0 * self.scale
        });
        self.last_click = if hit { None } else { Some((now, p)) };
        hit
    }

    fn press_editing(&mut self, p: Pt) {
        let Some(sel) = self.sel.as_ref().map(|s| s.rect) else {
            return;
        };
        let inside = sel.contains(p);

        if self.adjustable() {
            if let Some(handle) = self.handle_at(p) {
                self.drag = Drag::Resize { handle, orig: sel };
                return;
            }
            if inside {
                if self.is_double_click(p) {
                    self.confirm();
                } else {
                    self.drag = Drag::Move { grab: p, orig: sel };
                }
                return;
            }
        } else if self.tool == Tool::Select && inside {
            if self.is_double_click(p) {
                self.confirm();
            }
            return;
        }

        if !inside {
            // Clicking outside starts over, unless that would discard markup.
            if self.annotations.is_empty()
                && (self.tool == Tool::Select || self.ui.kind == Kind::Record)
            {
                self.clear_selection();
                self.press_selecting(p);
            }
            return;
        }

        match self.tool {
            Tool::Select => {}
            Tool::Pen | Tool::Highlighter => {
                self.draft = Some(Annotation {
                    shape: Shape::Stroke {
                        pts: vec![p],
                        highlighter: self.tool == Tool::Highlighter,
                    },
                    color: self.color,
                    width: self.stroke_width(),
                });
                self.drag = Drag::Draw { start: p };
            }
            Tool::Line | Tool::Arrow | Tool::Rect | Tool::Ellipse | Tool::Pixelate => {
                self.drag = Drag::Draw { start: p };
            }
            Tool::Text => {
                let m = crate::gfx::text::ui().metrics(self.text_em());
                // Place the caret so the click lands on the text's mid-line.
                self.text_edit = Some(TextEdit {
                    pos: Pt::new(p.x, p.y - m.line_height / 2.0),
                    text: String::new(),
                });
            }
            Tool::Eraser => {
                self.drag = Drag::Erase {
                    checkpointed: false,
                };
                self.erase_at(p);
            }
            Tool::Pipette => self.pick_color(p),
        }
    }

    fn release(&mut self) {
        let p = self.cursor;
        match std::mem::replace(&mut self.drag, Drag::None) {
            Drag::None | Drag::Move { .. } | Drag::Resize { .. } | Drag::Erase { .. } => {}
            Drag::Button(btn) => {
                if self.bars().hit(p) == Some(btn) {
                    self.activate(btn);
                }
                self.hover_btn = self.bars().hit(p);
            }
            Drag::NewRect { start } => {
                let rect = RectF::from_points(start, p).intersect(&self.bounds());
                if rect.w >= 3.0 && rect.h >= 3.0 {
                    self.set_selection(rect, None);
                }
            }
            Drag::Lasso { mut pts } => {
                let b = self.bounds();
                pts.push(Pt::new(
                    p.x.clamp(b.x, b.right()),
                    p.y.clamp(b.y, b.bottom()),
                ));
                self.finish_lasso(pts);
            }
            Drag::Draw { .. } => {
                if let Some(d) = self.draft.take() {
                    let keep = match &d.shape {
                        Shape::Stroke { .. } => true,
                        Shape::Line { a, b, .. } => a.dist(*b) >= 3.0,
                        Shape::Rect(r) | Shape::Ellipse(r) | Shape::Pixelate(r) => {
                            r.w >= 3.0 && r.h >= 3.0
                        }
                        Shape::Text { .. } => true,
                    };
                    if keep {
                        self.checkpoint();
                        self.annotations.push(d);
                    }
                }
            }
        }
    }

    fn update_draft(&mut self, start: Pt, p: Pt) {
        let shift = self.mods.shift;
        let (color, width) = (self.color, self.stroke_width());
        let square = |a: Pt, b: Pt| -> RectF {
            if !shift {
                return RectF::from_points(a, b);
            }
            let side = (b.x - a.x).abs().max((b.y - a.y).abs());
            let c = Pt::new(
                a.x + side * (b.x - a.x).signum(),
                a.y + side * (b.y - a.y).signum(),
            );
            RectF::from_points(a, c)
        };
        match self.tool {
            Tool::Pen | Tool::Highlighter => {
                if let Some(Annotation {
                    shape: Shape::Stroke { pts, .. },
                    ..
                }) = &mut self.draft
                {
                    if shift {
                        // Straight, axis-aligned stroke: ideal for underlining.
                        let end = if (p.x - start.x).abs() >= (p.y - start.y).abs() {
                            Pt::new(p.x, start.y)
                        } else {
                            Pt::new(start.x, p.y)
                        };
                        *pts = vec![start, end];
                    } else if pts.last().is_none_or(|l| l.dist(p) >= 1.0) {
                        pts.push(p);
                    }
                }
            }
            Tool::Line | Tool::Arrow => {
                let b = if shift { snap_angle(start, p) } else { p };
                self.draft = Some(Annotation {
                    shape: Shape::Line {
                        a: start,
                        b,
                        arrow: self.tool == Tool::Arrow,
                    },
                    color,
                    width,
                });
            }
            Tool::Rect => {
                self.draft = Some(Annotation {
                    shape: Shape::Rect(square(start, p)),
                    color,
                    width,
                })
            }
            Tool::Ellipse => {
                self.draft = Some(Annotation {
                    shape: Shape::Ellipse(square(start, p)),
                    color,
                    width,
                })
            }
            Tool::Pixelate => {
                self.draft = Some(Annotation {
                    shape: Shape::Pixelate(square(start, p)),
                    color,
                    width,
                })
            }
            _ => {}
        }
    }

    fn erase_at(&mut self, p: Pt) {
        let tol = 6.0 * self.scale;
        if let Some(i) = self.annotations.iter().rposition(|a| a.hit(p, tol)) {
            if let Drag::Erase { checkpointed } = &mut self.drag {
                if !*checkpointed {
                    *checkpointed = true;
                    self.undo.push(self.annotations.clone());
                    self.redo.clear();
                }
            }
            self.annotations.remove(i);
        }
    }

    fn pick_color(&mut self, p: Pt) {
        let (x, y) = (p.x as i32, p.y as i32);
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 {
            return;
        }
        let c = Color::from_u32(self.px[(y as u32 * self.w + x as u32) as usize]);
        self.color = c;
        let hex = c.to_hex_string();
        self.flash = Some((format!("Copied {hex}"), Instant::now()));
        self.outcome = Some(Outcome::CopyText(hex));
    }

    /// Show a short message at the top of the overlay.
    pub fn flash(&mut self, msg: impl Into<String>) {
        self.flash = Some((msg.into(), Instant::now()));
        self.dirty = true;
    }

    pub fn scroll(&mut self, dy: f32) {
        if dy == 0.0 {
            return;
        }
        if self.hover_btn == Some(Btn::Delay) {
            const STEPS: [u32; 3] = [3, 5, 10];
            let i = STEPS.iter().position(|s| *s >= self.ui.delay).unwrap_or(0);
            let n = if dy > 0.0 {
                (i + 1) % STEPS.len()
            } else {
                (i + STEPS.len() - 1) % STEPS.len()
            };
            self.ui.delay = STEPS[n];
            self.dirty = true;
        } else if self.sel.is_some() && self.tool.has_style() {
            let max = theme::WIDTHS.len() - 1;
            self.width_idx = if dy > 0.0 {
                (self.width_idx + 1).min(max)
            } else {
                self.width_idx.saturating_sub(1)
            };
            self.dirty = true;
        }
    }

    // ---- input: keyboard ---------------------------------------------------

    /// Esc / right click: undo one level of state, closing at the outermost.
    fn step_back(&mut self) {
        if !matches!(self.drag, Drag::None) {
            self.drag = Drag::None;
            self.draft = None;
        } else if self.text_edit.is_some() {
            self.commit_text();
        } else if self.sel.is_some() {
            self.clear_selection();
        } else {
            self.outcome = Some(Outcome::Cancel);
        }
    }

    /// Enter / double click: the primary action for the current state.
    fn confirm(&mut self) {
        if self.sel.is_none() {
            return;
        }
        match self.ui.kind {
            Kind::Snip => self.finish(Finish::Copy),
            Kind::Record => {
                if let Some(r) = self.selection() {
                    self.outcome = Some(Outcome::Record(r));
                }
            }
        }
    }

    pub fn text_input(&mut self, s: &str) {
        if let Some(edit) = &mut self.text_edit {
            edit.text.extend(s.chars().filter(|c| !c.is_control()));
            self.dirty = true;
        }
    }

    pub fn key(&mut self, key: Key) {
        self.dirty = true;
        if self.text_edit.is_some() {
            match key {
                Key::Escape => self.commit_text(),
                Key::Enter if self.mods.shift => {
                    if let Some(e) = &mut self.text_edit {
                        e.text.push('\n');
                    }
                }
                Key::Enter => self.commit_text(),
                Key::Backspace => {
                    if let Some(e) = &mut self.text_edit {
                        e.text.pop();
                    }
                }
                Key::Char(c) if self.mods.ctrl => self.ctrl_shortcut(c),
                _ => {}
            }
            return;
        }
        match key {
            Key::Escape => self.step_back(),
            Key::Enter => self.confirm(),
            Key::Char(c) if self.mods.ctrl => self.ctrl_shortcut(c),
            Key::Char(c) => self.plain_shortcut(c.to_ascii_lowercase()),
            Key::Left | Key::Right | Key::Up | Key::Down => self.nudge(key),
            Key::Backspace | Key::Delete => {}
        }
    }

    fn ctrl_shortcut(&mut self, c: char) {
        match c.to_ascii_lowercase() {
            'c' => self.confirm(),
            's' if self.sel.is_some() && self.ui.kind == Kind::Snip => self.finish(Finish::SaveAs),
            'z' if self.mods.shift => self.redo(),
            'z' => self.undo(),
            'y' => self.redo(),
            'a' => {
                self.clear_selection();
                self.select_all();
            }
            _ => {}
        }
    }

    fn plain_shortcut(&mut self, c: char) {
        if self.sel.is_none() {
            let mode = match c {
                'r' => Mode::Rect,
                'f' if self.ui.kind == Kind::Snip => Mode::Freeform,
                'w' => Mode::Window,
                'm' => Mode::Fullscreen,
                _ => return,
            };
            self.activate(Btn::Mode(mode));
            return;
        }
        if self.ui.kind != Kind::Snip {
            return;
        }
        let tool = match c {
            'v' | 's' => Tool::Select,
            'p' => Tool::Pen,
            'h' => Tool::Highlighter,
            'l' => Tool::Line,
            'a' => Tool::Arrow,
            'r' => Tool::Rect,
            'e' | 'o' => Tool::Ellipse,
            't' => Tool::Text,
            'b' => Tool::Pixelate,
            'x' => Tool::Eraser,
            'i' | 'c' => Tool::Pipette,
            _ => return,
        };
        self.tool = tool;
    }

    fn nudge(&mut self, key: Key) {
        if !self.adjustable() {
            return;
        }
        let step = if self.mods.shift { 10.0 } else { 1.0 };
        let (dx, dy) = match key {
            Key::Left => (-step, 0.0),
            Key::Right => (step, 0.0),
            Key::Up => (0.0, -step),
            _ => (0.0, step),
        };
        let b = self.bounds();
        if let Some(sel) = &mut self.sel {
            let r = sel.rect;
            sel.rect = if self.mods.ctrl {
                // Grow / shrink from the bottom-right corner.
                RectF::new(
                    r.x,
                    r.y,
                    (r.w + dx).clamp(1.0, b.w - r.x),
                    (r.h + dy).clamp(1.0, b.h - r.y),
                )
            } else {
                RectF::new(
                    (r.x + dx).clamp(0.0, b.w - r.w),
                    (r.y + dy).clamp(0.0, b.h - r.h),
                    r.w,
                    r.h,
                )
            };
        }
    }

    // ---- buttons -----------------------------------------------------------

    fn btn_enabled(&self, btn: Btn) -> bool {
        match btn {
            Btn::Undo => !self.undo.is_empty() || self.text_edit.is_some(),
            Btn::Redo => !self.redo.is_empty(),
            Btn::Mode(Mode::Freeform) => self.ui.kind == Kind::Snip,
            _ => true,
        }
    }

    fn btn_active(&self, btn: Btn) -> bool {
        match btn {
            Btn::Kind(k) => self.ui.kind == k,
            Btn::Mode(m) => self.ui.mode == m,
            Btn::Tool(t) => self.tool == t,
            Btn::Color(i) => theme::PALETTE[i as usize] == self.color,
            Btn::Width(i) => self.width_idx == i as usize,
            _ => false,
        }
    }

    /// Run a button's action (public so shortcuts and tests share one path).
    pub fn activate(&mut self, btn: Btn) {
        if !self.btn_enabled(btn) {
            return;
        }
        self.dirty = true;
        match btn {
            Btn::Kind(k) => {
                self.ui.kind = k;
                if k == Kind::Record && self.ui.mode == Mode::Freeform {
                    self.ui.mode = Mode::Rect;
                }
            }
            Btn::Mode(m) => {
                self.ui.mode = m;
                if m == Mode::Fullscreen && self.sel.is_none() {
                    self.select_all();
                }
            }
            Btn::Delay => self.outcome = Some(Outcome::Delay(self.ui.delay)),
            Btn::Cancel => self.outcome = Some(Outcome::Cancel),
            Btn::Tool(t) => {
                self.commit_text();
                self.tool = t;
            }
            Btn::Undo => self.undo(),
            Btn::Redo => self.redo(),
            Btn::Ocr => self.finish(Finish::ExtractText),
            Btn::Open => self.finish(Finish::OpenExternal),
            Btn::Save => self.finish(Finish::SaveAs),
            Btn::Copy => self.finish(Finish::Copy),
            Btn::Color(i) => self.color = theme::PALETTE[i as usize],
            Btn::Width(i) => self.width_idx = (i as usize).min(theme::WIDTHS.len() - 1),
            Btn::Record => self.confirm(),
        }
    }

    pub fn cursor_kind(&self) -> CursorKind {
        if let Drag::Move { .. } = self.drag {
            return CursorKind::Move;
        }
        if let Drag::Resize { handle, .. } = self.drag {
            return handle_cursor(handle);
        }
        if matches!(self.drag, Drag::None | Drag::Button(_)) {
            if self.hover_btn.is_some() {
                return CursorKind::Pointer;
            }
            if self.bars().contains(self.cursor) {
                return CursorKind::Default;
            }
        }
        let Some(sel) = &self.sel else {
            return match self.ui.mode {
                Mode::Rect | Mode::Freeform => CursorKind::Crosshair,
                Mode::Window | Mode::Fullscreen => CursorKind::Pointer,
            };
        };
        if let Some(h) = self.handle_at(self.cursor) {
            return handle_cursor(h);
        }
        let inside = sel.rect.contains(self.cursor);
        if self.adjustable() {
            return if inside {
                CursorKind::Move
            } else {
                CursorKind::Crosshair
            };
        }
        if !inside {
            return CursorKind::Default;
        }
        match self.tool {
            Tool::Select => CursorKind::Default,
            Tool::Text => CursorKind::Text,
            Tool::Eraser => CursorKind::Pointer,
            _ => CursorKind::Crosshair,
        }
    }
}

fn handle_cursor(h: Handle) -> CursorKind {
    match h {
        Handle::N | Handle::S => CursorKind::ResizeNs,
        Handle::E | Handle::W => CursorKind::ResizeEw,
        Handle::Ne | Handle::Sw => CursorKind::ResizeNesw,
        Handle::Nw | Handle::Se => CursorKind::ResizeNwse,
    }
}

/// Snap the direction `a`->`b` to the nearest 45 degree step.
fn snap_angle(a: Pt, b: Pt) -> Pt {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let len = (dx * dx + dy * dy).sqrt();
    let step = std::f32::consts::FRAC_PI_4;
    let ang = (dy.atan2(dx) / step).round() * step;
    Pt::new(a.x + len * ang.cos(), a.y + len * ang.sin())
}

/// Size of the text currently being typed (used for the caret box).
fn edit_size(edit: &TextEdit, em: f32) -> (f32, f32) {
    text_size(
        if edit.text.is_empty() {
            " "
        } else {
            &edit.text
        },
        em,
    )
}

#[cfg(test)]
mod tests;
