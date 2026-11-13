//! Small floating status surfaces: the "copied" toast, the delay countdown
//! and the recording indicator. Rendering is pure; windows live in `app`.

use crate::geom::{Pt, RectF};
use crate::gfx::{icons, text, theme, Canvas, Color};
use crate::output::Image;
use crate::record::Control;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

pub enum Hud {
    Toast {
        title: String,
        detail: String,
        /// Preview of what was captured.
        thumb: Option<Image>,
        /// Opened when the toast is clicked.
        action: Option<PathBuf>,
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
    },
}

/// What a click on a HUD does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HudHit {
    Pause,
    Stop,
    Other,
}

impl Hud {
    /// Size in logical pixels.
    pub fn size(&self) -> (f32, f32) {
        match self {
            Hud::Toast { title, detail, .. } => {
                let face = text::ui();
                let text_w = face.measure(title, 13.5).max(face.measure(detail, 11.5));
                ((112.0 + text_w + 22.0).clamp(280.0, 440.0), 78.0)
            }
            Hud::Countdown { .. } => (250.0, 52.0),
            Hud::Recording { .. } => (214.0, 46.0),
        }
    }

    /// The pause and stop buttons of the recording HUD (device pixels).
    fn recording_buttons(w: f32, h: f32, s: f32) -> (RectF, RectF) {
        let cy = h / 2.0;
        let stop = RectF::new(w - 78.0 * s, cy - 15.0 * s, 68.0 * s, 30.0 * s);
        let pause = RectF::new(stop.x - 36.0 * s, cy - 15.0 * s, 30.0 * s, 30.0 * s);
        (pause, stop)
    }

    /// What clicking at `p` (device pixels) on a `w` x `h` HUD does.
    pub fn hit(&self, p: Pt, w: f32, h: f32, scale: f32) -> HudHit {
        match self {
            Hud::Recording { .. } => {
                let (pause, _) = Self::recording_buttons(w, h, scale);
                if pause.contains(p) {
                    HudHit::Pause
                } else {
                    HudHit::Stop
                }
            }
            _ => HudHit::Other,
        }
    }

    /// Does the content change with time (needs periodic redraws)?
    pub fn animated(&self) -> bool {
        !matches!(self, Hud::Toast { .. })
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
                ..
            } => {
                let pad = 11.0 * s;
                let frame = RectF::new(pad, pad, 84.0 * s, fh - 2.0 * pad);
                c.fill_rrect(frame, 6.0 * s, Color::rgb(0x0E, 0x0F, 0x12));
                match thumb {
                    Some(img) => c.blit_rgba_fit(&img.rgba, img.w, img.h, frame.inflate(-3.0 * s)),
                    None => crate::brand::draw_mark(&mut c, frame.inflate(-6.0 * s), false),
                }

                let tx = frame.right() + 13.0 * s;
                let max_w = fw - tx - pad;
                let title_fit = ellipsize(title, 13.5 * s, max_w);
                let detail_fit = ellipsize(detail, 11.5 * s, max_w);
                face.draw(
                    &mut c,
                    &title_fit,
                    tx,
                    fh / 2.0 - 4.0 * s,
                    13.5 * s,
                    theme::TEXT,
                );
                face.draw(
                    &mut c,
                    &detail_fit,
                    tx,
                    fh / 2.0 + 15.0 * s,
                    11.5 * s,
                    theme::TEXT_DIM,
                );
                // Accent strip marks it as Krab's.
                c.fill_rect(RectF::new(0.0, 0.0, 3.0 * s, fh), theme::ACCENT);
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
            Hud::Recording {
                control,
                saving,
                pointer,
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
                let (pause, btn) = Self::recording_buttons(fw, fh, s);
                let over = |r: RectF| hovered && pointer.is_some_and(|p| r.contains(p));
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
                    if hovered && !over(pause) {
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
        let huds = [
            Hud::Toast {
                title: "Copied to clipboard".into(),
                detail: "Saved to Screenshots".into(),
                thumb: None,
                action: None,
            },
            Hud::Countdown {
                deadline: Instant::now() + std::time::Duration::from_secs(3),
            },
            Hud::Recording {
                control: Arc::new(Control::new()),
                saving: false,
                pointer: None,
            },
            Hud::Recording {
                control: Arc::new(Control::new()),
                saving: true,
                pointer: None,
            },
        ];
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
        let hud = Hud::Recording {
            control: Arc::new(Control::new()),
            saving: false,
            pointer: None,
        };
        let (w, h) = hud.size();
        let (pause, stop) = Hud::recording_buttons(w, h, 1.0);
        assert!(pause.right() < stop.x, "buttons overlap");
        assert_eq!(hud.hit(pause.center(), w, h, 1.0), HudHit::Pause);
        assert_eq!(hud.hit(stop.center(), w, h, 1.0), HudHit::Stop);
        assert_eq!(hud.hit(Pt::new(10.0, 10.0), w, h, 1.0), HudHit::Stop);
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
