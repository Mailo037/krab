//! Generates the icon and every README image.
//!
//! `cargo run --release --example gen_assets`
//!
//! Nothing here is a mock-up of the UI: a synthetic desktop is drawn, then the
//! real `Overlay` is driven with pointer and keyboard events and rendered by
//! the same code that runs on screen.

use krab::annotate::Tool;
use krab::capture::{Shot, WinInfo};
use krab::geom::{Pt, RectF, RectI};
use krab::gfx::{argb_to_rgba, rrect_path, text, theme, Canvas, Color};
use krab::hud::{self, Hud};
use krab::overlay::{Btn, Button, Key, Kind, Mode, Options, Overlay, Shared};
use std::path::Path;
use std::time::{Duration, Instant};

/// Logical desktop size and the scale it is rendered at.
const LW: f32 = 1200.0;
const LH: f32 = 750.0;
const K: f32 = 1.5;
const W: u32 = (LW * K) as u32;
const H: u32 = (LH * K) as u32;

static BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");
static SEMI: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
static REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");

struct Fonts {
    bold: text::Face,
    semi: text::Face,
    regular: text::Face,
}

fn fonts() -> Fonts {
    Fonts {
        bold: text::Face::from_static(BOLD).unwrap(),
        semi: text::Face::from_static(SEMI).unwrap(),
        regular: text::Face::from_static(REGULAR).unwrap(),
    }
}

fn save(path: &str, px: &[u32], w: u32, h: u32) {
    let rgba = argb_to_rgba(px);
    image::save_buffer(Path::new(path), &rgba, w, h, image::ColorType::Rgba8).expect("write png");
    println!("wrote {path} ({w}x{h})");
}

/// Scale a logical rect to device pixels.
fn r(x: f32, y: f32, w: f32, h: f32) -> RectF {
    RectF::new(x * K, y * K, w * K, h * K)
}

fn wallpaper(c: &mut Canvas, w: f32, h: f32) {
    let full = rrect_path(RectF::new(0.0, 0.0, w, h), 0.0).unwrap();
    c.fill_path_gradient(
        &full,
        Pt::new(0.0, 0.0),
        Color::hex(0x101A3A),
        Pt::new(w, h),
        Color::hex(0x3B1D5A),
    );
    for (cx, cy, rad, col) in [
        (0.82, 0.90, 0.55, Color::rgba(0xF4, 0x66, 0x23, 150)),
        (0.10, 0.15, 0.50, Color::rgba(0x22, 0x9E, 0xD9, 110)),
        (0.55, 0.05, 0.40, Color::rgba(0xD9, 0x46, 0xEF, 70)),
        (0.25, 0.95, 0.45, Color::rgba(0x4F, 0x46, 0xE5, 120)),
    ] {
        let rad = rad * w;
        c.fill_path_radial(&full, Pt::new(cx * w, cy * h), rad, col, col.alpha(0.0));
    }
}

fn traffic_lights(c: &mut Canvas, x: f32, y: f32) {
    for (i, col) in [0xFF5F57u32, 0xFEBC2E, 0x28C840].iter().enumerate() {
        c.fill_circle(
            Pt::new((x + i as f32 * 18.0) * K, y * K),
            5.5 * K,
            Color::hex(*col),
        );
    }
}

/// A light "report" window. Returns its bounds (logical).
fn report_window(c: &mut Canvas, f: &Fonts) -> RectF {
    let (x, y, w, h) = (56.0, 52.0, 660.0, 500.0);
    let win = r(x, y, w, h);
    c.shadow(win, 12.0 * K, 30.0 * K, 0.6);
    c.fill_rrect(win, 12.0 * K, Color::hex(0xFFFFFF));
    // Title bar.
    if let Some(p) = rrect_path(r(x, y, w, 42.0), 12.0 * K) {
        c.fill_path(&p, Color::hex(0xEEF0F4));
    }
    c.fill_rect(r(x, y + 28.0, w, 14.0), Color::hex(0xEEF0F4));
    c.fill_rect(r(x, y + 42.0, w, 1.0), Color::hex(0xDADDE3));
    traffic_lights(c, x + 20.0, y + 21.0);
    c.fill_rrect(
        r(x + 150.0, y + 9.0, 360.0, 24.0),
        12.0 * K,
        Color::hex(0xFFFFFF),
    );
    f.regular.draw(
        c,
        "reports.example.com/q3",
        (x + 166.0) * K,
        (y + 26.0) * K,
        12.0 * K,
        Color::hex(0x6B7280),
    );

    // Heading.
    f.bold.draw(
        c,
        "Quarterly revenue",
        (x + 32.0) * K,
        (y + 92.0) * K,
        26.0 * K,
        Color::hex(0x111827),
    );
    f.regular.draw(
        c,
        "Q3 summary - all regions",
        (x + 32.0) * K,
        (y + 116.0) * K,
        13.0 * K,
        Color::hex(0x6B7280),
    );

    // Stat cards.
    for (i, (label, value)) in [
        ("Revenue", "$ 4.82M"),
        ("Customers", "12,480"),
        ("Churn", "1.9 %"),
    ]
    .iter()
    .enumerate()
    {
        let cx = x + 32.0 + i as f32 * 202.0;
        let card = r(cx, y + 138.0, 190.0, 66.0);
        c.fill_rrect(card, 8.0 * K, Color::hex(0xF6F7F9));
        f.regular.draw(
            c,
            label,
            (cx + 14.0) * K,
            (y + 162.0) * K,
            11.5 * K,
            Color::hex(0x6B7280),
        );
        f.semi.draw(
            c,
            value,
            (cx + 14.0) * K,
            (y + 190.0) * K,
            20.0 * K,
            Color::hex(0x111827),
        );
    }

    // Bar chart.
    let (bx, by, bw, bh) = (x + 32.0, y + 226.0, 596.0, 190.0);
    c.fill_rrect(r(bx, by, bw, bh), 8.0 * K, Color::hex(0xF6F7F9));
    for i in 0..4 {
        c.fill_rect(
            r(bx + 16.0, by + 30.0 + i as f32 * 40.0, bw - 32.0, 1.0),
            Color::hex(0xE3E6EB),
        );
    }
    let heights = [58.0, 74.0, 66.0, 92.0, 84.0, 136.0, 110.0, 98.0];
    for (i, hgt) in heights.iter().enumerate() {
        let col = if i == 5 {
            Color::hex(0xF46623)
        } else {
            Color::hex(0x6366F1)
        };
        let bar_x = bx + 40.0 + i as f32 * 68.0;
        c.fill_rrect(r(bar_x, by + bh - 22.0 - hgt, 40.0, *hgt), 5.0 * K, col);
        let label = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug"][i];
        let lw = f.regular.measure(label, 10.5 * K);
        f.regular.draw(
            c,
            label,
            (bar_x + 20.0) * K - lw / 2.0,
            (by + bh - 7.0) * K,
            10.5 * K,
            Color::hex(0x9CA3AF),
        );
    }

    // Footer line with something worth redacting.
    f.regular.draw(
        c,
        "Prepared by",
        (x + 32.0) * K,
        (y + 452.0) * K,
        12.5 * K,
        Color::hex(0x6B7280),
    );
    f.semi.draw(
        c,
        "jane.doe@example.com",
        (x + 112.0) * K,
        (y + 452.0) * K,
        12.5 * K,
        Color::hex(0x111827),
    );
    f.regular.draw(
        c,
        "Confidential - internal use only",
        (x + 32.0) * K,
        (y + 474.0) * K,
        11.5 * K,
        Color::hex(0x9CA3AF),
    );
    RectF::new(x, y, w, h)
}

/// A dark "editor" window with abstract code. Returns its bounds (logical).
fn editor_window(c: &mut Canvas, f: &Fonts) -> RectF {
    let (x, y, w, h) = (742.0, 236.0, 410.0, 400.0);
    let win = r(x, y, w, h);
    c.shadow(win, 12.0 * K, 30.0 * K, 0.6);
    c.fill_rrect(win, 12.0 * K, Color::hex(0x1E1F29));
    if let Some(p) = rrect_path(r(x, y, w, 40.0), 12.0 * K) {
        c.fill_path(&p, Color::hex(0x282A36));
    }
    c.fill_rect(r(x, y + 26.0, w, 14.0), Color::hex(0x282A36));
    traffic_lights(c, x + 20.0, y + 20.0);
    f.regular.draw(
        c,
        "main.rs - krab",
        (x + 172.0) * K,
        (y + 25.0) * K,
        12.0 * K,
        Color::hex(0x9AA0B4),
    );

    // Line numbers and syntax-coloured tokens.
    let palette = [
        0xFF79C6u32,
        0x8BE9FD,
        0x50FA7B,
        0xF1FA8C,
        0xBD93F9,
        0xF8F8F2,
        0x6272A4,
    ];
    let lines: [&[(usize, f32)]; 13] = [
        &[(0, 34.0), (1, 58.0), (5, 20.0)],
        &[(6, 210.0)],
        &[(0, 26.0), (4, 46.0), (5, 12.0), (2, 96.0)],
        &[(5, 60.0), (3, 120.0), (5, 30.0)],
        &[(0, 40.0), (1, 70.0), (5, 16.0), (4, 54.0)],
        &[(2, 150.0), (5, 22.0)],
        &[(5, 12.0)],
        &[],
        &[(0, 34.0), (1, 82.0), (5, 20.0)],
        &[(4, 64.0), (5, 14.0), (3, 132.0)],
        &[(0, 48.0), (2, 88.0), (5, 34.0), (1, 40.0)],
        &[(6, 170.0)],
        &[(5, 12.0)],
    ];
    for (i, toks) in lines.iter().enumerate() {
        let ly = y + 64.0 + i as f32 * 24.0;
        let num = format!("{}", i + 1);
        f.regular.draw(
            c,
            &num,
            (x + 20.0) * K,
            (ly + 9.0) * K,
            11.0 * K,
            Color::hex(0x565B73),
        );
        let indent = match i {
            1 | 2 | 5 | 9 | 10 | 11 => 24.0,
            3 | 4 => 48.0,
            _ => 0.0,
        };
        let mut tx = x + 52.0 + indent;
        for (col, tw) in toks.iter() {
            c.fill_rrect(
                r(tx, ly, *tw, 10.0),
                5.0 * K,
                Color::hex(palette[*col]).alpha(0.9),
            );
            tx += tw + 8.0;
        }
    }
    RectF::new(x, y, w, h)
}

fn taskbar(c: &mut Canvas, f: &Fonts) -> RectF {
    let (x, y, w, h) = (0.0, LH - 46.0, LW, 46.0);
    c.fill_rect(r(x, y, w, h), Color::rgba(0x12, 0x14, 0x1C, 225));
    let cols = [
        0x3B82F6u32,
        0xF59E0B,
        0x10B981,
        0xEF4444,
        0x8B5CF6,
        0xEC4899,
        0x14B8A6,
    ];
    let start = LW / 2.0 - cols.len() as f32 * 21.0;
    for (i, col) in cols.iter().enumerate() {
        c.fill_rrect(
            r(start + i as f32 * 42.0, y + 9.0, 28.0, 28.0),
            7.0 * K,
            Color::hex(*col),
        );
    }
    // Krab lives in the tray.
    krab::brand::draw_mark(c, r(LW - 150.0, y + 11.0, 24.0, 24.0), false);
    f.regular.draw(
        c,
        "14:35",
        (LW - 62.0) * K,
        (y + 21.0) * K,
        11.5 * K,
        Color::hex(0xE5E7EB),
    );
    f.regular.draw(
        c,
        "12.11.2026",
        (LW - 76.0) * K,
        (y + 36.0) * K,
        10.5 * K,
        Color::hex(0x9CA3AF),
    );
    RectF::new(x, y, w, h)
}

/// Draw the fake desktop and report its windows, top-most first.
fn desktop() -> (Vec<u32>, Vec<WinInfo>) {
    let f = fonts();
    let mut px = vec![0xFF00_0000u32; (W * H) as usize];
    let mut c = Canvas::new(&mut px, W, H);
    wallpaper(&mut c, W as f32, H as f32);
    let report = report_window(&mut c, &f);
    let editor = editor_window(&mut c, &f);
    let bar = taskbar(&mut c, &f);
    let info = |rect: RectF, title: &str| WinInfo {
        rect: RectF::new(rect.x * K, rect.y * K, rect.w * K, rect.h * K).round(),
        title: title.into(),
    };
    (
        px,
        vec![
            info(bar, "taskbar"),
            info(editor, "editor"),
            info(report, "report"),
        ],
    )
}

fn overlay(px: &[u32], windows: &[WinInfo], mode: Mode, kind: Kind) -> Overlay {
    let shot = Shot {
        native: RectI::new(0, 0, W as i32, H as i32),
        w: W,
        h: H,
        px: px.to_vec(),
        scale: K,
        primary: true,
        name: "demo".into(),
    };
    Overlay::new(
        shot,
        windows,
        Options::default(),
        Shared {
            mode,
            kind,
            delay: 3,
        },
    )
}

/// Pointer helpers working in logical desktop coordinates.
fn mv(o: &mut Overlay, x: f32, y: f32) {
    o.pointer_moved(x * K, y * K);
}

fn drag(o: &mut Overlay, from: (f32, f32), to: (f32, f32)) {
    mv(o, from.0, from.1);
    o.pointer_button(Button::Left, true);
    for i in 1..=24 {
        let t = i as f32 / 24.0;
        mv(
            o,
            from.0 + (to.0 - from.0) * t,
            from.1 + (to.1 - from.1) * t,
        );
    }
    o.pointer_button(Button::Left, false);
}

fn render(o: &mut Overlay) -> Vec<u32> {
    let mut buf = vec![0u32; (W * H) as usize];
    o.render(&mut buf, W, H);
    buf
}

/// Markup in action: the hero screenshot.
fn scene_annotate(px: &[u32], windows: &[WinInfo]) -> (Vec<u32>, krab::output::Image) {
    let mut o = overlay(px, windows, Mode::Rect, Kind::Snip);
    drag(&mut o, (72.0, 106.0), (700.0, 536.0));

    // Box around the best month.
    o.activate(Btn::Tool(Tool::Rect));
    o.activate(Btn::Color(0));
    o.activate(Btn::Width(1));
    drag(&mut o, (455.0, 300.0), (517.0, 462.0));
    // Arrow from the note to the box.
    o.activate(Btn::Tool(Tool::Arrow));
    drag(&mut o, (612.0, 322.0), (526.0, 340.0));
    // The note itself.
    o.activate(Btn::Tool(Tool::Text));
    o.activate(Btn::Width(0));
    mv(&mut o, 548.0, 300.0);
    o.pointer_button(Button::Left, true);
    o.pointer_button(Button::Left, false);
    o.text_input("Best month!");
    o.key(Key::Enter);
    // Highlight the headline.
    o.activate(Btn::Tool(Tool::Highlighter));
    o.activate(Btn::Color(2));
    o.activate(Btn::Width(2));
    o.set_mods(krab::overlay::Mods {
        shift: true,
        ..Default::default()
    });
    drag(&mut o, (90.0, 135.0), (318.0, 135.0));
    o.set_mods(Default::default());
    // Hide the e-mail address.
    o.activate(Btn::Tool(Tool::Pixelate));
    drag(&mut o, (164.0, 490.0), (316.0, 510.0));
    // A freehand underline with the pen.
    o.activate(Btn::Tool(Tool::Pen));
    o.activate(Btn::Color(4));
    o.activate(Btn::Width(1));
    mv(&mut o, 96.0, 262.0);
    o.pointer_button(Button::Left, true);
    for i in 0..=40 {
        let t = i as f32 / 40.0;
        mv(&mut o, 96.0 + t * 150.0, 262.0 + (t * 14.0).sin() * 2.2);
    }
    o.pointer_button(Button::Left, false);

    let image = o.compose().expect("selection exists");
    // Hover the copy button so its tooltip shows.
    let hover = (642.0, 566.0);
    mv(&mut o, hover.0, hover.1);
    (render(&mut o), image)
}

/// Window snip: hovering highlights the window under the pointer.
fn scene_modes(px: &[u32], windows: &[WinInfo]) -> Vec<u32> {
    let mut o = overlay(px, windows, Mode::Window, Kind::Snip);
    mv(&mut o, 940.0, 450.0);
    render(&mut o)
}

/// Mid-drag with the pixel loupe.
fn scene_precision(px: &[u32], windows: &[WinInfo]) -> Vec<u32> {
    let mut o = overlay(px, windows, Mode::Rect, Kind::Snip);
    let (from, to) = ((104.0, 300.0), (508.7, 446.7));
    mv(&mut o, from.0, from.1);
    o.pointer_button(Button::Left, true);
    for i in 1..=20 {
        let t = i as f32 / 20.0;
        mv(
            &mut o,
            from.0 + (to.0 - from.0) * t,
            from.1 + (to.1 - from.1) * t,
        );
    }
    render(&mut o)
}

/// The right-click menu over a window.
fn scene_menu(px: &[u32], windows: &[WinInfo]) -> Vec<u32> {
    let mut o = overlay(px, windows, Mode::Rect, Kind::Snip);
    mv(&mut o, 860.0, 330.0);
    o.pointer_button(Button::Right, true);
    o.pointer_button(Button::Right, false);
    mv(&mut o, 900.0, 345.0);
    render(&mut o)
}

/// Recording: the area is chosen, one click starts the capture.
fn scene_record(px: &[u32], windows: &[WinInfo]) -> Vec<u32> {
    let mut o = overlay(px, windows, Mode::Rect, Kind::Record);
    drag(&mut o, (742.0, 236.0), (1152.0, 636.0));
    mv(&mut o, 1010.0, 400.0);
    render(&mut o)
}

fn draw_hud(c: &mut Canvas, hud: &Hud, x: f32, y: f32, scale: f32, hovered: bool) -> (f32, f32) {
    let (lw, lh) = hud.size();
    let (w, h) = ((lw * scale).round() as u32, (lh * scale).round() as u32);
    let mut buf = vec![0u32; (w * h) as usize];
    hud.render(&mut buf, w, h, scale, hovered);
    let dst = RectF::new(x, y, w as f32, h as f32);
    c.shadow(dst, 9.0 * scale, 22.0 * scale, 0.6);
    c.draw_image(&buf, w, h, dst, 9.0 * scale);
    (w as f32, h as f32)
}

/// The three status surfaces on a strip of wallpaper.
fn scene_huds(snip: &krab::output::Image) -> (Vec<u32>, u32, u32) {
    let (w, h) = (1800u32, 330u32);
    let s = 2.0;
    let mut px = vec![0xFF00_0000u32; (w * h) as usize];
    let mut c = Canvas::new(&mut px, w, h);
    wallpaper(&mut c, w as f32, h as f32 * 3.0);
    let f = fonts();
    let caption = |c: &mut Canvas, text: &str, cx: f32| {
        let tw = f.semi.measure(text, 22.0);
        f.semi.draw(
            c,
            text,
            cx - tw / 2.0,
            286.0,
            22.0,
            Color::rgba(255, 255, 255, 215),
        );
    };

    let toast = Hud::Toast {
        title: "Copied to clipboard".into(),
        detail: "Saved to Screenshots - click to open".into(),
        thumb: Some(hud::thumbnail(snip, 320, 220)),
        action: None,
    };
    let countdown = Hud::Countdown {
        deadline: Instant::now() + Duration::from_millis(2600),
    };
    let recording = Hud::Recording {
        started: Instant::now() - Duration::from_secs(83),
        saving: false,
    };
    // Lay the three surfaces out as one centred row.
    let gap = 80.0;
    let widths = [
        toast.size().0 * s,
        countdown.size().0 * s,
        recording.size().0 * s,
    ];
    let mut x = (w as f32 - widths.iter().sum::<f32>() - 2.0 * gap) / 2.0;
    let row_mid = 140.0;
    for (hud, label) in [
        (&toast, "Instant confirmation"),
        (&countdown, "Delayed snips"),
        (&recording, "Screen recording"),
    ] {
        let y = row_mid - hud.size().1 * s / 2.0;
        let (hw, _) = draw_hud(&mut c, hud, x.round(), y.round(), s, false);
        caption(&mut c, label, x + hw / 2.0);
        x += hw + gap;
    }
    (px, w, h)
}

fn banner(shot: &[u32]) -> (Vec<u32>, u32, u32) {
    let (w, h) = (1800u32, 760u32);
    let f = fonts();
    let mut px = vec![0xFF00_0000u32; (w * h) as usize];
    let mut c = Canvas::new(&mut px, w, h);
    let full = rrect_path(RectF::new(0.0, 0.0, w as f32, h as f32), 0.0).unwrap();
    c.fill_path_gradient(
        &full,
        Pt::new(0.0, 0.0),
        Color::hex(0x15161C),
        Pt::new(w as f32, h as f32),
        Color::hex(0x241A1F),
    );
    c.fill_path_radial(
        &full,
        Pt::new(1500.0, 760.0),
        900.0,
        Color::rgba(0xF4, 0x66, 0x23, 120),
        Color::rgba(0xF4, 0x66, 0x23, 0),
    );
    c.fill_path_radial(
        &full,
        Pt::new(100.0, 0.0),
        700.0,
        Color::rgba(0x4F, 0x46, 0xE5, 70),
        Color::rgba(0x4F, 0x46, 0xE5, 0),
    );

    // Left: identity.
    krab::brand::draw_mark(&mut c, RectF::new(96.0, 110.0, 150.0, 150.0), true);
    f.bold
        .draw(&mut c, "Krab", 276.0, 232.0, 128.0, Color::WHITE);
    f.semi.draw(
        &mut c,
        "The snipping tool that opens",
        100.0,
        368.0,
        46.0,
        Color::hex(0xF3F4F6),
    );
    let adv = f
        .semi
        .draw(&mut c, "in ", 100.0, 426.0, 46.0, Color::hex(0xF3F4F6));
    f.semi.draw(
        &mut c,
        "milliseconds.",
        100.0 + adv,
        426.0,
        46.0,
        theme::ACCENT_HOVER,
    );
    f.regular.draw(
        &mut c,
        "Capture, annotate, record - written in Rust,",
        100.0,
        492.0,
        25.0,
        Color::hex(0xB6BAC6),
    );
    f.regular.draw(
        &mut c,
        "rendered on the CPU, one small binary.",
        100.0,
        526.0,
        25.0,
        Color::hex(0xB6BAC6),
    );

    let mut x = 100.0;
    for label in ["Windows", "macOS", "Linux"] {
        let tw = f.semi.measure(label, 21.0);
        let pill = RectF::new(x, 584.0, tw + 44.0, 46.0);
        c.fill_rrect(pill, 23.0, Color::rgba(255, 255, 255, 22));
        c.stroke_rrect(
            pill.inflate(-0.5),
            23.0,
            1.0,
            Color::rgba(255, 255, 255, 44),
        );
        f.semi
            .draw(&mut c, label, x + 22.0, 614.0, 21.0, Color::hex(0xE5E7EB));
        x += tw + 60.0;
    }

    // Right: the real overlay, tilted out of frame.
    let frame = RectF::new(870.0, 110.0, 1060.0, 1060.0 * H as f32 / W as f32);
    c.shadow(frame, 18.0, 60.0, 0.9);
    c.draw_image(shot, W, H, frame, 18.0);
    c.stroke_rrect(
        frame.inflate(-0.5),
        18.0,
        1.5,
        Color::rgba(255, 255, 255, 50),
    );
    (px, w, h)
}

/// The settings window, with one shortcut taken by another app.
fn scene_settings() -> (Vec<u32>, u32, u32) {
    use krab::hotkey::Status;
    let cfg = krab::config::Config {
        hotkey: "PrintScreen".into(),
        hotkey_alt: "Alt+Shift+S".into(),
        hotkey_record: "Alt+Shift+V".into(),
        // A neutral path instead of the machine's real one.
        save_dir: "Pictures/Screenshots".into(),
        ..Default::default()
    };
    let mut ui = krab::settings::Settings::new(
        cfg,
        [Status::Active, Status::Taken, Status::Active],
        true,
        false,
    );
    let s = 2.0;
    let (w, h) = (
        (krab::settings::SIZE.0 * s) as u32,
        (krab::settings::SIZE.1 * s) as u32,
    );
    let mut px = vec![0u32; (w * h) as usize];
    ui.render(&mut px, w, h, s);
    (px, w, h)
}

/// PNG-compressed ICO with the usual Windows sizes.
fn write_ico(path: &str) {
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let pngs: Vec<Vec<u8>> = sizes
        .iter()
        .map(|s| {
            // The badge needs room to read; tiny sizes use the bare crab.
            let rgba = krab::brand::mark_rgba(*s, *s >= 32);
            let mut out = Vec::new();
            image::write_buffer_with_format(
                &mut std::io::Cursor::new(&mut out),
                &rgba,
                *s,
                *s,
                image::ColorType::Rgba8,
                image::ImageFormat::Png,
            )
            .expect("encode png");
            out
        })
        .collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, png) in sizes.iter().zip(&pngs) {
        let dim = if *s >= 256 { 0u8 } else { *s as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(png.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += png.len() as u32;
    }
    for png in &pngs {
        ico.extend_from_slice(png);
    }
    std::fs::write(path, ico).expect("write ico");
    println!("wrote {path}");
}

fn main() {
    std::fs::create_dir_all("assets").unwrap();

    write_ico("assets/icon.ico");
    let icon = krab::brand::mark_rgba(512, true);
    image::save_buffer("assets/icon.png", &icon, 512, 512, image::ColorType::Rgba8).unwrap();
    println!("wrote assets/icon.png");

    let (px, windows) = desktop();
    let (annotate, snip) = scene_annotate(&px, &windows);
    save("assets/screenshot-annotate.png", &annotate, W, H);
    save(
        "assets/screenshot-window.png",
        &scene_modes(&px, &windows),
        W,
        H,
    );
    save(
        "assets/screenshot-precision.png",
        &scene_precision(&px, &windows),
        W,
        H,
    );
    save(
        "assets/screenshot-record.png",
        &scene_record(&px, &windows),
        W,
        H,
    );
    let (huds, hw, hh) = scene_huds(&snip);
    save("assets/screenshot-status.png", &huds, hw, hh);
    save(
        "assets/screenshot-menu.png",
        &scene_menu(&px, &windows),
        W,
        H,
    );
    let (st, sw, sh) = scene_settings();
    save("assets/screenshot-settings.png", &st, sw, sh);
    let (b, bw, bh) = banner(&annotate);
    save("assets/banner.png", &b, bw, bh);
}
