//! The right-click menu of the overlay.

use super::{Finish, Kind, Outcome, Overlay, Win};
use crate::geom::{Pt, RectF};
use crate::gfx::icons::{self, Icon};
use crate::gfx::{text, theme, Canvas, Color};
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Item {
    Copy,
    SaveAs,
    ExtractText,
    OpenExternal,
    StartRecording,
    Pin,
    Redact,
    Frame,
    CopyColor,
    CopyColorRgb,
    CopyColorHsl,
    SnipWindow,
    SnipScreen,
    Undo,
    StartOver,
    Cancel,
}

pub(super) struct Menu {
    /// Where it was opened (local pixels).
    pub at: Pt,
    pub items: Vec<Option<Item>>,
    pub hover: Option<usize>,
    /// Pixel under the pointer when the menu opened.
    pub color: Color,
    /// Window under the pointer when the menu opened.
    pub window: Option<Win>,
    /// The button that opened it: the menu hangs off its edges.
    pub anchor: Option<RectF>,
}

const ITEM_H: f32 = 30.0;
const SEP_H: f32 = 9.0;
const PAD: f32 = 5.0;

impl Overlay {
    /// Right click: open the menu for whatever is under the pointer.
    pub(super) fn open_menu(&mut self) {
        self.commit_text();
        let at = self.cursor;
        let (x, y) = (at.x as i32, at.y as i32);
        let color = if x >= 0 && y >= 0 && x < self.w as i32 && y < self.h as i32 {
            Color::from_u32(self.px[(y as u32 * self.w + x as u32) as usize])
        } else {
            Color::BLACK
        };
        let window = self
            .window_at(at)
            .filter(|w| self.selection() != Some(w.rect));
        let mut items: Vec<Option<Item>> = Vec::new();
        let has_sel = self.sel.is_some();
        match (has_sel, self.ui.kind) {
            (true, Kind::Snip) => {
                items.extend([
                    Some(Item::Copy),
                    Some(Item::SaveAs),
                    Some(Item::Pin),
                    Some(Item::ExtractText),
                    Some(Item::Redact),
                    Some(Item::OpenExternal),
                    None,
                ]);
            }
            (true, Kind::Record) => items.extend([Some(Item::StartRecording), None]),
            _ => {}
        }
        if window.is_some() {
            items.push(Some(Item::SnipWindow));
        }
        if !has_sel {
            items.push(Some(Item::SnipScreen));
        }
        items.push(Some(Item::CopyColor));
        items.push(Some(Item::CopyColorRgb));
        items.push(Some(Item::CopyColorHsl));
        items.push(None);
        if has_sel && !self.undo.is_empty() {
            items.push(Some(Item::Undo));
        }
        if has_sel {
            items.push(Some(Item::StartOver));
        }
        items.push(Some(Item::Cancel));
        self.menu = Some(Menu {
            at,
            items,
            hover: None,
            color,
            window,
            anchor: None,
        });
        self.hover_btn = None;
        self.dirty = true;
    }

    /// The menu behind the "more" button, opened just off `anchor`.
    pub(super) fn open_more_menu(&mut self, anchor: RectF) {
        self.commit_text();
        self.menu = Some(Menu {
            at: Pt::new(anchor.x, anchor.bottom()),
            items: vec![
                Some(Item::SaveAs),
                Some(Item::Pin),
                Some(Item::OpenExternal),
                None,
                Some(Item::ExtractText),
                Some(Item::Redact),
                Some(Item::Frame),
            ],
            hover: None,
            color: Color::BLACK,
            window: None,
            anchor: Some(anchor),
        });
        self.hover_btn = None;
        self.dirty = true;
    }

    fn label(&self, menu: &Menu, item: Item) -> String {
        let has_sel = self.sel.is_some();
        match item {
            Item::Copy => "Copy".into(),
            Item::SaveAs => "Save as...".into(),
            Item::ExtractText => "Copy text / QR code".into(),
            Item::OpenExternal => "Open in default app".into(),
            Item::StartRecording => "Start recording".into(),
            Item::Pin => "Pin to screen".into(),
            Item::Redact => "Hide e-mails, numbers and keys".into(),
            Item::Frame if self.opts.frame.is_some() => "Remove the background".into(),
            Item::Frame => "Put on a background".into(),
            Item::CopyColor => format!("Copy colour {}", menu.color.to_hex_string()),
            Item::CopyColorRgb => format!("Copy {}", menu.color.to_rgb_string()),
            Item::CopyColorHsl => format!("Copy {}", menu.color.to_hsl_string()),
            Item::SnipWindow if has_sel => "Select this window instead".into(),
            Item::SnipWindow if self.ui.kind == Kind::Record => "Record this window".into(),
            Item::SnipWindow => "Snip this window".into(),
            Item::SnipScreen if self.ui.kind == Kind::Record => "Record the whole screen".into(),
            Item::SnipScreen => "Snip the whole screen".into(),
            Item::Undo => "Undo".into(),
            Item::StartOver => "Select again".into(),
            Item::Cancel => "Close".into(),
        }
    }

    fn icon(item: Item) -> Icon {
        match item {
            Item::Copy => Icon::Copy,
            Item::SaveAs => Icon::Save,
            Item::ExtractText => Icon::TextScan,
            Item::OpenExternal => Icon::External,
            Item::StartRecording => Icon::Video,
            Item::Pin => Icon::Pin,
            Item::Redact => Icon::Redact,
            Item::Frame => Icon::Frame,
            Item::CopyColor | Item::CopyColorRgb | Item::CopyColorHsl => Icon::Pipette,
            Item::SnipWindow => Icon::ModeWindow,
            Item::SnipScreen => Icon::ModeFull,
            Item::Undo => Icon::Undo,
            Item::StartOver => Icon::ModeRect,
            Item::Cancel => Icon::Close,
        }
    }

    fn keys(item: Item) -> &'static [&'static str] {
        let ctrl = if cfg!(target_os = "macos") {
            "Cmd"
        } else {
            "Ctrl"
        };
        match item {
            Item::Copy | Item::StartRecording => &["Enter"],
            Item::SaveAs => {
                if ctrl == "Cmd" {
                    &["Cmd", "S"]
                } else {
                    &["Ctrl", "S"]
                }
            }
            Item::Undo => {
                if ctrl == "Cmd" {
                    &["Cmd", "Z"]
                } else {
                    &["Ctrl", "Z"]
                }
            }
            Item::Pin => {
                if ctrl == "Cmd" {
                    &["Cmd", "T"]
                } else {
                    &["Ctrl", "T"]
                }
            }
            Item::Cancel => &["Esc"],
            Item::Frame => &["F"],
            _ => &[],
        }
    }

    /// The menu's rectangle and the rectangle of each entry (None for
    /// separators), kept inside the monitor.
    pub(super) fn menu_layout(&self) -> Option<(RectF, Vec<Option<RectF>>)> {
        let menu = self.menu.as_ref()?;
        let s = self.scale;
        let face = text::ui();
        let em = 12.5 * s;
        let key_em = 11.0 * s;
        let mut width: f32 = 0.0;
        for item in menu.items.iter().flatten() {
            let keys: f32 = Self::keys(*item)
                .iter()
                .map(|k| (face.measure(k, key_em) + 10.0 * s).max(18.0 * s) + 3.0 * s)
                .sum();
            let w = 36.0 * s + face.measure(&self.label(menu, *item), em) + 24.0 * s + keys;
            width = width.max(w);
        }
        width = width.max(200.0 * s);
        let height: f32 = 2.0 * PAD * s
            + menu
                .items
                .iter()
                .map(|i| if i.is_some() { ITEM_H } else { SEP_H })
                .sum::<f32>()
                * s;
        let b = self.bounds();
        let margin = 6.0 * s;
        // Open to the right and below; flip when that would leave the screen.
        // A menu opened from a button flips around the button, not the point.
        let a = menu
            .anchor
            .unwrap_or(RectF::new(menu.at.x, menu.at.y, 0.0, 0.0));
        let gap = if menu.anchor.is_some() { 6.0 } else { 2.0 } * s;
        let mut x = a.x + 2.0 * s;
        if x + width > b.right() - margin {
            x = a.right() - width - 2.0 * s;
        }
        let mut y = a.bottom() + gap;
        if y + height > b.bottom() - margin {
            y = a.y - height - gap;
        }
        let x = x.clamp(margin, (b.right() - width - margin).max(margin));
        let y = y.clamp(margin, (b.bottom() - height - margin).max(margin));
        let frame = RectF::new(x.round(), y.round(), width, height);
        let mut rows = Vec::with_capacity(menu.items.len());
        let mut ry = frame.y + PAD * s;
        for item in &menu.items {
            if item.is_some() {
                rows.push(Some(RectF::new(
                    frame.x + PAD * s,
                    ry,
                    width - 2.0 * PAD * s,
                    ITEM_H * s,
                )));
                ry += ITEM_H * s;
            } else {
                rows.push(None);
                ry += SEP_H * s;
            }
        }
        Some((frame, rows))
    }

    pub(super) fn menu_hit(&self, p: Pt) -> Option<usize> {
        let (_, rows) = self.menu_layout()?;
        rows.iter().position(|r| r.is_some_and(|r| r.contains(p)))
    }

    pub(super) fn menu_contains(&self, p: Pt) -> bool {
        self.menu_layout().is_some_and(|(f, _)| f.contains(p))
    }

    /// Carry out the entry at `index` and close the menu.
    pub(super) fn run_menu(&mut self, index: usize) {
        let Some(menu) = self.menu.take() else {
            return;
        };
        self.dirty = true;
        let Some(Some(item)) = menu.items.get(index).copied() else {
            return;
        };
        match item {
            Item::Copy => self.finish(Finish::Copy),
            Item::SaveAs => self.finish(Finish::SaveAs),
            Item::ExtractText => self.finish(Finish::ExtractText),
            Item::OpenExternal => self.finish(Finish::OpenExternal),
            Item::StartRecording => self.confirm(),
            Item::Pin => self.finish(Finish::Pin),
            Item::Redact => self.activate(super::Btn::Redact),
            Item::Frame => self.activate(super::Btn::Frame),
            Item::CopyColor | Item::CopyColorRgb | Item::CopyColorHsl => {
                let text = match item {
                    Item::CopyColorRgb => menu.color.to_rgb_string(),
                    Item::CopyColorHsl => menu.color.to_hsl_string(),
                    _ => menu.color.to_hex_string(),
                };
                self.flash = Some((format!("Copied {text}"), Instant::now()));
                self.outcome = Some(Outcome::CopyText(text));
            }
            Item::SnipWindow => {
                if let Some(w) = menu.window {
                    self.select_window(w);
                }
            }
            Item::SnipScreen => self.select_all(),
            Item::Undo => self.undo(),
            Item::StartOver => self.clear_selection(),
            Item::Cancel => self.outcome = Some(Outcome::Cancel),
        }
    }

    pub(super) fn draw_menu(&self, c: &mut Canvas) {
        let (Some(menu), Some((frame, rows))) = (&self.menu, self.menu_layout()) else {
            return;
        };
        let s = self.scale;
        let face = text::ui();
        let em = 12.5 * s;
        let key_em = 11.0 * s;
        c.panel(frame, 10.0 * s, s);
        for (i, (item, row)) in menu.items.iter().zip(&rows).enumerate() {
            let (Some(item), Some(r)) = (item, row) else {
                // Separator.
                let y = frame.y
                    + PAD * s
                    + menu.items[..i]
                        .iter()
                        .map(|it| if it.is_some() { ITEM_H } else { SEP_H })
                        .sum::<f32>()
                        * s
                    + SEP_H * s / 2.0;
                c.fill_rect(
                    RectF::new(
                        frame.x + 12.0 * s,
                        y.round(),
                        frame.w - 24.0 * s,
                        1.0 * s.max(1.0),
                    ),
                    theme::SEPARATOR,
                );
                continue;
            };
            if menu.hover == Some(i) {
                c.fill_rrect(*r, 7.0 * s, theme::HOVER);
            }
            let cy = r.y + r.h / 2.0;
            let icon_c = Pt::new(r.x + 16.0 * s, cy);
            if matches!(
                item,
                Item::CopyColor | Item::CopyColorRgb | Item::CopyColorHsl
            ) {
                c.fill_rrect(
                    RectF::new(icon_c.x - 7.0 * s, cy - 7.0 * s, 14.0 * s, 14.0 * s),
                    4.0 * s,
                    menu.color,
                );
            } else {
                icons::draw(c, Self::icon(*item), icon_c, 16.0 * s, theme::ICON);
            }
            let label = self.label(menu, *item);
            face.draw(
                c,
                &label,
                r.x + 34.0 * s,
                face.baseline_for_center(cy, em),
                em,
                theme::TEXT,
            );
            // Shortcut keycaps on the right.
            let keys = Self::keys(*item);
            let mut x = r.right() - 8.0 * s;
            for k in keys.iter().rev() {
                let w = (face.measure(k, key_em) + 10.0 * s).max(18.0 * s);
                x -= w;
                let cap = RectF::new(x, cy - 9.0 * s, w, 18.0 * s);
                c.fill_rrect(cap, 5.0 * s, Color::rgba(255, 255, 255, 26));
                face.draw_centered(c, k, cap.x + w / 2.0, cy, key_em, theme::TEXT_DIM);
                x -= 3.0 * s;
            }
        }
    }
}
