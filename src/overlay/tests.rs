use super::*;
use crate::capture::{Shot, WinInfo};

// Large enough that the toolbars never cover the areas the tests draw in.
const W: u32 = 1000;
const H: u32 = 800;

/// A synthetic screen: every pixel encodes its own coordinates (mod 256).
fn src_px(x: u32, y: u32) -> u32 {
    0xFF00_0000 | (x & 0xFF) << 8 | (y & 0xFF)
}

fn shot() -> Shot {
    let px = (0..W * H).map(|i| src_px(i % W, i / W)).collect();
    Shot {
        native: RectI::new(0, 0, W as i32, H as i32),
        w: W,
        h: H,
        px,
        scale: 1.0,
        primary: true,
        name: "test".into(),
    }
}

fn overlay(mode: Mode) -> Overlay {
    Overlay::new(
        shot(),
        &[],
        Options::default(),
        Shared {
            mode,
            kind: Kind::Snip,
            delay: 3,
        },
    )
}

fn drag(o: &mut Overlay, from: (f32, f32), to: (f32, f32)) {
    o.pointer_moved(from.0, from.1);
    o.pointer_button(Button::Left, true);
    // A few intermediate samples, like a real mouse.
    for i in 1..=8 {
        let t = i as f32 / 8.0;
        o.pointer_moved(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
    }
    o.pointer_button(Button::Left, false);
}

fn click(o: &mut Overlay, at: (f32, f32)) {
    o.pointer_moved(at.0, at.1);
    o.pointer_button(Button::Left, true);
    o.pointer_button(Button::Left, false);
}

fn pixel(img: &Image, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * img.w + x) * 4) as usize;
    [
        img.rgba[i],
        img.rgba[i + 1],
        img.rgba[i + 2],
        img.rgba[i + 3],
    ]
}

fn finished(o: &mut Overlay) -> (Finish, Image) {
    match o.take_outcome() {
        Some(Outcome::Finish(kind, img)) => (kind, img),
        _ => panic!("expected a finished capture"),
    }
}

#[test]
fn rect_drag_selects_and_copy_crops_exact_pixels() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (50.0, 40.0), (250.0, 190.0));
    assert_eq!(o.selection(), Some(RectI::new(50, 40, 200, 150)));
    assert!(
        o.take_outcome().is_none(),
        "edit mode keeps the overlay open"
    );

    o.key(Key::Enter);
    let (kind, img) = finished(&mut o);
    assert_eq!(kind, Finish::Copy);
    assert_eq!((img.w, img.h), (200, 150));
    // Top-left output pixel is screen pixel (50, 40): green = x, blue = y.
    assert_eq!(pixel(&img, 0, 0), [0, 50, 40, 255]);
    assert_eq!(pixel(&img, 199, 149), [0, 249, 189, 255]);
}

#[test]
fn reverse_drag_and_out_of_bounds_are_normalized() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (300.0, 200.0), (-50.0, -20.0));
    assert_eq!(o.selection(), Some(RectI::new(0, 0, 300, 200)));
}

#[test]
fn tiny_drag_is_ignored() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (50.0, 50.0), (51.0, 51.0));
    assert_eq!(o.selection(), None);
}

#[test]
fn quick_copy_finishes_on_release() {
    let opts = Options {
        quick_copy: true,
        ..Options::default()
    };
    let mut o = Overlay::new(
        shot(),
        &[],
        opts,
        Shared {
            mode: Mode::Rect,
            kind: Kind::Snip,
            delay: 3,
        },
    );
    drag(&mut o, (10.0, 10.0), (110.0, 60.0));
    let (kind, img) = finished(&mut o);
    assert_eq!(kind, Finish::Copy);
    assert_eq!((img.w, img.h), (100, 50));
}

#[test]
fn escape_steps_back_then_cancels() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (50.0, 40.0), (250.0, 190.0));
    o.key(Key::Escape);
    assert_eq!(o.selection(), None);
    assert!(o.take_outcome().is_none());
    o.key(Key::Escape);
    assert!(matches!(o.take_outcome(), Some(Outcome::Cancel)));
}

#[test]
fn right_click_cancels_when_nothing_is_selected() {
    let mut o = overlay(Mode::Rect);
    o.pointer_moved(100.0, 100.0);
    o.pointer_button(Button::Right, true);
    assert!(matches!(o.take_outcome(), Some(Outcome::Cancel)));
}

#[test]
fn window_mode_selects_topmost_window_under_cursor() {
    let wins = [
        WinInfo {
            rect: RectI::new(100, 80, 120, 90),
            title: "front".into(),
        },
        WinInfo {
            rect: RectI::new(60, 50, 300, 200),
            title: "back".into(),
        },
    ];
    let mut o = Overlay::new(
        shot(),
        &wins,
        Options::default(),
        Shared {
            mode: Mode::Window,
            kind: Kind::Snip,
            delay: 3,
        },
    );
    click(&mut o, (150.0, 100.0));
    assert_eq!(o.selection(), Some(RectI::new(100, 80, 120, 90)));

    let mut o = Overlay::new(
        shot(),
        &wins,
        Options::default(),
        Shared {
            mode: Mode::Window,
            kind: Kind::Snip,
            delay: 3,
        },
    );
    click(&mut o, (70.0, 240.0));
    assert_eq!(o.selection(), Some(RectI::new(60, 50, 300, 200)));

    // No window there: the whole monitor.
    let mut o = Overlay::new(
        shot(),
        &wins,
        Options::default(),
        Shared {
            mode: Mode::Window,
            kind: Kind::Snip,
            delay: 3,
        },
    );
    click(&mut o, (10.0, 290.0));
    assert_eq!(o.selection(), Some(RectI::new(0, 0, W as i32, H as i32)));
}

#[test]
fn fullscreen_mode_selects_everything() {
    let mut o = overlay(Mode::Rect);
    o.activate(Btn::Mode(Mode::Fullscreen));
    assert_eq!(o.selection(), Some(RectI::new(0, 0, W as i32, H as i32)));
    assert_eq!(o.shared().mode, Mode::Fullscreen);
}

#[test]
fn freeform_output_is_transparent_outside_the_lasso() {
    let mut o = overlay(Mode::Freeform);
    // A triangle.
    o.pointer_moved(100.0, 50.0);
    o.pointer_button(Button::Left, true);
    for (x, y) in [(200.0, 250.0), (20.0, 250.0), (100.0, 50.0)] {
        for i in 1..=20 {
            let t = i as f32 / 20.0;
            let from = o.cursor;
            o.pointer_moved(from.x + (x - from.x) * t, from.y + (y - from.y) * t);
        }
    }
    o.pointer_button(Button::Left, false);
    let sel = o.selection().expect("lasso creates a selection");
    // Samples closer than 1.5px are skipped, so allow a pixel of slack.
    assert!((sel.x - 20).abs() <= 1 && sel.y == 50, "{sel:?}");

    let img = o.compose().unwrap();
    assert!(!img.is_opaque());
    assert_eq!(
        pixel(&img, 1, 1)[3],
        0,
        "bounding-box corner is outside the triangle"
    );
    let inside = pixel(&img, (100 - sel.x) as u32, (200 - sel.y) as u32);
    assert_eq!(inside, [0, 100, 200, 255]);
}

#[test]
fn drawing_tools_leave_ink_and_undo_restores() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (20.0, 20.0), (380.0, 280.0));
    let clean = o.compose().unwrap();

    o.activate(Btn::Tool(Tool::Rect));
    o.activate(Btn::Color(4)); // blue
    drag(&mut o, (100.0, 100.0), (200.0, 180.0));
    assert_eq!(o.annotation_count(), 1);
    let inked = o.compose().unwrap();
    assert_ne!(inked, clean);
    // The rectangle's left edge at screen (100, 140) is blue now.
    let [r, g, b, _] = pixel(&inked, 80, 120);
    assert_eq!([r, g, b], [0x0A, 0x84, 0xFF]);

    o.undo();
    assert_eq!(o.annotation_count(), 0);
    assert_eq!(o.compose().unwrap(), clean);
    o.redo();
    assert_eq!(o.compose().unwrap(), inked);
}

#[test]
fn annotations_never_bleed_outside_the_selection() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (200.0, 200.0));
    o.activate(Btn::Tool(Tool::Pen));
    o.activate(Btn::Width(2));
    // Start inside, scribble far outside (up and to the left).
    drag(&mut o, (150.0, 150.0), (30.0, 90.0));
    assert_eq!(o.annotation_count(), 1);
    let mut buf = vec![0u32; (W * H) as usize];
    o.pointer_left();
    o.render(&mut buf, W, H);
    // (120, 135) lies on the stroke inside the selection: inked.
    assert_eq!(buf[(135 * W + 120) as usize], theme::PALETTE[0].to_u32());
    // (60, 105) lies on the stroke too, but outside: dimmed original, no ink.
    let src = src_px(60, 105);
    let expect = 0xFF00_0000 | (((src >> 1) & 0x7F7F7F) - ((src >> 4) & 0x0F0F0F));
    assert_eq!(buf[(105 * W + 60) as usize], expect);
}

#[test]
fn eraser_removes_the_stroke_it_touches() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (20.0, 20.0), (380.0, 280.0));
    o.activate(Btn::Tool(Tool::Line));
    drag(&mut o, (60.0, 60.0), (300.0, 60.0));
    drag(&mut o, (60.0, 200.0), (300.0, 200.0));
    assert_eq!(o.annotation_count(), 2);
    o.activate(Btn::Tool(Tool::Eraser));
    click(&mut o, (180.0, 61.0));
    assert_eq!(o.annotation_count(), 1);
    click(&mut o, (180.0, 130.0)); // empty space
    assert_eq!(o.annotation_count(), 1);
    o.undo();
    assert_eq!(o.annotation_count(), 2);
}

#[test]
fn text_tool_types_and_commits() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (20.0, 20.0), (380.0, 280.0));
    let clean = o.compose().unwrap();
    o.key(Key::Char('t'));
    assert_eq!(o.tool(), Tool::Text);
    click(&mut o, (100.0, 100.0));
    assert!(o.is_typing());
    o.text_input("Hi");
    o.key(Key::Char('p')); // must not switch tools while typing
    assert_eq!(o.tool(), Tool::Text);
    o.key(Key::Enter);
    assert!(!o.is_typing());
    assert_eq!(o.annotation_count(), 1);
    assert_ne!(o.compose().unwrap(), clean);
}

#[test]
fn empty_text_is_discarded() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (20.0, 20.0), (380.0, 280.0));
    o.activate(Btn::Tool(Tool::Text));
    click(&mut o, (100.0, 100.0));
    o.key(Key::Escape);
    assert_eq!(o.annotation_count(), 0);
    assert!(o.selection().is_some(), "escape only left text entry");
}

#[test]
fn pixelate_hides_detail() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (0.0, 0.0), (400.0, 300.0));
    o.activate(Btn::Tool(Tool::Pixelate));
    drag(&mut o, (90.0, 90.0), (180.0, 180.0));
    let img = o.compose().unwrap();
    // Neighbouring pixels inside one mosaic block are identical...
    assert_eq!(pixel(&img, 100, 100), pixel(&img, 101, 101));
    // ...while the untouched area still has per-pixel detail.
    assert_ne!(pixel(&img, 300, 100), pixel(&img, 301, 101));
}

#[test]
fn pipette_copies_hex_and_keeps_overlay_open() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (20.0, 20.0), (380.0, 280.0));
    o.activate(Btn::Tool(Tool::Pipette));
    click(&mut o, (0x40 as f32, 0x30 as f32));
    match o.take_outcome() {
        Some(Outcome::CopyText(hex)) => assert_eq!(hex, "#004030"),
        _ => panic!("expected CopyText"),
    }
    assert!(o.selection().is_some());
}

#[test]
fn move_and_resize_adjust_the_selection() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (200.0, 180.0));
    drag(&mut o, (150.0, 140.0), (170.0, 150.0)); // move by (20, 10)
    assert_eq!(o.selection(), Some(RectI::new(120, 110, 100, 80)));
    drag(&mut o, (220.0, 190.0), (260.0, 220.0)); // bottom-right handle
    assert_eq!(o.selection(), Some(RectI::new(120, 110, 140, 110)));
    o.key(Key::Left);
    assert_eq!(o.selection(), Some(RectI::new(119, 110, 140, 110)));
}

#[test]
fn record_kind_reports_the_area() {
    let mut o = overlay(Mode::Rect);
    o.activate(Btn::Kind(Kind::Record));
    drag(&mut o, (40.0, 40.0), (240.0, 140.0));
    assert!(o.take_outcome().is_none());
    o.key(Key::Enter);
    match o.take_outcome() {
        Some(Outcome::Record(r)) => assert_eq!(r, RectI::new(40, 40, 200, 100)),
        _ => panic!("expected Record"),
    }
}

#[test]
fn record_kind_disables_freeform() {
    let mut o = overlay(Mode::Freeform);
    o.activate(Btn::Kind(Kind::Record));
    assert_eq!(o.shared().mode, Mode::Rect);
    o.activate(Btn::Mode(Mode::Freeform));
    assert_eq!(o.shared().mode, Mode::Rect);
}

#[test]
fn delay_button_requests_a_delayed_restart() {
    let mut o = overlay(Mode::Rect);
    // Hover the delay button, scroll to change, click.
    let bars = o.bars();
    let slot = bars
        .main
        .as_ref()
        .unwrap()
        .slots
        .iter()
        .find(|s| s.btn == Btn::Delay)
        .unwrap();
    let c = slot.rect.center();
    o.pointer_moved(c.x, c.y);
    o.scroll(1.0);
    assert_eq!(o.shared().delay, 5);
    click(&mut o, (c.x, c.y));
    assert!(matches!(o.take_outcome(), Some(Outcome::Delay(5))));
}

#[test]
fn toolbar_buttons_are_clickable_and_stay_on_screen() {
    for sel in [
        ((10.0, 10.0), (390.0, 290.0)),
        ((150.0, 100.0), (250.0, 160.0)),
        ((5.0, 250.0), (60.0, 295.0)),
    ] {
        let mut o = overlay(Mode::Rect);
        // A screen wide enough to hold the toolbar.
        let px = vec![0xFF20_2020u32; 1600 * 900];
        o.px = px;
        o.w = 1600;
        o.h = 900;
        drag(
            &mut o,
            (sel.0 .0 * 3.0, sel.0 .1 * 3.0),
            (sel.1 .0 * 3.0, sel.1 .1 * 3.0),
        );
        o.activate(Btn::Tool(Tool::Pen)); // shows the properties bar too
        let bars = o.bars();
        for bar in bars.iter() {
            assert!(
                bar.rect.x >= 0.0 && bar.rect.right() <= 1600.0,
                "{:?}",
                bar.rect
            );
            assert!(
                bar.rect.y >= 0.0 && bar.rect.bottom() <= 900.0,
                "{:?}",
                bar.rect
            );
            for slot in &bar.slots {
                assert!(bar.rect.contains(slot.rect.center()));
            }
        }
        let main = bars.main.as_ref().unwrap();
        let props = bars.props.as_ref().unwrap();
        assert!(main.rect.intersect(&props.rect).is_empty(), "bars overlap");
        // Clicking the arrow tool through real pointer events selects it.
        let arrow = main
            .slots
            .iter()
            .find(|s| s.btn == Btn::Tool(Tool::Arrow))
            .unwrap()
            .rect
            .center();
        click(&mut o, (arrow.x, arrow.y));
        assert_eq!(o.tool(), Tool::Arrow);
    }
}

#[test]
fn render_fills_every_pixel_opaque() {
    let mut o = overlay(Mode::Rect);
    let mut buf = vec![0u32; (W * H) as usize];
    o.pointer_moved(200.0, 150.0);
    o.render(&mut buf, W, H);
    assert!(buf.iter().all(|p| p >> 24 == 0xFF));
    drag(&mut o, (50.0, 40.0), (250.0, 190.0));
    o.render(&mut buf, W, H);
    assert!(buf.iter().all(|p| p >> 24 == 0xFF));
    // Selection interior shows the original pixels (away from any chrome).
    assert_eq!(buf[(120 * W + 150) as usize], src_px(150, 120));
}

#[test]
fn render_adapts_to_a_differently_sized_surface() {
    let mut o = overlay(Mode::Rect);
    let (w, h) = (W / 2, H / 2);
    let mut buf = vec![0u32; (w * h) as usize];
    o.render(&mut buf, w, h);
    assert_eq!(o.size(), (w, h));
    assert!(buf.iter().all(|p| p >> 24 == 0xFF));
}

#[test]
fn border_option_frames_the_output() {
    let opts = Options {
        border: Some((Color::rgb(1, 2, 3), 2)),
        ..Options::default()
    };
    let mut o = Overlay::new(
        shot(),
        &[],
        opts,
        Shared {
            mode: Mode::Rect,
            kind: Kind::Snip,
            delay: 3,
        },
    );
    drag(&mut o, (50.0, 40.0), (250.0, 190.0));
    let img = o.compose().unwrap();
    assert_eq!(pixel(&img, 0, 75), [1, 2, 3, 255]);
    assert_eq!(pixel(&img, 199, 75), [1, 2, 3, 255]);
    assert_ne!(pixel(&img, 100, 75), [1, 2, 3, 255]);
}

#[test]
fn mode_parsing_accepts_aliases() {
    assert_eq!(Mode::parse("Rect"), Some(Mode::Rect));
    assert_eq!(Mode::parse("fullscreen"), Some(Mode::Fullscreen));
    assert_eq!(Mode::parse("lasso"), Some(Mode::Freeform));
    assert_eq!(Mode::parse("nope"), None);
    for m in [Mode::Rect, Mode::Freeform, Mode::Window, Mode::Fullscreen] {
        assert_eq!(Mode::parse(m.as_str()), Some(m));
    }
}

#[test]
fn full_hd_drag_then_enter_keeps_the_dragged_size() {
    let (w, h) = (1920u32, 1080u32);
    let shot = Shot {
        native: RectI::new(0, 0, w as i32, h as i32),
        w,
        h,
        px: vec![0xFF33_4455; (w * h) as usize],
        scale: 1.0,
        primary: true,
        name: "fhd".into(),
    };
    for quick in [false, true] {
        let opts = Options {
            quick_copy: quick,
            ..Options::default()
        };
        let mut o = Overlay::new(
            shot_clone(&shot),
            &[],
            opts,
            Shared {
                mode: Mode::Rect,
                kind: Kind::Snip,
                delay: 3,
            },
        );
        let mut buf = vec![0u32; (w * h) as usize];
        o.pointer_moved(300.0, 300.0);
        o.render(&mut buf, w, h);
        o.pointer_button(Button::Left, true);
        for i in 1..=12 {
            o.pointer_moved(
                300.0 + 320.0 * i as f32 / 12.0,
                300.0 + 200.0 * i as f32 / 12.0,
            );
            o.render(&mut buf, w, h);
        }
        o.pointer_button(Button::Left, false);
        o.render(&mut buf, w, h);
        if !quick {
            o.key(Key::Enter);
        }
        let (_, img) = finished(&mut o);
        assert_eq!((img.w, img.h), (320, 200), "quick={quick}");
    }
}

fn shot_clone(s: &Shot) -> Shot {
    Shot {
        native: s.native,
        w: s.w,
        h: s.h,
        px: s.px.clone(),
        scale: s.scale,
        primary: s.primary,
        name: s.name.clone(),
    }
}
