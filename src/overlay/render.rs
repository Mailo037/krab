//! Drawing the overlay: dimmed screenshot, selection, markup and toolbars.

use super::layout::{Bar, Bars};
use super::{edit_size, Btn, Drag, Kind, Mode, Overlay, Win};
use crate::annotate::{Annotation, Shape, Source, Tool};
use crate::geom::{merge_rects, Pt, RectF, RectI};
use crate::gfx::{icons, text, theme, Canvas, Color};
use tiny_skia::{BlendMode, LineCap, LineJoin, PathBuilder, Stroke, StrokeDash, Transform};

/// Darken a row of pixels to ~44% brightness. Branch-free, so the compiler
/// vectorizes it; a 4K frame takes a couple of milliseconds.
#[inline]
fn dim_row(dst: &mut [u32], src: &[u32]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d = dim(*s);
    }
}

#[inline(always)]
fn dim(p: u32) -> u32 {
    let half = (p >> 1) & 0x007F_7F7F;
    let sixteenth = (p >> 4) & 0x000F_0F0F;
    0xFF00_0000 | (half - sixteenth)
}

#[inline]
fn mix(a: u32, b: u32, t: u32) -> u32 {
    // a * (255 - t) + b * t, per channel
    let inv = 255 - t;
    let ch =
        |shift: u32| ((((a >> shift) & 0xFF) * inv + ((b >> shift) & 0xFF) * t) / 255) << shift;
    0xFF00_0000 | ch(16) | ch(8) | ch(0)
}

/// The parts of a frame that decide what the next frame must repaint.
pub(super) struct PrevFrame {
    size: (u32, u32),
    bright: Option<RectI>,
    masked: bool,
    version: u64,
    draft: Option<RectI>,
    chrome: Vec<RectI>,
}

/// The selection with its finished markup, rendered once.
pub(super) struct Layer {
    rect: RectI,
    version: u64,
    px: Vec<u32>,
}

struct FrameInfo {
    bars: Bars,
    over_bar: bool,
    full: RectI,
    idle: bool,
    dragging_rect: Option<RectI>,
    hover_target: Option<RectI>,
    sel_rect: Option<RectI>,
    /// Shown undimmed.
    bright: Option<RectI>,
    /// A freeform mask shapes the bright area.
    masked: bool,
    /// Where the markup being drawn right now can paint.
    draft_rect: Option<RectI>,
}

impl Overlay {
    /// Adapt to a surface whose size differs from the captured image (rare:
    /// fractional scaling on some compositors, or a window that is still
    /// settling into full screen). The capture itself is never degraded: the
    /// original pixels are kept and every size is resampled from them, so the
    /// overlay is pixel-exact again as soon as the sizes match.
    fn ensure_size(&mut self, w: u32, h: u32) {
        if (w, h) == (self.w, self.h) {
            return;
        }
        // Resampling starts from the capture as taken, not an isolated window.
        self.restore_patch();
        let (src, sw, sh) = match self.original.take() {
            Some(original) => original,
            None => (std::mem::take(&mut self.px), self.w, self.h),
        };
        if (w, h) == (sw, sh) {
            self.px = src;
        } else {
            let mut px = Vec::with_capacity((w * h) as usize);
            for y in 0..h {
                let sy = (y as u64 * sh as u64 / h as u64) as u32;
                let row = (sy * sw) as usize;
                for x in 0..w {
                    let sx = (x as u64 * sw as u64 / w as u64) as usize;
                    px.push(src[row + sx]);
                }
            }
            self.px = px;
            self.original = Some((src, sw, sh));
        }
        let (kx, ky) = (w as f32 / sw as f32, h as f32 / sh as f32);
        let scale = |r: RectI| {
            RectF::new(
                r.x as f32 * kx,
                r.y as f32 * ky,
                r.w as f32 * kx,
                r.h as f32 * ky,
            )
            .round()
        };
        self.windows = self
            .captured_windows
            .iter()
            .map(|win| Win {
                rect: scale(win.rect),
                full: scale(win.full),
                ..*win
            })
            .collect();
        self.w = w;
        self.h = h;
        self.prev = None;
        self.layer = None;
        self.clear_selection();
    }

    /// Everything the frame depends on, computed once per frame.
    fn frame_info(&self) -> FrameInfo {
        let bars = self.bars();
        let over_bar = bars.contains(self.cursor);
        let full = RectI::new(0, 0, self.w as i32, self.h as i32);
        let idle = matches!(self.drag, Drag::None | Drag::Button(_));
        let dragging_rect = match self.drag {
            Drag::NewRect { start } => Some(
                RectF::from_points(start, self.cursor)
                    .round()
                    .intersect(&full),
            ),
            _ => None,
        };
        let hover_target = (self.sel.is_none() && idle && self.cursor_inside && !over_bar)
            .then(|| match self.ui.mode {
                Mode::Window => Some(self.hover_window.unwrap_or(full)),
                Mode::Fullscreen => Some(full),
                _ => None,
            })
            .flatten();
        let sel_rect = self
            .sel
            .as_ref()
            .map(|sel| sel.rect.round().intersect(&full));
        let bright = sel_rect
            .or(dragging_rect)
            .or(hover_target)
            .filter(|r| !r.is_empty());
        let masked = bright.is_some()
            && self
                .sel
                .as_ref()
                .and_then(|s| s.mask.as_ref())
                .is_some_and(|m| Some(m.rect) == bright);
        let draft_rect = match (&self.draft, bright) {
            (Some(d), Some(b)) => {
                let r = d
                    .bounds()
                    .inflate(d.width * 2.0 + 12.0 * self.scale)
                    .round_out()
                    .intersect(&b);
                (!r.is_empty()).then_some(r)
            }
            _ => None,
        };
        FrameInfo {
            bars,
            over_bar,
            full,
            idle,
            dragging_rect,
            hover_target,
            sel_rect,
            bright,
            masked,
            draft_rect,
        }
    }

    /// Keep the selection with its finished markup rendered in one buffer,
    /// so frames copy it instead of redrawing every annotation.
    fn update_layer(&mut self, f: &FrameInfo) {
        let rect = match (self.sel.is_some(), f.bright) {
            (true, Some(b)) if !self.annotations.is_empty() => b,
            _ => {
                self.layer = None;
                return;
            }
        };
        if self
            .layer
            .as_ref()
            .is_some_and(|l| l.rect == rect && l.version == self.ann_version)
        {
            return;
        }
        let mut px = self.layer.take().map(|l| l.px).unwrap_or_default();
        px.clear();
        px.reserve((rect.w * rect.h) as usize);
        let w = self.w as usize;
        for y in rect.y..rect.bottom() {
            let o = y as usize * w;
            px.extend_from_slice(&self.px[o + rect.x as usize..o + rect.right() as usize]);
        }
        {
            let src = Source {
                px: &self.px,
                w: self.w,
                h: self.h,
            };
            let mut c = Canvas::new(&mut px, rect.w as u32, rect.h as u32)
                .with_offset(-rect.x as f32, -rect.y as f32);
            for a in &self.annotations {
                a.draw(&mut c, &src, self.scale);
            }
        }
        self.layer = Some(Layer {
            rect,
            version: self.ann_version,
            px,
        });
    }

    /// Paint the screenshot part of the frame (bright inside the selection,
    /// dimmed elsewhere, markup on top) for `region` into `dst`, whose first
    /// pixel is the region's top-left corner and whose rows are `stride`
    /// pixels apart.
    fn paint_base(&self, dst: &mut [u32], stride: usize, region: RectI, f: &FrameInfo) {
        let w = self.w as usize;
        let at = |y: i32, x: i32| (y - region.y) as usize * stride + (x - region.x) as usize;
        let inside = f
            .bright
            .map(|b| b.intersect(&region))
            .filter(|b| !b.is_empty());
        if let Some(b) = inside {
            let layer = self.layer.as_ref().filter(|l| Some(l.rect) == f.bright);
            let n = b.w as usize;
            for y in b.y..b.bottom() {
                let d = at(y, b.x);
                match layer {
                    Some(l) => {
                        let o =
                            (y - l.rect.y) as usize * l.rect.w as usize + (b.x - l.rect.x) as usize;
                        dst[d..d + n].copy_from_slice(&l.px[o..o + n]);
                    }
                    None => {
                        let o = y as usize * w + b.x as usize;
                        dst[d..d + n].copy_from_slice(&self.px[o..o + n]);
                    }
                }
            }
            if let (Some(draft), Some(dr)) = (&self.draft, f.draft_rect) {
                if !dr.intersect(&region).is_empty() {
                    let src = Source {
                        px: &self.px,
                        w: self.w,
                        h: self.h,
                    };
                    let rows = region.h as usize;
                    let mut c = Canvas::new(&mut dst[..stride * rows], stride as u32, rows as u32)
                        .with_offset(-region.x as f32, -region.y as f32);
                    draft.draw(&mut c, &src, self.scale);
                }
            }
        }
        // Dim everything outside the bright rect; this also clips the draft.
        let (x0, x1) = (region.x, region.right());
        for y in region.y..region.bottom() {
            let s = y as usize * w;
            let d = at(y, x0);
            let mut dim_span = |a: i32, b: i32| {
                if a < b {
                    let n = (b - a) as usize;
                    let (da, sa) = (d + (a - x0) as usize, s + a as usize);
                    dim_row(&mut dst[da..da + n], &self.px[sa..sa + n]);
                }
            };
            match f.bright {
                Some(b) if y >= b.y && y < b.bottom() => {
                    dim_span(x0, b.x.clamp(x0, x1));
                    dim_span(b.right().clamp(x0, x1), x1);
                }
                _ => dim_span(x0, x1),
            }
        }
        if f.masked {
            if let Some(m) = self.sel.as_ref().and_then(|s| s.mask.as_ref()) {
                let r = m.rect.intersect(&region);
                for y in r.y..r.bottom() {
                    let mrow = (y - m.rect.y) as usize * m.rect.w as usize;
                    for x in r.x..r.right() {
                        let cov = m.a[mrow + (x - m.rect.x) as usize];
                        if cov != 255 {
                            let dimmed = dim(self.px[y as usize * w + x as usize]);
                            let px = &mut dst[at(y, x)];
                            *px = if cov == 0 {
                                dimmed
                            } else {
                                mix(dimmed, *px, cov as u32)
                            };
                        }
                    }
                }
            }
        }
    }

    /// Render the whole frame into `buf`.
    pub fn render(&mut self, buf: &mut [u32], w: u32, h: u32) {
        self.render_frame(buf, w, h, false);
    }

    /// Render a frame. With `incremental`, `buf` must still hold the frame
    /// this overlay rendered last; only what changed since is repainted and
    /// the changed rects are returned (to present just those). `None` means
    /// the whole buffer was repainted.
    pub fn render_frame(
        &mut self,
        buf: &mut [u32],
        w: u32,
        h: u32,
        incremental: bool,
    ) -> Option<Vec<RectI>> {
        if w == 0 || h == 0 || buf.len() != (w * h) as usize {
            return None;
        }
        self.ensure_size(w, h);
        let f = self.frame_info();
        self.update_layer(&f);

        // Where the toolbars, loupe, labels and menu land this frame.
        let mut chrome = Vec::new();
        self.draw_chrome(&mut Canvas::measure(w, h, &mut chrome), &f);
        let chrome = merge_rects(&chrome, f.full, 48);

        let prev = self.prev.take().filter(|p| incremental && p.size == (w, h));
        let damage = prev.map(|p| {
            let mut d: Vec<RectI> = Vec::new();
            d.extend(&p.chrome);
            d.extend(&chrome);
            if p.version != self.ann_version {
                d.extend(p.bright);
                d.extend(f.bright);
            } else if p.bright != f.bright || p.masked != f.masked {
                match (p.bright, f.bright) {
                    // The overlap of both looks the same in both frames.
                    (Some(a), Some(b)) if !p.masked && !f.masked => {
                        d.extend(a.subtract(&b));
                        d.extend(b.subtract(&a));
                    }
                    (a, b) => {
                        d.extend(a);
                        d.extend(b);
                    }
                }
            }
            d.extend(p.draft);
            d.extend(f.draft_rect);
            merge_rects(&d, f.full, 12)
        });

        match &damage {
            None => self.paint_base(buf, w as usize, f.full, &f),
            Some(rects) => {
                let mut tmp = std::mem::take(&mut self.scratch);
                for r in rects {
                    let n = (r.w * r.h) as usize;
                    tmp.clear();
                    tmp.resize(n, 0);
                    self.paint_base(&mut tmp, r.w as usize, *r, &f);
                    for (i, y) in (r.y..r.bottom()).enumerate() {
                        let o = y as usize * w as usize + r.x as usize;
                        let rw = r.w as usize;
                        buf[o..o + rw].copy_from_slice(&tmp[i * rw..(i + 1) * rw]);
                    }
                }
                self.scratch = tmp;
            }
        }
        // Every chrome pixel lies inside the damage, which was just
        // restored to the plain screenshot, so drawing it over is exact.
        self.draw_chrome(&mut Canvas::new(buf, w, h), &f);
        self.prev = Some(PrevFrame {
            size: (w, h),
            bright: f.bright,
            masked: f.masked,
            version: self.ann_version,
            draft: f.draft_rect,
            chrome,
        });
        damage
    }

    /// Selection outlines, toolbars, loupe, labels and menus: everything
    /// drawn on top of the screenshot.
    fn draw_chrome(&self, c: &mut Canvas, f: &FrameInfo) {
        let s = self.scale;
        let bars = &f.bars;
        let (sel_rect, dragging_rect) = (f.sel_rect, f.dragging_rect);
        let mask = self.sel.as_ref().and_then(|sel| sel.mask.as_ref());
        let idle = f.idle;

        // --- selection chrome -----------------------------------------------
        if let Some(r) = f
            .hover_target
            .filter(|_| sel_rect.is_none() && dragging_rect.is_none())
        {
            let inset = r.to_f().inflate(-s);
            c.stroke_rrect(inset, 0.0, 2.0 * s, theme::ACCENT);
        }
        if let Some(r) = sel_rect.or(dragging_rect).filter(|r| !r.is_empty()) {
            let rf = r.to_f();
            match mask {
                Some(m) => self.draw_lasso(c, &m.poly, true),
                None => {
                    c.stroke_rrect(rf.inflate(0.5 * s), 0.0, 1.0 * s, Color::rgba(0, 0, 0, 90));
                    c.stroke_rrect(rf.inflate(0.75 * s), 0.0, 1.5 * s, theme::ACCENT);
                }
            }
            if self.adjustable() && !matches!(self.drag, Drag::Move { .. }) {
                for (_, hp) in Self::handle_points(rf) {
                    c.fill_circle(hp, 4.6 * s, theme::ACCENT);
                    c.fill_circle(hp, 3.0 * s, Color::WHITE);
                }
            }
            self.draw_size_label(c, rf, bars.above);
        }
        if let Drag::Lasso { pts } = &self.drag {
            self.draw_lasso(c, pts, false);
        }
        // --- text being typed -------------------------------------------------
        if let Some(edit) = &self.text_edit {
            let em = self.text_em();
            let (tw, th) = edit_size(edit, em);
            let src = Source {
                px: &self.px,
                w: self.w,
                h: self.h,
            };
            Annotation {
                shape: Shape::Text {
                    pos: edit.pos,
                    text: edit.text.clone(),
                    size: em,
                },
                color: self.color,
                width: 0.0,
            }
            .draw(c, &src, s);
            let frame = RectF::new(
                edit.pos.x - 5.0 * s,
                edit.pos.y - 3.0 * s,
                tw.max(6.0 * s) + 10.0 * s,
                th + 6.0 * s,
            );
            if let Some(p) = crate::gfx::rrect_path(frame, 3.0 * s) {
                let stroke = Stroke {
                    width: 1.0 * s,
                    line_cap: LineCap::Butt,
                    line_join: LineJoin::Round,
                    dash: StrokeDash::new(vec![4.0 * s, 3.0 * s], 0.0),
                    ..Stroke::default()
                };
                c.stroke_path_ex(
                    &p,
                    &stroke,
                    Color::rgba(255, 255, 255, 200),
                    BlendMode::SourceOver,
                    Transform::identity(),
                );
            }
            let m = text::ui().metrics(em);
            let lines: Vec<&str> = edit.text.split('\n').collect();
            let last = lines.last().copied().unwrap_or("");
            let cx = edit.pos.x + text::ui().measure(last, em) + 1.5 * s;
            let cy = edit.pos.y + (lines.len() as f32 - 1.0) * m.line_height;
            c.line(
                Pt::new(cx, cy + 2.0 * s),
                Pt::new(cx, cy + m.line_height - 2.0 * s),
                1.6 * s,
                self.color,
            );
        }

        // --- toolbars ---------------------------------------------------------
        for bar in bars.iter() {
            self.draw_bar(c, bar);
        }
        if idle && self.menu.is_none() {
            if let Some(btn) = self.hover_btn {
                self.draw_tooltip(c, bars, btn);
            }
        }

        // --- helpers ----------------------------------------------------------
        let picking = self.tool == Tool::Pipette
            && self
                .sel
                .as_ref()
                .is_some_and(|sel| sel.rect.contains(self.cursor));
        let selecting = self.sel.is_none()
            && self.ui.mode == Mode::Rect
            && matches!(self.drag, Drag::None | Drag::NewRect { .. });
        let resizing = matches!(self.drag, Drag::Resize { .. });
        if self.cursor_inside
            && !f.over_bar
            && self.menu.is_none()
            && (picking || resizing || (selecting && self.opts.loupe))
        {
            self.draw_loupe(c);
        }
        if self.sel.is_none() && matches!(self.drag, Drag::None) {
            let hint = match (self.ui.kind, self.ui.mode) {
                (Kind::Record, Mode::Window) => "Click the window to record",
                (Kind::Record, Mode::Fullscreen) => "Click to record this screen",
                (Kind::Record, _) => "Drag to choose the area to record",
                (_, Mode::Rect) => "Drag to select an area",
                (_, Mode::Freeform) => "Draw around what you want to capture",
                (_, Mode::Window) => "Click a window",
                (_, Mode::Fullscreen) => "Click to capture this screen",
            };
            self.draw_pill(
                c,
                &format!("{hint}   \u{00B7}   Esc to cancel"),
                self.h as f32 - 64.0 * s,
                false,
            );
        }
        if let Some((msg, _)) = &self.flash {
            self.draw_pill(c, msg, 70.0 * s, true);
        }
        self.draw_menu(c);
    }

    fn draw_lasso(&self, c: &mut Canvas, pts: &[Pt], closed: bool) {
        if pts.len() < 2 {
            return;
        }
        let mut pb = PathBuilder::new();
        pb.move_to(pts[0].x, pts[0].y);
        for p in &pts[1..] {
            pb.line_to(p.x, p.y);
        }
        if closed {
            pb.close();
        }
        if let Some(path) = pb.finish() {
            c.stroke_path(&path, 3.0 * self.scale, Color::rgba(0, 0, 0, 70));
            c.stroke_path(&path, 1.6 * self.scale, theme::ACCENT);
        }
    }

    fn draw_size_label(&self, c: &mut Canvas, sel: RectF, bar_above: bool) {
        let s = self.scale;
        let label = format!("{} \u{00D7} {}", sel.w.round() as i32, sel.h.round() as i32);
        let em = theme::FONT * s;
        let tw = text::ui().measure(&label, em);
        let (pw, ph) = (tw + 14.0 * s, 22.0 * s);
        let outside_y = sel.y - ph - 7.0 * s;
        let y = if outside_y < 4.0 * s || bar_above {
            sel.y + 7.0 * s
        } else {
            outside_y
        };
        let x = (sel.x + if y > sel.y { 7.0 * s } else { 0.0 })
            .clamp(4.0 * s, (self.w as f32 - pw - 4.0 * s).max(4.0 * s));
        let r = RectF::new(x.round(), y.round(), pw, ph);
        c.fill_rrect(r, 6.0 * s, theme::PANEL);
        text::ui().draw_centered(c, &label, r.x + r.w / 2.0, r.y + r.h / 2.0, em, theme::TEXT);
    }

    fn draw_pill(&self, c: &mut Canvas, msg: &str, y: f32, accent: bool) {
        let s = self.scale;
        let em = theme::FONT * s;
        let tw = text::ui().measure(msg, em);
        let (pw, ph) = (tw + 28.0 * s, 30.0 * s);
        let r = RectF::new(((self.w as f32 - pw) / 2.0).round(), y.round(), pw, ph);
        c.shadow(r, ph / 2.0, 10.0 * s, 0.45);
        c.fill_rrect(
            r,
            ph / 2.0,
            if accent { theme::ACCENT } else { theme::PANEL },
        );

        let color = if accent {
            Color::WHITE
        } else {
            theme::TEXT_DIM
        };
        text::ui().draw_centered(c, msg, r.x + r.w / 2.0, r.y + r.h / 2.0, em, color);
    }

    fn draw_bar(&self, c: &mut Canvas, bar: &Bar) {
        let s = self.scale;
        c.panel(bar.rect, theme::BAR_RADIUS * s, s);
        for x in &bar.seps {
            let (y0, y1) = (bar.rect.y + 11.0 * s, bar.rect.bottom() - 11.0 * s);
            c.fill_rect(
                RectF::new(x.round(), y0, 1.0f32.max(s.round()), y1 - y0),
                theme::SEPARATOR,
            );
        }
        for slot in &bar.slots {
            self.draw_slot(c, slot.btn, slot.rect);
        }
    }

    fn draw_slot(&self, c: &mut Canvas, btn: Btn, r: RectF) {
        let s = self.scale;
        let hovered =
            self.hover_btn == Some(btn) && matches!(self.drag, Drag::None | Drag::Button(_));
        let pressed = hovered && matches!(self.drag, Drag::Button(b) if b == btn);
        let active = self.btn_active(btn);
        let enabled = self.btn_enabled(btn);
        let center = r.center();
        let radius = theme::BTN_RADIUS * s;

        match btn {
            Btn::Color(i) => {
                let col = theme::PALETTE[i as usize];
                if active {
                    c.stroke_circle(center, 10.0 * s, 1.8 * s, Color::WHITE);
                } else if hovered {
                    c.stroke_circle(center, 10.0 * s, 1.5 * s, Color::rgba(255, 255, 255, 90));
                }
                c.fill_circle(center, 7.0 * s, col);
                if col.luma() < 60.0 {
                    c.stroke_circle(center, 7.0 * s, 1.0 * s, Color::rgba(255, 255, 255, 80));
                }
                return;
            }
            Btn::Width(i) => {
                if active {
                    c.fill_rrect(r, radius, theme::PRESSED);
                } else if hovered {
                    c.fill_rrect(r, radius, theme::HOVER);
                }
                let dot = (2.0 + i as f32 * 1.7) * s;
                c.fill_circle(center, dot, if active { Color::WHITE } else { theme::ICON });
                return;
            }
            _ => {}
        }

        let primary = matches!(btn, Btn::Copy | Btn::Record);
        let fill = if primary {
            let base = if btn == Btn::Record {
                theme::DANGER
            } else {
                theme::ACCENT
            };
            Some(if pressed {
                theme::ACCENT_DEEP
            } else if hovered && btn == Btn::Copy {
                theme::ACCENT_HOVER
            } else {
                base
            })
        } else if active {
            Some(theme::ACCENT)
        } else if pressed {
            Some(theme::PRESSED)
        } else if hovered && enabled {
            Some(theme::HOVER)
        } else {
            None
        };
        if let Some(f) = fill {
            c.fill_rrect(r, radius, f);
        }
        let ink = if !enabled {
            theme::ICON_DISABLED
        } else if primary || active {
            Color::WHITE
        } else {
            theme::ICON
        };

        let label = self.btn_label(btn);
        let icon_x = if label.is_some() {
            r.x + theme::BTN * s / 2.0
        } else {
            center.x
        };
        if let Some(icon) = self.btn_icon(btn) {
            icons::draw(
                c,
                icon,
                Pt::new(icon_x, center.y),
                theme::ICON_SIZE * s,
                ink,
            );
        }
        if let Some(label) = label {
            let em = theme::FONT * s;
            let x = r.x + theme::BTN * s - 2.0 * s;
            text::ui().draw(
                c,
                &label,
                x,
                text::ui().baseline_for_center(center.y, em),
                em,
                ink,
            );
        }
    }

    fn draw_tooltip(&self, c: &mut Canvas, bars: &Bars, btn: Btn) {
        let s = self.scale;
        let Some((bar, slot)) = bars
            .iter()
            .find_map(|b| b.slots.iter().find(|sl| sl.btn == btn).map(|sl| (b, sl)))
        else {
            return;
        };
        let tip = self.btn_tip(btn);
        let em = 12.0 * s;
        let face = text::ui();
        // "Label   Keys": the keys are drawn as keycaps after a dot.
        let (label, keys): (&str, Vec<&str>) = match tip.split_once("   ") {
            Some((l, k)) if !k.starts_with('(') => (l, k.split('+').collect()),
            _ => (tip.as_str(), Vec::new()),
        };
        let key_em = 11.0 * s;
        let cap_h = 18.0 * s;
        let cap_w = |k: &str| (face.measure(k, key_em) + 10.0 * s).max(cap_h);
        let dot_w = 14.0 * s;
        let keys_w: f32 = if keys.is_empty() {
            0.0
        } else {
            dot_w + keys.iter().map(|k| cap_w(k)).sum::<f32>() + 3.0 * s * (keys.len() - 1) as f32
        };
        let tw = face.measure(label, em) + keys_w;
        let (pw, ph) = (tw + 18.0 * s, 28.0 * s);
        let gap = 7.0 * s;

        // Keep the tooltip off the neighbouring bar.
        let other = bars.iter().find(|b| !std::ptr::eq(*b, bar));
        let below_blocked = other.is_some_and(|o| o.rect.y > bar.rect.y);
        let above_blocked = other.is_some_and(|o| o.rect.y < bar.rect.y);
        let below_y = bar.rect.bottom() + gap;
        let above_y = bar.rect.y - gap - ph;
        let y = if (below_blocked || below_y + ph > self.h as f32 - 4.0 * s)
            && !above_blocked
            && above_y >= 4.0 * s
        {
            above_y
        } else {
            below_y
        };
        let x = (slot.rect.center().x - pw / 2.0)
            .clamp(4.0 * s, (self.w as f32 - pw - 4.0 * s).max(4.0 * s));
        let r = RectF::new(x.round(), y.round(), pw, ph);
        c.fill_rrect(r, 8.0 * s, Color::rgb(0x0F, 0x10, 0x13));
        let cy = r.y + r.h / 2.0;
        let mut x = r.x + 9.0 * s;
        x += face.draw(
            c,
            label,
            x,
            face.baseline_for_center(cy, em),
            em,
            theme::TEXT,
        );
        if !keys.is_empty() {
            c.fill_circle(Pt::new(x + dot_w / 2.0, cy), 1.6 * s, theme::TEXT_DIM);
            x += dot_w;
            for k in &keys {
                let w = cap_w(k);
                let cap = RectF::new(x, cy - cap_h / 2.0, w, cap_h);
                c.fill_rrect(cap, 5.0 * s, Color::rgba(255, 255, 255, 30));
                face.draw_centered(c, k, cap.x + w / 2.0, cy, key_em, theme::TEXT);
                x += w + 3.0 * s;
            }
        }
    }

    /// Pixel magnifier with the colour and position under the pointer.
    fn draw_loupe(&self, c: &mut Canvas) {
        const CELLS: i32 = 13;
        let s = self.scale;
        let p = self.cursor;
        let (px, py) = (p.x.floor() as i32, p.y.floor() as i32);
        let cell = (9.0 * s).round().max(4.0);
        let side = CELLS as f32 * cell;
        let pad = (5.0 * s).round();
        let label_h = (40.0 * s).round();
        let (pw, ph) = (side + 2.0 * pad, side + pad + label_h);
        let off = 22.0 * s;
        let mut x = p.x + off;
        let mut y = p.y + off;
        if x + pw > self.w as f32 - 4.0 {
            x = p.x - off - pw;
        }
        if y + ph > self.h as f32 - 4.0 {
            y = p.y - off - ph;
        }
        let panel = RectF::new(x.round().max(2.0), y.round().max(2.0), pw, ph);
        c.panel(panel, 9.0 * s, s);

        let (gx, gy) = (panel.x + pad, panel.y + pad);
        let half = CELLS / 2;
        let mut center_color = Color::BLACK;
        for j in 0..CELLS {
            for i in 0..CELLS {
                let (sx, sy) = (px + i - half, py + j - half);
                let inside = sx >= 0 && sy >= 0 && sx < self.w as i32 && sy < self.h as i32;
                let v = if inside {
                    self.px[(sy as u32 * self.w + sx as u32) as usize]
                } else {
                    0xFF10_1114
                };
                if i == half && j == half {
                    center_color = Color::from_u32(v);
                }
                let cell_rect = RectI::new(
                    (gx + i as f32 * cell) as i32,
                    (gy + j as f32 * cell) as i32,
                    cell as i32,
                    cell as i32,
                );
                c.fill_rect_px(cell_rect, v);
            }
        }
        // Crosshair cell.
        let cr = RectF::new(gx + half as f32 * cell, gy + half as f32 * cell, cell, cell);
        c.stroke_rrect(cr.inflate(1.0), 0.0, 1.0, Color::BLACK);
        c.stroke_rrect(cr.inflate(0.0), 0.0, 1.0, Color::WHITE);

        // Two lines: colour, then position.
        let em = 11.5 * s;
        let line1 = gy + side + label_h * 0.30;
        let line2 = gy + side + label_h * 0.70;
        let sw = 10.0 * s;
        let sw_rect = RectF::new(gx + 1.0 * s, line1 - sw / 2.0, sw, sw);
        c.fill_rrect(sw_rect, 2.5 * s, center_color);
        c.stroke_rrect(sw_rect, 2.5 * s, 1.0, theme::PANEL_BORDER);
        let hex = center_color.to_hex_string();
        text::ui().draw(
            c,
            &hex,
            gx + sw + 7.0 * s,
            text::ui().baseline_for_center(line1, em),
            em,
            theme::TEXT,
        );
        let pos = format!("{px}, {py} px");
        text::ui().draw(
            c,
            &pos,
            gx + 1.0 * s,
            text::ui().baseline_for_center(line2, em),
            em,
            theme::TEXT_DIM,
        );
    }
}
