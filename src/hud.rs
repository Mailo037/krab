//! Small floating status surfaces: the "copied" toast, the delay countdown
//! and the recording indicator. Rendering is pure; windows live in `app`.

use crate::geom::{Pt, RectF};
use crate::gfx::{icons, text, theme, Canvas, Color};
use crate::output::Image;
use crate::record::Control;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Where on the screen notifications appear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastPos {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl ToastPos {
    pub const ALL: [ToastPos; 6] = [
        ToastPos::TopLeft,
        ToastPos::TopCenter,
        ToastPos::TopRight,
        ToastPos::BottomLeft,
        ToastPos::BottomCenter,
        ToastPos::BottomRight,
    ];

    /// The name in `config.toml`.
    pub fn key(self) -> &'static str {
        match self {
            ToastPos::TopLeft => "top-left",
            ToastPos::TopCenter => "top-center",
            ToastPos::TopRight => "top-right",
            ToastPos::BottomLeft => "bottom-left",
            ToastPos::BottomCenter => "bottom-center",
            ToastPos::BottomRight => "bottom-right",
        }
    }

    /// Unknown names give the default, bottom right.
    pub fn parse(s: &str) -> ToastPos {
        let s = s.trim().to_ascii_lowercase().replace(['_', ' '], "-");
        Self::ALL
            .into_iter()
            .find(|p| p.key() == s)
            .unwrap_or(ToastPos::BottomRight)
    }

    pub fn top(self) -> bool {
        matches!(
            self,
            ToastPos::TopLeft | ToastPos::TopCenter | ToastPos::TopRight
        )
    }

    /// -1 left, 0 middle, 1 right.
    pub fn column(self) -> i32 {
        match self {
            ToastPos::TopLeft | ToastPos::BottomLeft => -1,
            ToastPos::TopCenter | ToastPos::BottomCenter => 0,
            ToastPos::TopRight | ToastPos::BottomRight => 1,
        }
    }

    /// Top-left corner of a `w` x `h` toast inside `area`, `margin` away
    /// from its edges.
    pub fn place(self, area: crate::geom::RectI, w: i32, h: i32, margin: i32) -> (i32, i32) {
        let x = match self.column() {
            -1 => area.x + margin,
            0 => area.x + (area.w - w) / 2,
            _ => area.right() - w - margin,
        };
        let y = if self.top() {
            area.y + margin
        } else {
            area.bottom() - h - margin
        };
        (x, y)
    }
}

/// How big notifications are.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastSize {
    /// One line with an icon, no picture.
    Small,
    /// A small picture of the snip next to the text.
    Medium,
    /// A big picture of the snip with the text below it.
    Large,
}

impl ToastSize {
    pub fn key(self) -> &'static str {
        match self {
            ToastSize::Small => "small",
            ToastSize::Medium => "medium",
            ToastSize::Large => "large",
        }
    }

    pub fn parse(s: &str) -> ToastSize {
        match s.trim().to_ascii_lowercase().as_str() {
            "small" => ToastSize::Small,
            "large" => ToastSize::Large,
            _ => ToastSize::Medium,
        }
    }

    /// Largest picture kept for a toast of this size.
    pub fn thumb_size(self) -> (u32, u32) {
        match self {
            ToastSize::Large => (760, 420),
            _ => (320, 220),
        }
    }
}

/// What kind of news a toast brings; picks its icon and colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    /// Something worked (copied, saved).
    Done,
    /// Something did not work.
    Problem,
    /// News about Krab itself (running, updated).
    Info,
}

impl Tone {
    /// Guess the tone from a toast's title.
    pub fn guess(title: &str) -> Tone {
        let t = title.to_ascii_lowercase();
        if ["could not", "failed", "no text"]
            .iter()
            .any(|w| t.contains(w))
        {
            Tone::Problem
        } else if t.starts_with("krab ") || t.starts_with("notifications") {
            Tone::Info
        } else {
            Tone::Done
        }
    }

    pub fn color(self) -> Color {
        match self {
            Tone::Done => Color::rgb(0x3D, 0xD6, 0x8C),
            Tone::Problem => theme::DANGER,
            Tone::Info => theme::ACCENT,
        }
    }
}

/// How long a toast stays when the pointer is not on it.
pub const TOAST_TIME: Duration = Duration::from_millis(4000);

/// Where the parts of a toast go (device pixels).
struct ToastLayout {
    w: f32,
    h: f32,
    /// The picture of the snip, if it is shown.
    thumb: Option<RectF>,
    /// The round icon: centre and radius.
    badge: (Pt, f32),
    text_x: f32,
    title_em: f32,
    detail_em: f32,
    /// Vertical centre of the text block.
    text_cy: f32,
    close: RectF,
}

pub enum Hud {
    Toast {
        title: String,
        detail: String,
        /// Preview of what was captured.
        thumb: Option<Image>,
        /// Opened when the toast is clicked.
        action: Option<PathBuf>,
        tone: Tone,
        size: ToastSize,
        /// How much of its time is left, 1 to 0 (the bar along the bottom).
        left: f32,
        /// Pointer position (device pixels) while it is over the toast.
        pointer: Option<Pt>,
    },
    Countdown {
        deadline: Instant,
    },
    Recording {
        control: Arc<Control>,
        /// Set once stop was requested and the file is being finalized.
        saving: bool,
        /// Pointer position (device pixels) while it is over the HUD.
        pointer: Option<Pt>,
        /// Markup can be drawn over the recording (`None`: not here), and
        /// whether the user is drawing right now.
        drawing: Option<bool>,
    },
    /// A scrolling capture is collecting rows.
    Scrolling {
        /// Rows collected so far.
        height: Arc<AtomicU32>,
        /// Krab scrolls by itself (otherwise the user scrolls).
        auto: bool,
    },
}

/// What a click on a HUD does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudHit {
    /// Start or stop drawing over the recording.
    Draw,
    Pause,
    Stop,
    /// The close button of a toast.
    Close,
    Other,
}

impl Hud {
    pub fn toast(
        title: String,
        detail: String,
        thumb: Option<Image>,
        action: Option<PathBuf>,
        size: ToastSize,
    ) -> Hud {
        Hud::Toast {
            tone: Tone::guess(&title),
            title,
            detail,
            thumb,
            action,
            size,
            left: 1.0,
            pointer: None,
        }
    }

    /// Remember where the pointer is over the HUD (device pixels).
    pub fn set_pointer(&mut self, p: Option<Pt>) {
        if let Hud::Toast { pointer, .. } | Hud::Recording { pointer, .. } = self {
            *pointer = p;
        }
    }

    fn toast_layout(
        title: &str,
        detail: &str,
        thumb: Option<&Image>,
        size: ToastSize,
        s: f32,
    ) -> ToastLayout {
        let face = text::ui();
        let text_w = |title_em: f32, detail_em: f32| {
            face.measure(title, title_em)
                .max(face.measure(detail, detail_em))
        };
        // Built in logical pixels, scaled at the end.
        let mut l = match (size, thumb) {
            (ToastSize::Small, _) => {
                let (te, de) = (12.5, 11.0);
                let text_x = 48.0;
                ToastLayout {
                    w: (text_x + text_w(te, de) + 40.0).clamp(240.0, 380.0),
                    h: 54.0,
                    thumb: None,
                    badge: (Pt::new(26.0, 27.0), 13.0),
                    text_x,
                    title_em: te,
                    detail_em: de,
                    text_cy: 27.0,
                    close: RectF::default(),
                }
            }
            (ToastSize::Medium, Some(_)) => {
                let (te, de) = (13.5, 11.5);
                let thumb = RectF::new(12.0, 12.0, 96.0, 60.0);
                let text_x = thumb.right() + 16.0;
                ToastLayout {
                    w: (text_x + text_w(te, de) + 44.0).clamp(320.0, 480.0),
                    h: 84.0,
                    thumb: Some(thumb),
                    badge: (Pt::new(thumb.right() - 2.0, thumb.bottom() - 2.0), 9.0),
                    text_x,
                    title_em: te,
                    detail_em: de,
                    text_cy: 42.0,
                    close: RectF::default(),
                }
            }
            (ToastSize::Medium, None) => {
                let (te, de) = (13.5, 11.5);
                let text_x = 64.0;
                ToastLayout {
                    w: (text_x + text_w(te, de) + 44.0).clamp(300.0, 460.0),
                    h: 72.0,
                    thumb: None,
                    badge: (Pt::new(34.0, 36.0), 18.0),
                    text_x,
                    title_em: te,
                    detail_em: de,
                    text_cy: 36.0,
                    close: RectF::default(),
                }
            }
            (ToastSize::Large, Some(img)) => {
                let (te, de) = (14.5, 12.0);
                let w = 400.0;
                let pw = w - 24.0;
                let ph = (pw * img.h as f32 / img.w.max(1) as f32).clamp(110.0, 220.0);
                let thumb = RectF::new(12.0, 12.0, pw, ph);
                let row = thumb.bottom() + 34.0;
                ToastLayout {
                    w,
                    h: thumb.bottom() + 68.0,
                    thumb: Some(thumb),
                    badge: (Pt::new(29.0, row), 15.0),
                    text_x: 56.0,
                    title_em: te,
                    detail_em: de,
                    text_cy: row,
                    close: RectF::default(),
                }
            }
            (ToastSize::Large, None) => {
                let (te, de) = (15.5, 12.5);
                let text_x = 80.0;
                ToastLayout {
                    w: (text_x + text_w(te, de) + 48.0).clamp(360.0, 520.0),
                    h: 96.0,
                    thumb: None,
                    badge: (Pt::new(42.0, 48.0), 24.0),
                    text_x,
                    title_em: te,
                    detail_em: de,
                    text_cy: 48.0,
                    close: RectF::default(),
                }
            }
        };
        l.close = RectF::new(l.w - 32.0, 8.0, 24.0, 24.0);
        let k = |r: RectF| RectF::new(r.x * s, r.y * s, r.w * s, r.h * s);
        ToastLayout {
            w: l.w * s,
            h: l.h * s,
            thumb: l.thumb.map(k),
            badge: (Pt::new(l.badge.0.x * s, l.badge.0.y * s), l.badge.1 * s),
            text_x: l.text_x * s,
            title_em: l.title_em * s,
            detail_em: l.detail_em * s,
            text_cy: l.text_cy * s,
            close: k(l.close),
        }
    }

    /// Size in logical pixels.
    pub fn size(&self) -> (f32, f32) {
        match self {
            Hud::Toast {
                title,
                detail,
                thumb,
                size,
                ..
            } => {
                let l = Self::toast_layout(title, detail, thumb.as_ref(), *size, 1.0);
                (l.w, l.h)
            }
            Hud::Countdown { .. } => (250.0, 52.0),
            Hud::Recording { drawing, .. } => (if drawing.is_some() { 250.0 } else { 214.0 }, 46.0),
            Hud::Scrolling { .. } => (330.0, 46.0),
        }
    }

    /// The draw, pause and stop buttons of the recording HUD (device pixels).
    fn recording_buttons(w: f32, h: f32, s: f32) -> (RectF, RectF, RectF) {
        let cy = h / 2.0;
        let stop = RectF::new(w - 78.0 * s, cy - 15.0 * s, 68.0 * s, 30.0 * s);
        let pause = RectF::new(stop.x - 36.0 * s, cy - 15.0 * s, 30.0 * s, 30.0 * s);
        let draw = RectF::new(pause.x - 36.0 * s, cy - 15.0 * s, 30.0 * s, 30.0 * s);
        (draw, pause, stop)
    }

    /// What clicking at `p` (device pixels) on a `w` x `h` HUD does.
    pub fn hit(&self, p: Pt, w: f32, h: f32, scale: f32) -> HudHit {
        match self {
            Hud::Recording { drawing, .. } => {
                let (draw, pause, _) = Self::recording_buttons(w, h, scale);
                if drawing.is_some() && draw.contains(p) {
                    HudHit::Draw
                } else if pause.contains(p) {
                    HudHit::Pause
                } else {
                    HudHit::Stop
                }
            }
            Hud::Scrolling { .. } => HudHit::Stop,
            Hud::Toast {
                title,
                detail,
                thumb,
                size,
                ..
            } => {
                let l = Self::toast_layout(title, detail, thumb.as_ref(), *size, scale);
                if l.close.inflate(4.0 * scale).contains(p) {
                    HudHit::Close
                } else {
                    HudHit::Other
                }
            }
            _ => HudHit::Other,
        }
    }

    /// How often the content changes (it needs redraws that often), if at all.
    pub fn frame_interval(&self) -> Option<Duration> {
        Some(match self {
            // The time bar along the bottom moves smoothly.
            Hud::Toast { .. } => Duration::from_millis(33),
            _ => Duration::from_millis(200),
        })
    }

    pub fn render(&self, buf: &mut [u32], w: u32, h: u32, scale: f32, hovered: bool) {
        if w == 0 || h == 0 || buf.len() != (w * h) as usize {
            return;
        }
        let s = scale;
        buf.fill(theme::PANEL_SOLID.to_u32());
        let mut c = Canvas::new(buf, w, h);
        let (fw, fh) = (w as f32, h as f32);
        if hovered {
            c.fill_rect(RectF::new(0.0, 0.0, fw, fh), Color::rgba(255, 255, 255, 12));
        }

        let face = text::ui();

        match self {
            Hud::Toast {
                title,
                detail,
                thumb,
                tone,
                size,
                left,
                pointer,
                ..
            } => {
                let l = Self::toast_layout(title, detail, thumb.as_ref(), *size, s);
                let col = tone.color();

                // The picture of the snip, rounded, filling its box.
                if let (Some(r), Some(img)) = (l.thumb, thumb) {
                    let (px, iw, ih) = cover(img, r.w / r.h);
                    c.draw_image(&px, iw, ih, r, 7.0 * s);
                    c.stroke_rrect(r, 7.0 * s, 1.0 * s, Color::rgba(255, 255, 255, 22));
                }

                // The round icon: a small badge on the picture's corner, or
                // a tinted circle on its own.
                let (bc, br) = l.badge;
                let on_picture = matches!(size, ToastSize::Medium) && l.thumb.is_some();
                if on_picture {
                    c.fill_circle(bc, br + 2.5 * s, theme::PANEL_SOLID);
                    c.fill_circle(bc, br, col);
                    draw_tone_icon(&mut c, *tone, bc, br * 1.15, theme::PANEL_SOLID);
                } else {
                    c.fill_circle(bc, br, col.alpha(0.16));
                    draw_tone_icon(&mut c, *tone, bc, br * 1.05, col);
                }

                // Title and detail, centred as a block on `text_cy`.
                let max_w = fw - l.text_x - 36.0 * s;
                let title_fit = ellipsize(title, l.title_em, max_w);
                if detail.is_empty() {
                    let base = face.baseline_for_center(l.text_cy, l.title_em);
                    face.draw(&mut c, &title_fit, l.text_x, base, l.title_em, theme::TEXT);
                } else {
                    let gap = 4.0 * s;
                    let block = l.title_em + gap + l.detail_em;
                    let top = l.text_cy - block / 2.0;
                    let t_base = face.baseline_for_center(top + l.title_em / 2.0, l.title_em);
                    let d_base = face.baseline_for_center(
                        top + l.title_em + gap + l.detail_em / 2.0,
                        l.detail_em,
                    );
                    face.draw(
                        &mut c,
                        &title_fit,
                        l.text_x,
                        t_base,
                        l.title_em,
                        theme::TEXT,
                    );
                    let detail_fit = ellipsize(detail, l.detail_em, max_w);
                    face.draw(
                        &mut c,
                        &detail_fit,
                        l.text_x,
                        d_base,
                        l.detail_em,
                        theme::TEXT_DIM,
                    );
                }

                // Close button, shown while the pointer is on the toast.
                if hovered {
                    let over = pointer.is_some_and(|p| l.close.inflate(4.0 * s).contains(p));
                    let bg = if over {
                        Color::rgba(255, 255, 255, 40)
                    } else if l.thumb.is_some() && matches!(size, ToastSize::Large) {
                        Color::rgba(0, 0, 0, 140)
                    } else {
                        Color::rgba(255, 255, 255, 14)
                    };
                    c.fill_circle(l.close.center(), l.close.w / 2.0, bg);
                    icons::draw(
                        &mut c,
                        icons::Icon::Close,
                        l.close.center(),
                        13.0 * s,
                        theme::TEXT,
                    );
                }

                // Time left, as a thin bar along the bottom edge.
                let bar = 3.0 * s;
                c.fill_rect(
                    RectF::new(0.0, fh - bar, fw * left.clamp(0.0, 1.0), bar),
                    col.alpha(0.85),
                );
            }
            Hud::Countdown { deadline } => {
                let left = deadline
                    .saturating_duration_since(Instant::now())
                    .as_secs_f32()
                    .ceil()
                    .max(1.0) as u32;
                icons::draw(
                    &mut c,
                    icons::Icon::Timer,
                    Pt::new(26.0 * s, fh / 2.0),
                    22.0 * s,
                    theme::ACCENT,
                );
                let base = face.baseline_for_center(fh / 2.0, 14.0 * s);
                let x = 46.0 * s;
                let adv = face.draw(&mut c, "Snip in ", x, base, 14.0 * s, theme::TEXT_DIM);
                face.draw(
                    &mut c,
                    &format!("{left}"),
                    x + adv,
                    base,
                    14.0 * s,
                    theme::TEXT,
                );
                let hint = "click to cancel";
                let hw = face.measure(hint, 10.5 * s);
                face.draw(
                    &mut c,
                    hint,
                    fw - hw - 12.0 * s,
                    base,
                    10.5 * s,
                    theme::ICON_DISABLED,
                );
            }
            Hud::Scrolling { height, auto } => {
                let cy = fh / 2.0;
                icons::draw(
                    &mut c,
                    icons::Icon::Scroll,
                    Pt::new(24.0 * s, cy),
                    20.0 * s,
                    theme::ACCENT,
                );
                let rows = height.load(Ordering::Relaxed);
                let label = if *auto {
                    format!("Scrolling... {rows} px")
                } else {
                    format!("Scroll now... {rows} px")
                };
                let base = face.baseline_for_center(cy, 13.0 * s);
                face.draw(&mut c, &label, 44.0 * s, base, 13.0 * s, theme::TEXT);
                let btn = RectF::new(fw - 78.0 * s, cy - 15.0 * s, 68.0 * s, 30.0 * s);
                c.fill_rrect(
                    btn,
                    7.0 * s,
                    if hovered { theme::ACCENT } else { theme::HOVER },
                );
                face.draw_centered(&mut c, "Done", btn.center().x, cy, 12.5 * s, Color::WHITE);
            }
            Hud::Recording {
                control,
                saving,
                pointer,
                drawing,
            } => {
                let cy = fh / 2.0;
                if *saving {
                    face.draw_centered(
                        &mut c,
                        "Saving recording...",
                        fw / 2.0,
                        cy,
                        13.0 * s,
                        theme::TEXT,
                    );
                    return;
                }
                let paused = control.is_paused();
                let elapsed = control.elapsed();
                // Blink the dot once a second; hold it dim while paused.
                let on = !paused && elapsed.as_millis() % 1000 < 600;
                c.fill_circle(
                    Pt::new(22.0 * s, cy),
                    6.0 * s,
                    if on {
                        theme::DANGER
                    } else {
                        theme::DANGER.alpha(0.35)
                    },
                );
                let label = crate::record::format_elapsed(elapsed);
                let base = face.baseline_for_center(cy, 14.5 * s);
                face.draw(
                    &mut c,
                    &label,
                    38.0 * s,
                    base,
                    14.5 * s,
                    if paused { theme::TEXT_DIM } else { theme::TEXT },
                );
                let (draw, pause, btn) = Self::recording_buttons(fw, fh, s);
                let over = |r: RectF| hovered && pointer.is_some_and(|p| r.contains(p));
                if let Some(on) = *drawing {
                    if on {
                        c.fill_rrect(draw, 7.0 * s, theme::ACCENT);
                    } else if over(draw) {
                        c.fill_rrect(draw, 7.0 * s, theme::HOVER);
                    }
                    icons::draw(
                        &mut c,
                        icons::Icon::Pen,
                        draw.center(),
                        18.0 * s,
                        Color::WHITE,
                    );
                }
                if over(pause) || paused {
                    c.fill_rrect(pause, 7.0 * s, theme::HOVER);
                }
                icons::draw(
                    &mut c,
                    if paused {
                        icons::Icon::Play
                    } else {
                        icons::Icon::Pause
                    },
                    pause.center(),
                    18.0 * s,
                    Color::WHITE,
                );
                c.fill_rrect(
                    btn,
                    7.0 * s,
                    if hovered && !over(pause) && !(drawing.is_some() && over(draw)) {
                        theme::DANGER
                    } else {
                        theme::HOVER
                    },
                );
                icons::draw(
                    &mut c,
                    icons::Icon::Stop,
                    Pt::new(btn.x + 17.0 * s, cy),
                    18.0 * s,
                    Color::WHITE,
                );
                face.draw(
                    &mut c,
                    "Stop",
                    btn.x + 30.0 * s,
                    face.baseline_for_center(cy, 12.5 * s),
                    12.5 * s,
                    Color::WHITE,
                );
            }
        }
    }
}

/// Trim `text` with an ellipsis so it fits `max_w`.
pub fn ellipsize(text: &str, em: f32, max_w: f32) -> String {
    let face = text::ui();
    if face.measure(text, em) <= max_w {
        return text.to_string();
    }
    let mut out: String = text.to_string();
    while !out.is_empty() && face.measure(&format!("{out}..."), em) > max_w {
        out.pop();
    }
    format!("{}...", out.trim_end())
}

/// The middle part of `img` with the given width / height ratio, as an
/// `0xAARRGGBB` buffer, so the picture fills its box without bars.
fn cover(img: &Image, aspect: f32) -> (Vec<u32>, u32, u32) {
    let (iw, ih) = (img.w.max(1), img.h.max(1));
    let (cw, ch) = if iw as f32 / ih as f32 > aspect {
        (((ih as f32 * aspect).round() as u32).clamp(1, iw), ih)
    } else {
        (iw, ((iw as f32 / aspect).round() as u32).clamp(1, ih))
    };
    let (x0, y0) = ((iw - cw) / 2, (ih - ch) / 2);
    let mut px = Vec::with_capacity((cw * ch) as usize);
    for y in y0..y0 + ch {
        let row = (y * iw + x0) as usize * 4;
        px.extend(crate::gfx::rgba_to_argb(
            &img.rgba[row..row + cw as usize * 4],
        ));
    }
    (px, cw, ch)
}

/// The sign inside a toast's round icon: a tick, an exclamation mark or
/// Krab's mark.
fn draw_tone_icon(c: &mut Canvas, tone: Tone, at: Pt, size: f32, col: Color) {
    match tone {
        Tone::Done => icons::draw(c, icons::Icon::Check, at, size, col),
        Tone::Problem => {
            let w = size * 0.13;
            c.line(
                Pt::new(at.x, at.y - size * 0.3),
                Pt::new(at.x, at.y + size * 0.06),
                w,
                col,
            );
            c.fill_circle(Pt::new(at.x, at.y + size * 0.3), w * 0.62, col);
        }
        Tone::Info => {
            let r = size * 0.55;
            crate::brand::draw_mark(c, RectF::new(at.x - r, at.y - r, 2.0 * r, 2.0 * r), false);
        }
    }
}

/// A made-up snip for the preview toast in the settings.
pub fn sample_image() -> Image {
    let (w, h) = (480u32, 300u32);
    let mut buf = vec![0u32; (w * h) as usize];
    let mut c = Canvas::new(&mut buf, w, h);
    let (fw, fh) = (w as f32, h as f32);
    if let Some(path) = crate::gfx::rrect_path(RectF::new(0.0, 0.0, fw, fh), 0.0) {
        c.fill_path_gradient(
            &path,
            Pt::new(0.0, 0.0),
            Color::rgb(0x2B, 0x3A, 0x67),
            Pt::new(fw, fh),
            Color::rgb(0x7B, 0x2F, 0x87),
        );
    }
    // A window with a title bar, a few lines of text and a chart.
    let win = RectF::new(60.0, 44.0, 360.0, 212.0);
    c.fill_rrect(win, 10.0, Color::rgb(0xF4, 0xF5, 0xF8));
    c.fill_rect(
        RectF::new(win.x, win.y + 30.0, win.w, 1.0),
        Color::rgb(0xDD, 0xDF, 0xE5),
    );
    for (i, col) in [
        theme::DANGER,
        Color::rgb(0xFF, 0xBD, 0x2E),
        Color::rgb(0x28, 0xC8, 0x40),
    ]
    .into_iter()
    .enumerate()
    {
        c.fill_circle(
            Pt::new(win.x + 18.0 + i as f32 * 16.0, win.y + 15.0),
            5.0,
            col,
        );
    }
    for (i, len) in [150.0, 110.0, 130.0].into_iter().enumerate() {
        c.fill_rrect(
            RectF::new(win.x + 20.0, win.y + 50.0 + i as f32 * 20.0, len, 9.0),
            4.5,
            Color::rgb(0xC9, 0xCC, 0xD6),
        );
    }
    for (i, bh) in [50.0, 80.0, 64.0, 110.0, 92.0].into_iter().enumerate() {
        let x = win.x + 200.0 + i as f32 * 28.0;
        let col = if i == 3 {
            theme::ACCENT
        } else {
            Color::rgb(0x6C, 0x7B, 0xF2)
        };
        c.fill_rrect(RectF::new(x, win.bottom() - 22.0 - bh, 18.0, bh), 4.0, col);
    }
    Image {
        w,
        h,
        rgba: crate::gfx::argb_to_rgba(&buf),
    }
}

/// Shrink a capture to a small preview so toasts stay cheap to keep around.
pub fn thumbnail(img: &Image, max_w: u32, max_h: u32) -> Image {
    let k = (max_w as f32 / img.w as f32)
        .min(max_h as f32 / img.h as f32)
        .min(1.0);
    let (w, h) = (
        ((img.w as f32 * k) as u32).max(1),
        ((img.h as f32 * k) as u32).max(1),
    );
    let mut buf = vec![0xFF0E_0F12u32; (w * h) as usize];
    let mut c = Canvas::new(&mut buf, w, h);
    // Composite on the toast's dark frame colour so transparent areas look right.
    let mut flat = Vec::with_capacity(img.rgba.len());
    for p in img.rgba.chunks_exact(4) {
        let a = p[3] as u32;
        let f = |c: u8, bg: u32| ((c as u32 * a + bg * (255 - a)) / 255) as u8;
        flat.extend_from_slice(&[f(p[0], 0x0E), f(p[1], 0x0F), f(p[2], 0x12), 255]);
    }
    c.blit_rgba_fit(
        &flat,
        img.w,
        img.h,
        RectF::new(0.0, 0.0, w as f32, h as f32),
    );
    Image {
        w,
        h,
        rgba: crate::gfx::argb_to_rgba(&buf),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_hud_renders_opaque() {
        let mut huds = Vec::new();
        for size in [ToastSize::Small, ToastSize::Medium, ToastSize::Large] {
            for thumb in [None, Some(thumbnail(&sample_image(), 320, 220))] {
                for title in ["Copied to clipboard", "Could not save", "Krab is running"] {
                    huds.push(Hud::toast(
                        title.into(),
                        "Saved to Screenshots".into(),
                        thumb.clone(),
                        None,
                        size,
                    ));
                }
            }
        }
        huds.extend([
            Hud::Countdown {
                deadline: Instant::now() + std::time::Duration::from_secs(3),
            },
            Hud::Recording {
                control: Arc::new(Control::new()),
                saving: false,
                pointer: None,
                drawing: None,
            },
            Hud::Recording {
                control: Arc::new(Control::new()),
                saving: false,
                pointer: None,
                drawing: Some(true),
            },
            Hud::Recording {
                control: Arc::new(Control::new()),
                saving: true,
                pointer: None,
                drawing: Some(false),
            },
            Hud::Scrolling {
                height: Arc::new(AtomicU32::new(2400)),
                auto: true,
            },
        ]);
        for hud in &huds {
            for scale in [1.0f32, 1.5, 2.0] {
                let (lw, lh) = hud.size();
                let (w, h) = ((lw * scale) as u32, (lh * scale) as u32);
                let mut buf = vec![0u32; (w * h) as usize];
                hud.render(&mut buf, w, h, scale, false);
                assert!(buf.iter().all(|p| p >> 24 == 0xFF));
                assert!(buf.iter().any(|p| *p != theme::PANEL_SOLID.to_u32()));
            }
        }
    }

    #[test]
    fn recording_hud_tells_pause_from_stop() {
        for drawing in [None, Some(false)] {
            let hud = Hud::Recording {
                control: Arc::new(Control::new()),
                saving: false,
                pointer: None,
                drawing,
            };
            let (w, h) = hud.size();
            let (draw, pause, stop) = Hud::recording_buttons(w, h, 1.0);
            assert!(pause.right() < stop.x, "buttons overlap");
            if drawing.is_some() {
                assert!(draw.right() < pause.x, "buttons overlap");
                // The time ("59:59" ends near 90 px) keeps clear of the buttons.
                assert!(draw.x >= 95.0, "no room for the time");
            }
            assert_eq!(hud.hit(pause.center(), w, h, 1.0), HudHit::Pause);
            assert_eq!(hud.hit(stop.center(), w, h, 1.0), HudHit::Stop);
            assert_eq!(hud.hit(Pt::new(10.0, 10.0), w, h, 1.0), HudHit::Stop);
            let expect = if drawing.is_some() {
                HudHit::Draw
            } else {
                HudHit::Stop
            };
            assert_eq!(hud.hit(draw.center(), w, h, 1.0), expect);
        }
    }

    #[test]
    fn toast_positions_stay_inside_the_work_area() {
        let area = crate::geom::RectI::new(0, 0, 1920, 1040);
        for pos in ToastPos::ALL {
            assert_eq!(ToastPos::parse(pos.key()), pos);
            let (x, y) = pos.place(area, 360, 84, 16);
            assert!(x >= 16 && x + 360 <= 1920 - 16, "{pos:?}");
            assert!(y >= 16 && y + 84 <= 1040 - 16, "{pos:?}");
            assert_eq!(y == 16, pos.top(), "{pos:?}");
        }
        assert_eq!(ToastPos::parse("Top Left"), ToastPos::TopLeft);
        assert_eq!(ToastPos::parse("nowhere"), ToastPos::BottomRight);
    }

    #[test]
    fn toast_tone_follows_the_title() {
        assert_eq!(Tone::guess("Copied to clipboard"), Tone::Done);
        assert_eq!(Tone::guess("Could not save"), Tone::Problem);
        assert_eq!(Tone::guess("Recording failed"), Tone::Problem);
        assert_eq!(
            Tone::guess("Krab could not open the overlay"),
            Tone::Problem
        );
        assert_eq!(Tone::guess("Krab is running"), Tone::Info);
    }

    #[test]
    fn toast_close_button_is_hit() {
        for size in [ToastSize::Small, ToastSize::Medium, ToastSize::Large] {
            let hud = Hud::toast("Copied".into(), "x".into(), None, None, size);
            let (w, h) = hud.size();
            assert_eq!(hud.hit(Pt::new(w - 20.0, 20.0), w, h, 1.0), HudHit::Close);
            assert_eq!(hud.hit(Pt::new(w / 2.0, h - 6.0), w, h, 1.0), HudHit::Other);
        }
    }

    #[test]
    fn cover_crops_to_the_aspect() {
        let img = Image {
            w: 400,
            h: 100,
            rgba: vec![200; 400 * 100 * 4],
        };
        let (px, w, h) = cover(&img, 2.0);
        assert_eq!((w, h), (200, 100));
        assert_eq!(px.len(), 200 * 100);
    }

    #[test]
    fn thumbnail_keeps_aspect_and_never_upscales() {
        let img = Image {
            w: 400,
            h: 100,
            rgba: vec![200; 400 * 100 * 4],
        };
        let t = thumbnail(&img, 160, 120);
        assert_eq!((t.w, t.h), (160, 40));
        let small = Image {
            w: 20,
            h: 10,
            rgba: vec![200; 20 * 10 * 4],
        };
        let t = thumbnail(&small, 160, 120);
        assert_eq!((t.w, t.h), (20, 10));
    }

    #[test]
    fn long_text_is_ellipsized() {
        let s = ellipsize("A very long file name that will never fit.png", 12.0, 80.0);
        assert!(s.ends_with("...") && text::ui().measure(&s, 12.0) <= 80.0);
        assert_eq!(ellipsize("ok", 12.0, 80.0), "ok");
    }
}
