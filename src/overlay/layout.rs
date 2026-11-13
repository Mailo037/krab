//! Toolbar geometry: where every button lives for the current state.

use super::{Btn, Drag, Kind, Mode, Overlay};
use crate::annotate::Tool;
use crate::geom::{Pt, RectF};
use crate::gfx::icons::Icon;
use crate::gfx::{text, theme};

pub(super) struct Slot {
    pub btn: Btn,
    pub rect: RectF,
}

pub(super) struct Bar {
    pub rect: RectF,
    pub slots: Vec<Slot>,
    /// X positions of separator lines.
    pub seps: Vec<f32>,
}

#[derive(Default)]
pub(super) struct Bars {
    pub main: Option<Bar>,
    /// Second row of the toolbar on screens too narrow for one.
    pub main2: Option<Bar>,
    pub props: Option<Bar>,
    /// The open tool palette (one row, or two on narrow screens).
    pub palette: Vec<Bar>,
    /// The main bar sits above the selection (affects label placement).
    pub above: bool,
}

impl Bars {
    pub fn iter(&self) -> impl Iterator<Item = &Bar> {
        self.main
            .iter()
            .chain(self.main2.iter())
            .chain(self.props.iter())
            .chain(self.palette.iter())
    }

    pub fn hit(&self, p: Pt) -> Option<Btn> {
        self.iter()
            .flat_map(|b| b.slots.iter())
            .find(|s| s.rect.contains(p))
            .map(|s| s.btn)
    }

    pub fn contains(&self, p: Pt) -> bool {
        self.iter().any(|b| b.rect.contains(p))
    }
}

const MODE_BAR: &[Option<Btn>] = &[
    Some(Btn::Kind(Kind::Snip)),
    Some(Btn::Kind(Kind::Record)),
    None,
    Some(Btn::Mode(Mode::Rect)),
    Some(Btn::Mode(Mode::Freeform)),
    Some(Btn::Mode(Mode::Window)),
    Some(Btn::Mode(Mode::Fullscreen)),
    None,
    Some(Btn::Delay),
    None,
    Some(Btn::Cancel),
];

/// The toolbar once an area is selected: markup tools and the save options
/// each sit behind one button.
const TOOL_BAR: &[Option<Btn>] = &[
    Some(Btn::Tools),
    None,
    Some(Btn::Undo),
    Some(Btn::Redo),
    None,
    Some(Btn::More),
    None,
    Some(Btn::Copy),
    Some(Btn::Cancel),
];

/// The markup tools, opened from [`Btn::Tools`].
const PALETTE: &[Option<Btn>] = &[
    Some(Btn::Tool(Tool::Select)),
    Some(Btn::Tool(Tool::Pen)),
    Some(Btn::Tool(Tool::Highlighter)),
    Some(Btn::Tool(Tool::Line)),
    Some(Btn::Tool(Tool::Arrow)),
    Some(Btn::Tool(Tool::Rect)),
    Some(Btn::Tool(Tool::Ellipse)),
    Some(Btn::Tool(Tool::Text)),
    None,
    Some(Btn::Tool(Tool::Callout)),
    Some(Btn::Tool(Tool::Step)),
    Some(Btn::Tool(Tool::Measure)),
    None,
    Some(Btn::Tool(Tool::Pixelate)),
    Some(Btn::Tool(Tool::Blur)),
    Some(Btn::Tool(Tool::Eraser)),
    Some(Btn::Tool(Tool::Pipette)),
];

/// Number of buttons in the tool palette.
#[cfg(test)]
pub(super) fn palette_len() -> usize {
    PALETTE.iter().flatten().count()
}

const PROPS_BAR: &[Option<Btn>] = &[
    Some(Btn::Color(0)),
    Some(Btn::Color(1)),
    Some(Btn::Color(2)),
    Some(Btn::Color(3)),
    Some(Btn::Color(4)),
    Some(Btn::Color(5)),
    Some(Btn::Color(6)),
    Some(Btn::Color(7)),
    None,
    Some(Btn::Width(0)),
    Some(Btn::Width(1)),
    Some(Btn::Width(2)),
];

const RECORD_BAR: &[Option<Btn>] = &[Some(Btn::Record), None, Some(Btn::Cancel)];

const SEP_W: f32 = 9.0;

impl Overlay {
    pub(super) fn btn_icon(&self, btn: Btn) -> Option<Icon> {
        Some(match btn {
            Btn::Kind(Kind::Snip) => Icon::Camera,
            Btn::Kind(Kind::Record) => Icon::Video,
            Btn::Mode(Mode::Rect) => Icon::ModeRect,
            Btn::Mode(Mode::Freeform) => Icon::ModeFreeform,
            Btn::Mode(Mode::Window) => Icon::ModeWindow,
            Btn::Mode(Mode::Fullscreen) => Icon::ModeFull,
            Btn::Delay => Icon::Timer,
            Btn::Cancel => Icon::Close,
            Btn::Tool(t) => match t {
                Tool::Select => Icon::Pointer,
                Tool::Pen => Icon::Pen,
                Tool::Highlighter => Icon::Highlighter,
                Tool::Line => Icon::Line,
                Tool::Arrow => Icon::Arrow,
                Tool::Rect => Icon::Rect,
                Tool::Ellipse => Icon::Ellipse,
                Tool::Text => Icon::Text,
                Tool::Callout => Icon::Callout,
                Tool::Step => Icon::Step,
                Tool::Measure => Icon::Ruler,
                Tool::Blur => Icon::Blur,
                Tool::Pixelate => Icon::Pixelate,
                Tool::Eraser => Icon::Eraser,
                Tool::Pipette => Icon::Pipette,
            },
            Btn::Undo => Icon::Undo,
            Btn::Redo => Icon::Redo,
            Btn::Redact => Icon::Redact,
            Btn::Frame => Icon::Frame,
            Btn::Copy => Icon::Copy,
            Btn::Record => Icon::Dot,
            // The tools button shows the tool in use.
            Btn::Tools => return self.btn_icon(Btn::Tool(self.tool)),
            Btn::More => Icon::More,
            Btn::Color(_) | Btn::Width(_) => return None,
        })
    }

    pub(super) fn btn_label(&self, btn: Btn) -> Option<String> {
        match btn {
            Btn::Delay => Some(format!("{}s", self.ui.delay)),
            Btn::Record => Some("Start recording".into()),
            _ => None,
        }
    }

    pub(super) fn btn_tip(&self, btn: Btn) -> String {
        match btn {
            Btn::Kind(Kind::Snip) => "Screenshot".into(),
            Btn::Kind(Kind::Record) => "Screen recording".into(),
            Btn::Mode(Mode::Rect) => "Rectangle   R".into(),
            Btn::Mode(Mode::Freeform) => "Freeform   F".into(),
            Btn::Mode(Mode::Window) => "Window   W".into(),
            Btn::Mode(Mode::Fullscreen) => "Full screen   M".into(),
            Btn::Delay => format!("Snip in {} s", self.ui.delay),
            Btn::Cancel => "Close   Esc".into(),
            Btn::Tool(t) => match t {
                Tool::Select => "Move / resize   V",
                Tool::Pen => "Pen   P",
                Tool::Highlighter => "Highlighter   H",
                Tool::Line => "Line   L",
                Tool::Arrow => "Arrow   A",
                Tool::Rect => "Rectangle   R",
                Tool::Ellipse => "Ellipse   E",
                Tool::Text => "Text   T",
                Tool::Callout => "Callout   K",
                Tool::Step => "Step number   N",
                Tool::Measure => "Measure   M",
                Tool::Blur => "Blur   G",
                Tool::Pixelate => "Pixelate   B",
                Tool::Eraser => "Eraser   X",
                Tool::Pipette => "Color picker   I",
            }
            .into(),
            Btn::Undo => "Undo   Ctrl+Z".into(),
            Btn::Redo => "Redo   Ctrl+Y".into(),
            Btn::Redact => "Hide e-mails, numbers and keys".into(),
            Btn::Frame => "Background frame   F".into(),
            Btn::Copy => "Copy   Enter".into(),
            Btn::Record => "Start recording   Enter".into(),
            Btn::Tools => "Markup tools".into(),
            Btn::More => "Save, pin, text and more".into(),
            Btn::Color(i) => theme::PALETTE[i as usize].to_hex_string(),
            Btn::Width(i) => ["Small", "Medium", "Large"][i as usize].into(),
        }
    }

    /// What the mouse can do on a button besides clicking, shown in its
    /// tooltip next to the keys: turning the wheel, a right click.
    pub(super) fn btn_mouse(&self, btn: Btn) -> &'static [Icon] {
        match btn {
            Btn::Delay => &[Icon::MouseWheel, Icon::MouseRight],
            // The wheel changes the width anywhere on the selection.
            Btn::Width(_) => &[Icon::MouseWheel],
            _ => &[],
        }
    }

    fn slot_width(&self, btn: Btn) -> f32 {
        let s = self.scale;
        match btn {
            Btn::Color(_) => 26.0 * s,
            Btn::Width(_) => 30.0 * s,
            // Room for the small arrow next to the tool icon.
            Btn::Tools => (theme::BTN + 12.0) * s,
            Btn::Delay | Btn::Record => {
                let label = self.btn_label(btn).unwrap_or_default();
                let pad = if btn == Btn::Record { 14.0 } else { 6.0 };
                theme::BTN * s + text::ui().measure(&label, theme::FONT * s) + pad * s
            }
            _ => theme::BTN * s,
        }
    }

    fn bar_size(&self, items: &[Option<Btn>], btn_h: f32) -> (f32, f32) {
        let s = self.scale;
        let mut w = theme::BAR_PAD * s * 2.0 - theme::BTN_GAP * s;
        for item in items {
            w += match item {
                Some(b) => self.slot_width(*b) + theme::BTN_GAP * s,
                None => SEP_W * s,
            };
        }
        (w, btn_h + theme::BAR_PAD * s * 2.0)
    }

    fn build_bar(&self, items: &[Option<Btn>], x: f32, y: f32, btn_h: f32) -> Bar {
        let s = self.scale;
        let (w, h) = self.bar_size(items, btn_h);
        let (x, y) = (x.round(), y.round());
        let mut cx = x + theme::BAR_PAD * s;
        let mut slots = Vec::with_capacity(items.len());
        let mut seps = Vec::new();
        for item in items {
            match item {
                Some(b) => {
                    let bw = self.slot_width(*b);
                    slots.push(Slot {
                        btn: *b,
                        rect: RectF::new(cx, y + theme::BAR_PAD * s, bw, btn_h),
                    });
                    cx += bw + theme::BTN_GAP * s;
                }
                None => {
                    seps.push(cx + (SEP_W * s - theme::BTN_GAP * s) / 2.0);
                    cx += SEP_W * s;
                }
            }
        }
        Bar {
            rect: RectF::new(x, y, w, h),
            slots,
            seps,
        }
    }

    /// Compute the bars for the current state.
    pub(super) fn bars(&self) -> Bars {
        let s = self.scale;
        let (sw, sh) = (self.w as f32, self.h as f32);
        let btn_h = theme::BTN * s;

        let Some(sel) = &self.sel else {
            if matches!(self.drag, Drag::NewRect { .. } | Drag::Lasso { .. }) {
                return Bars::default();
            }
            let (w, _) = self.bar_size(MODE_BAR, btn_h);
            let bar = self.build_bar(MODE_BAR, (sw - w) / 2.0, 18.0 * s, btn_h);
            return Bars {
                main: Some(bar),
                main2: None,
                props: None,
                palette: Vec::new(),
                above: false,
            };
        };

        let record = self.ui.kind == Kind::Record;
        let items = if record { RECORD_BAR } else { TOOL_BAR };
        let gap = 8.0 * s;
        let margin = 8.0 * s;
        let row_gap = 4.0 * s;
        // Too wide for the screen: break the toolbar at a separator.
        let rows = self.split_rows(items, btn_h, sw - 2.0 * margin);
        let (mw, mh) = self.bar_size(rows[0], btn_h);
        let block_h = rows.len() as f32 * mh + (rows.len() - 1) as f32 * row_gap;
        let props_h = 26.0 * s;
        let (pw, ph) = self.bar_size(PROPS_BAR, props_h);
        // Always reserve room for the properties bar or the tool palette so
        // the toolbar does not jump when one of them opens.
        let reserve = if record { 0.0 } else { mh + gap * 0.75 };

        let r = sel.rect;
        let below_y = r.bottom() + gap;
        let above_y = r.y - gap - block_h;
        let (my, above, props_below) = if below_y + block_h + reserve <= sh - margin {
            (below_y, false, true)
        } else if above_y - reserve >= margin {
            (above_y, true, false)
        } else {
            // No room outside: tuck the bars inside the bottom edge.
            (
                (r.bottom() - gap - block_h).min(sh - margin - block_h),
                false,
                false,
            )
        };
        let place = |row: &[Option<Btn>], y: f32| {
            let (w, _) = self.bar_size(row, btn_h);
            let x = (r.right() - w).clamp(margin, (sw - w - margin).max(margin));
            self.build_bar(row, x, y, btn_h)
        };
        let main = place(rows[0], my);
        let main2 = rows.get(1).map(|row| place(row, my + mh + row_gap));
        let mx = main.rect.x;

        // The palette and the properties bar share the spot next to the toolbar.
        let mut palette = Vec::new();
        if !record && self.palette {
            let anchor = main
                .slots
                .iter()
                .find(|sl| sl.btn == Btn::Tools)
                .map_or(mx, |sl| sl.rect.x - theme::BAR_PAD * s);
            let prows = self.split_rows(PALETTE, btn_h, sw - 2.0 * margin);
            let n = prows.len() as f32;
            let first_y = if props_below {
                my + block_h + gap * 0.75
            } else {
                my - gap * 0.75 - n * mh - (n - 1.0) * row_gap
            };
            for (i, row) in prows.iter().enumerate() {
                let (w, _) = self.bar_size(row, btn_h);
                let x = anchor.clamp(margin, (sw - w - margin).max(margin));
                let y = (first_y + i as f32 * (mh + row_gap))
                    .clamp(margin, (sh - mh - margin).max(margin));
                palette.push(self.build_bar(row, x, y, btn_h));
            }
        }
        let props = (!record && palette.is_empty() && self.tool.has_style()).then(|| {
            let py = if props_below {
                my + block_h + gap * 0.75
            } else {
                my - gap * 0.75 - ph
            };
            let px = (mx + mw - pw).clamp(margin, (sw - pw - margin).max(margin));
            self.build_bar(PROPS_BAR, px, py, props_h)
        });
        Bars {
            main: Some(main),
            main2,
            props,
            palette,
            above,
        }
    }

    /// One row if `items` fit into `avail`, else two rows broken at the
    /// separator that balances them best.
    fn split_rows<'a>(
        &self,
        items: &'a [Option<Btn>],
        btn_h: f32,
        avail: f32,
    ) -> Vec<&'a [Option<Btn>]> {
        if self.bar_size(items, btn_h).0 <= avail {
            return vec![items];
        }
        let best = items
            .iter()
            .enumerate()
            .filter(|(_, it)| it.is_none())
            .map(|(i, _)| {
                let a = self.bar_size(&items[..i], btn_h).0;
                let b = self.bar_size(&items[i + 1..], btn_h).0;
                (i, a.max(b))
            })
            .min_by(|x, y| x.1.total_cmp(&y.1));
        match best {
            Some((i, _)) => vec![&items[..i], &items[i + 1..]],
            None => vec![items],
        }
    }
}
