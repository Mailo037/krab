use super::*;
use crate::capture::{Shot, WinInfo, WindowCache, WindowImage};

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
fn right_click_opens_a_menu_that_closes_and_copies_colours() {
    let mut o = overlay(Mode::Rect);
    o.pointer_moved(100.0, 100.0);
    o.pointer_button(Button::Right, true);
    assert!(o.menu.is_some());
    assert!(o.take_outcome().is_none());

    // "Copy colour" puts the hex code of the pixel under the click on the clipboard.
    let idx = o
        .menu
        .as_ref()
        .unwrap()
        .items
        .iter()
        .position(|i| *i == Some(menu::Item::CopyColor))
        .unwrap();
    let (_, rows) = o.menu_layout().unwrap();
    let c = rows[idx].unwrap().center();
    click(&mut o, (c.x, c.y));
    let want = Color::from_u32(src_px(100, 100)).to_hex_string();
    assert!(matches!(o.take_outcome(), Some(Outcome::CopyText(h)) if h == want));
    assert!(o.menu.is_none());

    // Esc closes the menu, a second Esc closes the overlay.
    o.pointer_button(Button::Right, true);
    o.key(Key::Escape);
    assert!(o.menu.is_none() && o.take_outcome().is_none());
    o.key(Key::Escape);
    assert!(matches!(o.take_outcome(), Some(Outcome::Cancel)));
}

#[test]
fn menu_selects_the_window_under_the_pointer() {
    let windows = [WinInfo {
        rect: RectI::new(200, 150, 300, 200),
        title: "editor".into(),
        id: 1,
    }];
    let mut o = Overlay::new(
        shot(),
        &windows,
        Options::default(),
        Shared {
            mode: Mode::Rect,
            kind: Kind::Snip,
            delay: 3,
        },
    );
    o.pointer_moved(300.0, 250.0);
    o.pointer_button(Button::Right, true);
    let items = o.menu.as_ref().unwrap().items.clone();
    let idx = items
        .iter()
        .position(|i| *i == Some(menu::Item::SnipWindow))
        .expect("window entry");
    let (_, rows) = o.menu_layout().unwrap();
    let c = rows[idx].unwrap().center();
    click(&mut o, (c.x, c.y));
    assert_eq!(o.selection(), Some(RectI::new(200, 150, 300, 200)));
}

#[test]
fn clicking_outside_the_menu_only_closes_it() {
    let mut o = overlay(Mode::Rect);
    o.pointer_moved(100.0, 100.0);
    o.pointer_button(Button::Right, true);
    drag(&mut o, (600.0, 500.0), (700.0, 600.0));
    assert!(o.menu.is_none());
    assert!(o.selection().is_none(), "the closing click must not select");
}

#[test]
fn window_mode_selects_topmost_window_under_cursor() {
    let wins = [
        WinInfo {
            rect: RectI::new(100, 80, 120, 90),
            title: "front".into(),
            id: 2,
        },
        WinInfo {
            rect: RectI::new(60, 50, 300, 200),
            title: "back".into(),
            id: 3,
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

fn ctrl_key(o: &mut Overlay, c: char, shift: bool) {
    o.set_mods(Mods {
        ctrl: true,
        shift,
        ..Mods::default()
    });
    o.key(Key::Char(c));
    o.set_mods(Mods::default());
}

#[test]
fn undo_and_redo_restore_moves_and_resizes() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (200.0, 180.0));
    let first = RectI::new(100, 100, 100, 80);
    drag(&mut o, (150.0, 140.0), (170.0, 150.0)); // move by (20, 10)
    let moved = RectI::new(120, 110, 100, 80);
    drag(&mut o, (220.0, 190.0), (260.0, 220.0)); // bottom-right handle
    let resized = RectI::new(120, 110, 140, 110);
    assert_eq!(o.selection(), Some(resized));

    ctrl_key(&mut o, 'z', false);
    assert_eq!(o.selection(), Some(moved));
    ctrl_key(&mut o, 'z', false);
    assert_eq!(o.selection(), Some(first));
    ctrl_key(&mut o, 'z', false);
    assert_eq!(o.selection(), Some(first), "nothing older to undo");
    ctrl_key(&mut o, 'y', false);
    assert_eq!(o.selection(), Some(moved));
    ctrl_key(&mut o, 'z', true); // Ctrl+Shift+Z redoes too
    assert_eq!(o.selection(), Some(resized));
}

#[test]
fn a_run_of_arrow_keys_is_one_undo_step() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (200.0, 180.0));
    let first = RectI::new(100, 100, 100, 80);
    for _ in 0..5 {
        o.key(Key::Right);
    }
    o.set_mods(Mods {
        ctrl: true,
        ..Mods::default()
    });
    o.key(Key::Down); // Ctrl+arrow resizes
    o.set_mods(Mods::default());
    let nudged = RectI::new(105, 100, 100, 81);
    assert_eq!(o.selection(), Some(nudged));
    ctrl_key(&mut o, 'z', false);
    assert_eq!(o.selection(), Some(first));
    ctrl_key(&mut o, 'y', false);
    assert_eq!(o.selection(), Some(nudged));
}

#[test]
fn undo_walks_back_through_markup_and_moves_in_order() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (20.0, 20.0), (380.0, 280.0));
    let first = RectI::new(20, 20, 360, 260);
    o.activate(Btn::Tool(Tool::Rect));
    drag(&mut o, (100.0, 100.0), (200.0, 180.0));
    o.activate(Btn::Tool(Tool::Select));
    drag(&mut o, (300.0, 250.0), (310.0, 260.0)); // move by (10, 10)
    assert_eq!(o.selection(), Some(RectI::new(30, 30, 360, 260)));
    assert_eq!(o.annotation_count(), 1);

    o.undo();
    assert_eq!(o.selection(), Some(first));
    assert_eq!(o.annotation_count(), 1, "the move is undone first");
    o.undo();
    assert_eq!(o.annotation_count(), 0);
    o.redo();
    o.redo();
    assert_eq!(o.selection(), Some(RectI::new(30, 30, 360, 260)));
    assert_eq!(o.annotation_count(), 1);
}

#[test]
fn escape_during_a_move_puts_the_selection_back() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (200.0, 180.0));
    o.pointer_moved(150.0, 140.0);
    o.pointer_button(Button::Left, true);
    o.pointer_moved(190.0, 170.0);
    o.key(Key::Escape);
    o.pointer_button(Button::Left, false);
    assert_eq!(o.selection(), Some(RectI::new(100, 100, 100, 80)));
    assert!(o.undo.is_empty(), "a cancelled move leaves no undo step");
}

/// Stand-in for the platform: every window renders as solid 0x123456,
/// except its top-left pixel, which it does not draw.
fn fake_window(_id: u64) -> Option<WindowImage> {
    let (w, h) = (300, 200);
    let mut px = vec![0xFF12_3456; (w * h) as usize];
    px[0] = 0;
    Some(WindowImage { w, h, px })
}

fn stacked_windows(mode: Mode) -> Overlay {
    let wins = [
        WinInfo {
            rect: RectI::new(100, 80, 120, 90),
            title: "front".into(),
            id: 1,
        },
        WinInfo {
            rect: RectI::new(60, 50, 300, 200),
            title: "back".into(),
            id: 2,
        },
    ];
    let opts = Options {
        grab_window: Some(fake_window),
        ..Options::default()
    };
    Overlay::new(
        shot(),
        &wins,
        opts,
        Shared {
            mode,
            kind: Kind::Snip,
            delay: 3,
        },
    )
}

#[test]
fn a_covered_corner_takes_the_window_edge_not_the_window_on_top() {
    let wins = [
        WinInfo {
            rect: RectI::new(40, 30, 80, 60), // lies on the back window's corner
            title: "front".into(),
            id: 1,
        },
        WinInfo {
            rect: RectI::new(60, 50, 300, 200),
            title: "back".into(),
            id: 2,
        },
    ];
    let opts = Options {
        grab_window: Some(fake_window),
        ..Options::default()
    };
    let shared = Shared {
        mode: Mode::Window,
        kind: Kind::Snip,
        delay: 3,
    };
    let mut o = Overlay::new(shot(), &wins, opts, shared);
    click(&mut o, (300.0, 200.0));
    assert_eq!(o.selection(), Some(RectI::new(60, 50, 300, 200)));
    assert_eq!(pixel(&o.compose().unwrap(), 0, 0), [0x12, 0x34, 0x56, 255]);
}

#[test]
fn window_snip_leaves_out_the_windows_on_top() {
    let mut o = stacked_windows(Mode::Window);
    click(&mut o, (70.0, 60.0)); // the back window, beside the front one
    assert_eq!(o.selection(), Some(RectI::new(60, 50, 300, 200)));
    let img = o.compose().unwrap();
    // Where the front window lay, the back window's own pixels show.
    assert_eq!(pixel(&img, 100, 60), [0x12, 0x34, 0x56, 255]);
    assert_eq!(pixel(&img, 299, 199), [0x12, 0x34, 0x56, 255]);
    // A pixel the window did not draw keeps the screen.
    assert_eq!(pixel(&img, 0, 0), [0, 60, 50, 255]);

    // Starting over brings back the screen as captured.
    o.key(Key::Escape);
    o.activate(Btn::Mode(Mode::Rect));
    drag(&mut o, (60.0, 50.0), (360.0, 250.0));
    assert_eq!(pixel(&o.compose().unwrap(), 100, 60), [0, 160, 110, 255]);
}

/// The same window as `fake_window`, rendered when the snip started:
/// its content has moved on since.
fn earlier_window(_id: u64) -> Option<WindowImage> {
    let (w, h) = (300, 200);
    Some(WindowImage {
        w,
        h,
        px: vec![0xFF65_4321; (w * h) as usize],
    })
}

#[test]
fn window_snip_shows_the_window_as_it_was_when_the_snip_started() {
    let mut o = stacked_windows(Mode::Window);
    o.opts.precaptured = Some(WindowCache::with_grabber(vec![2], earlier_window));
    click(&mut o, (70.0, 60.0));
    assert_eq!(o.selection(), Some(RectI::new(60, 50, 300, 200)));
    assert_eq!(
        pixel(&o.compose().unwrap(), 100, 60),
        [0x65, 0x43, 0x21, 255]
    );
}

#[test]
fn window_snip_of_the_top_window_keeps_the_screen() {
    let mut o = stacked_windows(Mode::Window);
    click(&mut o, (150.0, 100.0));
    assert_eq!(o.selection(), Some(RectI::new(100, 80, 120, 90)));
    assert_eq!(pixel(&o.compose().unwrap(), 10, 10), [0, 110, 90, 255]);
}

#[test]
fn menu_window_snip_also_leaves_out_the_windows_on_top() {
    let mut o = stacked_windows(Mode::Rect);
    o.pointer_moved(70.0, 60.0);
    o.pointer_button(Button::Right, true);
    let items = o.menu.as_ref().unwrap().items.clone();
    let idx = items
        .iter()
        .position(|i| *i == Some(menu::Item::SnipWindow))
        .expect("window entry");
    o.run_menu(idx);
    assert_eq!(o.selection(), Some(RectI::new(60, 50, 300, 200)));
    assert_eq!(
        pixel(&o.compose().unwrap(), 100, 60),
        [0x12, 0x34, 0x56, 255]
    );
}

#[test]
fn recording_a_window_shows_the_live_screen() {
    let mut o = stacked_windows(Mode::Window);
    o.activate(Btn::Kind(Kind::Record));
    click(&mut o, (70.0, 60.0));
    assert_eq!(o.selection(), Some(RectI::new(60, 50, 300, 200)));
    assert_eq!(o.px[(100 * W + 150) as usize], src_px(150, 100));
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

fn delay_slot(o: &Overlay) -> RectF {
    o.bars()
        .main
        .as_ref()
        .unwrap()
        .slots
        .iter()
        .find(|s| s.btn == Btn::Delay)
        .unwrap()
        .rect
}

fn right_click(o: &mut Overlay, at: Pt) {
    o.pointer_moved(at.x, at.y);
    o.pointer_button(Button::Right, true);
    o.pointer_button(Button::Right, false);
}

#[test]
fn right_click_on_the_delay_button_offers_the_waits() {
    let mut o = overlay(Mode::Rect);
    let slot = delay_slot(&o);
    right_click(&mut o, slot.center());
    let menu = o.menu.as_ref().expect("menu open");
    let items = menu.items.clone();
    assert_eq!(
        items,
        menu::DELAY_STEPS
            .map(|n| Some(menu::Item::Delay(n)))
            .to_vec()
    );
    let (frame, rows) = o.menu_layout().unwrap();
    assert!(frame.y >= slot.bottom(), "hangs below the button");

    // Choosing a wait sets it; it does not start it.
    let i = items
        .iter()
        .position(|it| *it == Some(menu::Item::Delay(10)))
        .unwrap();
    let r = rows[i].unwrap().center();
    click(&mut o, (r.x, r.y));
    assert!(o.menu.is_none());
    assert_eq!(o.shared().delay, 10);
    assert!(o.take_outcome().is_none());

    // A second right click on the button closes its menu again.
    right_click(&mut o, slot.center());
    assert!(o.menu.as_ref().is_some_and(|m| m.is_delay()));
    right_click(&mut o, slot.center());
    assert!(o.menu.is_none());

    // Right click elsewhere still opens the usual menu.
    right_click(&mut o, Pt::new(500.0, 500.0));
    assert!(o.menu.as_ref().is_some_and(|m| !m.is_delay()));
    o.key(Key::Escape);

    let c = delay_slot(&o).center();
    click(&mut o, (c.x, c.y));
    assert!(matches!(o.take_outcome(), Some(Outcome::Delay(10))));
}

#[test]
fn the_delay_menu_also_offers_a_wait_from_the_config() {
    let mut o = Overlay::new(
        shot(),
        &[],
        Options::default(),
        Shared {
            mode: Mode::Rect,
            kind: Kind::Snip,
            delay: 7,
        },
    );
    let slot = delay_slot(&o);
    right_click(&mut o, slot.center());
    let items = &o.menu.as_ref().unwrap().items;
    let waits: Vec<u32> = items
        .iter()
        .filter_map(|it| match it {
            Some(menu::Item::Delay(n)) => Some(*n),
            _ => None,
        })
        .collect();
    assert_eq!(waits, [1, 3, 5, 7, 10, 15]);
}

#[test]
fn menus_fade_in_frame_by_frame_and_end_exactly() {
    let animated = Options {
        animate: true,
        ..Options::default()
    };
    let shared = Shared {
        mode: Mode::Rect,
        kind: Kind::Snip,
        delay: 3,
    };
    let mut o = Overlay::new(shot(), &[], animated, shared);
    let t0 = Instant::now();
    o.clock = Some(t0);
    let mut fc = FrameCheck::new(&mut o);
    assert!(
        o.next_wakeup().is_none(),
        "the first frame does not fade in"
    );

    right_click(&mut o, Pt::new(320.0, 300.0));
    fc.step(&mut o, "menu appears");
    let (frame, _) = o.menu_layout().unwrap();
    let probe = ((frame.y as u32 + 8) * W + frame.center().x as u32) as usize;
    let first = fc.full[probe];
    assert!(o.next_wakeup().is_some_and(|t| t <= t0 + render::FRAME));
    o.tick();
    assert!(o.take_dirty(), "a fading frame asks for the next one");

    o.clock = Some(t0 + render::FADE / 2);
    fc.step(&mut o, "menu half faded in");
    let half = fc.full[probe];
    o.clock = Some(t0 + render::FADE + render::FRAME);
    fc.step(&mut o, "menu faded in");
    let done = fc.full[probe];
    assert!(
        first != half && half != done,
        "{first:08X} {half:08X} {done:08X}"
    );
    assert!(o.next_wakeup().is_none(), "the fade is over");

    // The last frame is exactly the menu drawn without animations.
    let mut still = overlay(Mode::Rect);
    let mut buf = vec![0u32; (W * H) as usize];
    still.render(&mut buf, W, H);
    right_click(&mut still, Pt::new(320.0, 300.0));
    still.render(&mut buf, W, H);
    assert!(buf == fc.full, "settled fade differs from a plain frame");
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
        // The tools button opens the palette; picking the arrow selects it
        // and closes the palette again.
        let tools = slot_center(main, Btn::Tools);
        click(&mut o, (tools.x, tools.y));
        let bars = o.bars();
        assert!(
            bars.props.is_none(),
            "the palette takes the props bar's place"
        );
        let palette = bars.palette.first().expect("palette open");
        assert!(
            main.rect.intersect(&palette.rect).is_empty(),
            "bars overlap"
        );
        assert!(palette.rect.x >= 0.0 && palette.rect.right() <= 1600.0);
        assert!(palette.rect.y >= 0.0 && palette.rect.bottom() <= 900.0);
        let arrow = slot_center(palette, Btn::Tool(Tool::Arrow));
        click(&mut o, (arrow.x, arrow.y));
        assert_eq!(o.tool(), Tool::Arrow);
        assert!(o.bars().palette.is_empty());
    }
}

fn slot_center(bar: &super::layout::Bar, btn: Btn) -> Pt {
    bar.slots
        .iter()
        .find(|s| s.btn == btn)
        .unwrap()
        .rect
        .center()
}

#[test]
fn clicking_beside_the_palette_only_closes_it() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (500.0, 350.0));
    o.activate(Btn::Tool(Tool::Pen));
    o.activate(Btn::Tools);
    assert!(!o.bars().palette.is_empty());
    let before = o.annotations.len();
    drag(&mut o, (200.0, 200.0), (300.0, 260.0));
    assert!(o.bars().palette.is_empty());
    assert_eq!(
        o.annotations.len(),
        before,
        "no stroke from the closing click"
    );
    o.activate(Btn::Tools);
    o.key(Key::Escape);
    assert!(o.bars().palette.is_empty());
    assert!(
        o.has_selection(),
        "Esc closes the palette before the selection"
    );
}

#[test]
fn more_button_opens_the_save_menu() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (500.0, 350.0));
    let bars = o.bars();
    let more = slot_center(bars.main.as_ref().unwrap(), Btn::More);
    click(&mut o, (more.x, more.y));
    let menu = o.menu.as_ref().expect("menu open");
    assert!(menu.items.contains(&Some(super::menu::Item::SaveAs)));
    let i = menu
        .items
        .iter()
        .position(|it| *it == Some(super::menu::Item::Frame))
        .unwrap();
    o.run_menu(i);
    assert!(o.opts.frame.is_some());
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

#[test]
fn a_temporary_size_mismatch_does_not_degrade_the_capture() {
    let wins = [WinInfo {
        rect: RectI::new(100, 80, 120, 90),
        title: "w".into(),
        id: 4,
    }];
    let shared = Shared {
        mode: Mode::Window,
        kind: Kind::Snip,
        delay: 3,
    };
    let mut o = Overlay::new(shot(), &wins, Options::default(), shared);
    // First frame arrives while the window is still small...
    let mut small = vec![0u32; 500 * 400];
    o.render(&mut small, 500, 400);
    assert_eq!(o.size(), (500, 400));
    // ...then it reaches its real size: pixels and window rects are exact again.
    let mut full = vec![0u32; (W * H) as usize];
    o.render(&mut full, W, H);
    assert_eq!(o.size(), (W, H));
    click(&mut o, (150.0, 100.0));
    assert_eq!(o.selection(), Some(RectI::new(100, 80, 120, 90)));
    let img = o.compose().unwrap();
    assert_eq!(pixel(&img, 0, 0), [0, 100, 80, 255]);
    assert_eq!(pixel(&img, 119, 89), [0, 219, 169, 255]);
}

/// Renders every step twice - incrementally into a buffer that keeps the
/// previous frame, and from scratch - and requires identical pixels.
struct FrameCheck {
    inc: Vec<u32>,
    full: Vec<u32>,
    frames: usize,
    partial: usize,
}

impl FrameCheck {
    fn new(o: &mut Overlay) -> Self {
        let mut inc = vec![0u32; (W * H) as usize];
        o.render(&mut inc, W, H);
        Self {
            inc,
            full: vec![0u32; (W * H) as usize],
            frames: 0,
            partial: 0,
        }
    }

    fn step(&mut self, o: &mut Overlay, what: &str) {
        let damage = o.render_frame(&mut self.inc, W, H, true);
        self.frames += 1;
        if let Some(d) = &damage {
            let area: i64 = d.iter().map(|r| r.area()).sum();
            if area < (W * H) as i64 / 2 {
                self.partial += 1;
            }
        }
        self.full.fill(0);
        o.render(&mut self.full, W, H);
        if let Some(i) = self.inc.iter().zip(&self.full).position(|(a, b)| a != b) {
            panic!(
                "{what}: incremental frame differs at ({}, {}): {:08X} vs {:08X}",
                i as u32 % W,
                i as u32 / W,
                self.inc[i],
                self.full[i]
            );
        }
    }

    fn moves(&mut self, o: &mut Overlay, from: (f32, f32), to: (f32, f32), what: &str) {
        for i in 1..=6 {
            let t = i as f32 / 6.0;
            o.pointer_moved(from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t);
            self.step(o, what);
        }
    }

    fn drag(&mut self, o: &mut Overlay, from: (f32, f32), to: (f32, f32), what: &str) {
        o.pointer_moved(from.0, from.1);
        o.pointer_button(Button::Left, true);
        self.step(o, what);
        self.moves(o, from, to, what);
        o.pointer_button(Button::Left, false);
        self.step(o, what);
    }
}

#[test]
fn incremental_frames_match_full_repaints() {
    let mut o = overlay(Mode::Rect);
    let mut fc = FrameCheck::new(&mut o);
    fc.moves(
        &mut o,
        (10.0, 10.0),
        (600.0, 400.0),
        "loupe follows the pointer",
    );
    fc.drag(
        &mut o,
        (100.0, 120.0),
        (520.0, 430.0),
        "dragging a selection",
    );
    fc.moves(
        &mut o,
        (300.0, 300.0),
        (540.0, 470.0),
        "hovering the toolbar",
    );
    o.activate(Btn::Tool(Tool::Pen));
    fc.step(&mut o, "pen selected");
    fc.drag(
        &mut o,
        (150.0, 150.0),
        (60.0, 90.0),
        "pen stroke leaving the selection",
    );
    o.activate(Btn::Tool(Tool::Arrow));
    fc.drag(&mut o, (200.0, 300.0), (400.0, 200.0), "arrow");
    o.activate(Btn::Tool(Tool::Pixelate));
    fc.drag(&mut o, (300.0, 320.0), (380.0, 400.0), "pixelate");
    o.undo();
    fc.step(&mut o, "undo");
    o.redo();
    fc.step(&mut o, "redo");
    o.activate(Btn::Tool(Tool::Text));
    click(&mut o, (250.0, 250.0));
    o.text_input("Krab");
    fc.step(&mut o, "typing");
    o.key(Key::Enter);
    fc.step(&mut o, "text committed");
    o.activate(Btn::Tool(Tool::Select));
    fc.drag(
        &mut o,
        (300.0, 300.0),
        (360.0, 330.0),
        "moving the selection",
    );
    fc.drag(
        &mut o,
        (580.0, 460.0),
        (640.0, 500.0),
        "resizing the selection",
    );
    o.pointer_moved(320.0, 300.0);
    o.pointer_button(Button::Right, true);
    o.pointer_button(Button::Right, false);
    fc.step(&mut o, "menu open");
    fc.moves(&mut o, (330.0, 310.0), (360.0, 380.0), "hovering the menu");
    o.key(Key::Escape);
    fc.step(&mut o, "menu closed");
    o.activate(Btn::Tool(Tool::Pipette));
    fc.moves(&mut o, (300.0, 300.0), (310.0, 320.0), "pipette loupe");
    click(&mut o, (310.0, 320.0));
    fc.step(&mut o, "colour flash");
    o.key(Key::Escape);
    fc.step(&mut o, "selection cleared");
    o.pointer_left();
    fc.step(&mut o, "pointer left");
    assert!(
        fc.partial * 2 > fc.frames,
        "most frames should repaint only a part ({} of {})",
        fc.partial,
        fc.frames
    );

    // Window mode: the highlight jumps between windows.
    let wins = [
        WinInfo {
            rect: RectI::new(100, 80, 220, 190),
            title: "a".into(),
            id: 11,
        },
        WinInfo {
            rect: RectI::new(400, 300, 300, 200),
            title: "b".into(),
            id: 12,
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
    let mut fc = FrameCheck::new(&mut o);
    fc.moves(&mut o, (150.0, 150.0), (500.0, 400.0), "window hover");
    fc.moves(&mut o, (500.0, 400.0), (900.0, 700.0), "off the windows");

    // Freeform selection with markup under a mask.
    let mut o = overlay(Mode::Freeform);
    let mut fc = FrameCheck::new(&mut o);
    o.pointer_moved(200.0, 200.0);
    o.pointer_button(Button::Left, true);
    for (x, y) in [
        (400.0, 210.0),
        (420.0, 400.0),
        (210.0, 420.0),
        (190.0, 300.0),
    ] {
        o.pointer_moved(x, y);
        fc.step(&mut o, "lasso");
    }
    o.pointer_button(Button::Left, false);
    fc.step(&mut o, "lasso closed");
    o.activate(Btn::Tool(Tool::Highlighter));
    fc.drag(
        &mut o,
        (220.0, 300.0),
        (430.0, 310.0),
        "highlighter under a mask",
    );
}

/// `cargo test --release --lib render_timing -- --ignored --nocapture`
#[test]
#[ignore]
fn render_timing() {
    let (w, h) = (3840u32, 2160u32);
    let px = (0..w * h)
        .map(|i| 0xFF00_0000 | (i * 2654435761) >> 8)
        .collect();
    let shot = Shot {
        native: RectI::new(0, 0, w as i32, h as i32),
        w,
        h,
        px,
        scale: 2.0,
        primary: true,
        name: "4k".into(),
    };
    let mut o = Overlay::new(
        shot,
        &[],
        Options::default(),
        Shared {
            mode: Mode::Rect,
            kind: Kind::Snip,
            delay: 3,
        },
    );
    let mut buf = vec![0u32; (w * h) as usize];
    let time = |o: &mut Overlay, buf: &mut Vec<u32>, inc: bool, n: u32| {
        let t = std::time::Instant::now();
        for i in 0..n {
            o.pointer_moved(800.0 + i as f32 * 3.0, 600.0 + i as f32);
            o.render_frame(buf, w, h, inc);
        }
        t.elapsed().as_secs_f64() * 1000.0 / n as f64
    };
    o.render(&mut buf, w, h);
    let full = time(&mut o, &mut buf, false, 30);
    let inc = time(&mut o, &mut buf, true, 30);
    println!("4K pointer move, selecting: full {full:.2} ms, incremental {inc:.2} ms");
    // A big selection with markup, drawing a new stroke.
    o.pointer_moved(200.0, 200.0);
    o.pointer_button(Button::Left, true);
    o.pointer_moved(3600.0, 1900.0);
    o.pointer_button(Button::Left, false);
    o.activate(Btn::Tool(Tool::Pen));
    for k in 0..40 {
        o.pointer_moved(300.0 + k as f32 * 40.0, 300.0);
        o.pointer_button(Button::Left, true);
        for j in 0..60 {
            o.pointer_moved(300.0 + k as f32 * 40.0 + j as f32, 300.0 + j as f32 * 20.0);
        }
        o.pointer_button(Button::Left, false);
    }
    o.render(&mut buf, w, h);
    o.pointer_moved(1000.0, 1000.0);
    o.pointer_button(Button::Left, true);
    let t = std::time::Instant::now();
    for i in 0..30 {
        o.pointer_moved(1000.0 + i as f32 * 4.0, 1000.0 + i as f32 * 2.0);
        o.render_frame(&mut buf, w, h, true);
    }
    let inc_draw = t.elapsed().as_secs_f64() * 1000.0 / 30.0;
    let t = std::time::Instant::now();
    for i in 0..30 {
        o.pointer_moved(1120.0 + i as f32 * 4.0, 1060.0 + i as f32 * 2.0);
        o.render_frame(&mut buf, w, h, false);
    }
    let full_draw = t.elapsed().as_secs_f64() * 1000.0 / 30.0;
    println!("4K drawing over 40 strokes: full {full_draw:.2} ms, incremental {inc_draw:.2} ms");
}

#[test]
fn isolated_windows_render_incrementally_like_full_frames() {
    let mut o = stacked_windows(Mode::Window);
    let mut fc = FrameCheck::new(&mut o);
    fc.moves(&mut o, (300.0, 220.0), (70.0, 60.0), "hovering the windows");
    click(&mut o, (70.0, 60.0));
    fc.step(&mut o, "back window isolated");
    o.key(Key::Escape);
    fc.step(&mut o, "screen restored");
    fc.moves(&mut o, (70.0, 60.0), (150.0, 100.0), "hovering again");
}

#[test]
fn step_markers_count_up_and_callouts_take_text() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (20.0, 20.0), (600.0, 400.0));
    o.activate(Btn::Tool(Tool::Step));
    click(&mut o, (100.0, 100.0));
    click(&mut o, (200.0, 100.0));
    let ns: Vec<u32> = o
        .annotations
        .iter()
        .filter_map(|a| match a.shape {
            Shape::Step { n, .. } => Some(n),
            _ => None,
        })
        .collect();
    assert_eq!(ns, [1, 2]);

    o.activate(Btn::Tool(Tool::Callout));
    drag(&mut o, (300.0, 300.0), (420.0, 200.0));
    assert!(o.is_typing());
    o.text_input("Here");
    o.key(Key::Enter);
    match &o.annotations.last().unwrap().shape {
        Shape::Callout { tip, text, .. } => {
            assert_eq!(*tip, Pt::new(300.0, 300.0));
            assert_eq!(text, "Here");
        }
        other => panic!("expected a callout, got {other:?}"),
    }
    // An empty callout is dropped like empty text.
    drag(&mut o, (310.0, 310.0), (320.0, 320.0));
    o.key(Key::Enter);
    assert_eq!(o.annotation_count(), 3);
}

#[test]
fn measure_and_blur_leave_marks_in_the_output() {
    let mut o = overlay(Mode::Rect);
    // A fine checkerboard: blurring turns it grey.
    o.px = (0..W * H)
        .map(|i| {
            if ((i % W) / 2 + (i / W) / 2) & 1 == 0 {
                0xFF00_0000
            } else {
                0xFFFF_FFFF
            }
        })
        .collect();
    drag(&mut o, (20.0, 20.0), (600.0, 400.0));
    let clean = o.compose().unwrap();
    o.activate(Btn::Tool(Tool::Blur));
    drag(&mut o, (100.0, 100.0), (200.0, 180.0));
    o.activate(Btn::Tool(Tool::Measure));
    drag(&mut o, (100.0, 300.0), (400.0, 300.0));
    assert_eq!(o.annotation_count(), 2);
    let marked = o.compose().unwrap();
    assert_ne!(marked, clean);
    let [r, g, b, _] = pixel(&marked, 130, 130);
    assert!(
        (100..156).contains(&r) && r == g && g == b,
        "not blurred: {r}"
    );
    assert!([0, 255].contains(&pixel(&clean, 130, 130)[0]));
}

#[test]
fn redactions_cover_the_reported_boxes() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (500.0, 400.0));
    o.activate(Btn::Redact);
    match o.take_outcome() {
        Some(Outcome::Redact(img)) => assert_eq!((img.w, img.h), (400, 300)),
        _ => panic!("expected a redaction request"),
    }
    o.apply_redactions(Ok(vec![RectF::new(10.0, 20.0, 50.0, 12.0)]));
    let img = o.compose().unwrap();
    assert_eq!(pixel(&img, 30, 25), [0x0B, 0x0B, 0x0D, 255]);
    assert_ne!(pixel(&img, 5, 5), [0x0B, 0x0B, 0x0D, 255]);
    o.undo();
    assert_eq!(o.annotation_count(), 0);
}

#[test]
fn frame_puts_the_snip_on_a_padded_background() {
    let mut o = overlay(Mode::Rect);
    drag(&mut o, (100.0, 100.0), (300.0, 250.0));
    o.activate(Btn::Frame);
    let img = o.compose().unwrap();
    assert!(img.w > 200 + 40 && img.h > 150 + 40, "{}x{}", img.w, img.h);
    assert!(img.is_opaque());
    o.activate(Btn::Frame);
    assert_eq!(o.compose().unwrap().w, 200);
}

#[test]
fn text_and_scroll_requests_finish_on_selection() {
    let mut o = overlay(Mode::Rect);
    o.opts.quick_text = true;
    drag(&mut o, (100.0, 100.0), (300.0, 250.0));
    assert!(matches!(
        o.take_outcome(),
        Some(Outcome::Finish(Finish::ExtractText, _))
    ));

    let mut o = overlay(Mode::Rect);
    o.opts.scroll = true;
    drag(&mut o, (100.0, 100.0), (300.0, 250.0));
    match o.take_outcome() {
        Some(Outcome::Scroll(r)) => assert_eq!(r, RectI::new(100, 100, 200, 150)),
        _ => panic!("expected a scrolling capture"),
    }
}

#[test]
fn narrow_screens_get_a_two_row_palette() {
    let mut o = overlay(Mode::Rect);
    let (w, h) = (420u32, 800u32);
    o.px = vec![0xFF20_2020u32; (w * h) as usize];
    o.w = w;
    o.h = h;
    drag(&mut o, (60.0, 100.0), (360.0, 400.0));
    o.activate(Btn::Tools);
    let bars = o.bars();
    assert_eq!(bars.palette.len(), 2, "a second row");
    let main = bars.main.as_ref().unwrap();
    for b in bars.iter() {
        assert!(
            b.rect.x >= 0.0 && b.rect.right() <= w as f32,
            "{:?}",
            b.rect
        );
    }
    assert!(main.rect.intersect(&bars.palette[0].rect).is_empty());
    assert!(bars.palette[0]
        .rect
        .intersect(&bars.palette[1].rect)
        .is_empty());
    // Every tool is still reachable.
    let count: usize = bars.palette.iter().map(|b| b.slots.len()).sum();
    assert_eq!(count, super::layout::palette_len());
}

#[test]
fn new_tools_render_incrementally_like_full_frames() {
    let mut o = overlay(Mode::Rect);
    let mut fc = FrameCheck::new(&mut o);
    fc.drag(&mut o, (60.0, 60.0), (640.0, 460.0), "selection");
    o.activate(Btn::Tool(Tool::Step));
    o.pointer_moved(120.0, 120.0);
    o.pointer_button(Button::Left, true);
    o.pointer_button(Button::Left, false);
    fc.step(&mut o, "step marker");
    o.activate(Btn::Tool(Tool::Blur));
    fc.drag(&mut o, (150.0, 150.0), (260.0, 240.0), "blur");
    o.activate(Btn::Tool(Tool::Measure));
    fc.drag(&mut o, (100.0, 400.0), (500.0, 330.0), "measure");
    o.activate(Btn::Tool(Tool::Callout));
    fc.drag(&mut o, (300.0, 300.0), (420.0, 200.0), "callout placed");
    o.text_input("Callout text");
    fc.step(&mut o, "typing a callout");
    o.key(Key::Enter);
    fc.step(&mut o, "callout committed");
}

/// `KRAB_PREVIEW=out.png cargo test --lib preview_new_tools -- --ignored`
#[test]
#[ignore]
fn preview_new_tools() {
    let mut o = overlay(Mode::Rect);
    o.px = (0..W * H)
        .map(|i| {
            let (x, y) = (i % W, i / W);
            if (x / 40 + y / 40) % 2 == 0 {
                0xFFF4_F5F7
            } else {
                0xFFE2_E5EA
            }
        })
        .collect();
    drag(&mut o, (40.0, 40.0), (960.0, 560.0));
    o.activate(Btn::Tool(Tool::Step));
    click(&mut o, (120.0, 120.0));
    click(&mut o, (200.0, 120.0));
    o.activate(Btn::Color(4));
    click(&mut o, (280.0, 120.0));
    o.activate(Btn::Tool(Tool::Callout));
    o.activate(Btn::Color(0));
    drag(&mut o, (420.0, 300.0), (520.0, 160.0));
    o.text_input("Click here first");
    o.key(Key::Enter);
    o.activate(Btn::Tool(Tool::Measure));
    drag(&mut o, (120.0, 420.0), (620.0, 420.0));
    drag(&mut o, (700.0, 220.0), (860.0, 480.0));
    o.activate(Btn::Tool(Tool::Blur));
    drag(&mut o, (120.0, 460.0), (400.0, 540.0));
    o.apply_redactions(Ok(vec![RectF::new(500.0, 440.0, 160.0, 22.0)]));
    o.pointer_moved(500.0, 700.0);
    let mut buf = vec![0u32; (W * H) as usize];
    o.render(&mut buf, W, H);
    let path = std::env::var("KRAB_PREVIEW").unwrap_or_else(|_| "preview.png".into());
    image::save_buffer(
        &path,
        &crate::gfx::argb_to_rgba(&buf),
        W,
        H,
        image::ColorType::Rgba8,
    )
    .unwrap();
}

/// `KRAB_PREVIEW=out cargo test --lib preview_toolbar -- --ignored` writes
/// out-closed.png, out-tools.png and out-more.png.
#[test]
#[ignore]
fn preview_toolbar() {
    let (w, h) = (1100u32, 720u32);
    let mut o = overlay(Mode::Rect);
    o.px = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            if (x / 40 + y / 40) % 2 == 0 {
                0xFFF4_F5F7
            } else {
                0xFFE2_E5EA
            }
        })
        .collect();
    o.w = w;
    o.h = h;
    drag(&mut o, (150.0, 80.0), (950.0, 470.0));
    o.activate(Btn::Tool(Tool::Arrow));
    drag(&mut o, (300.0, 300.0), (500.0, 200.0));
    let base = std::env::var("KRAB_PREVIEW").unwrap_or_else(|_| "preview".into());
    let shoot = |o: &mut Overlay, name: &str| {
        let mut buf = vec![0u32; (w * h) as usize];
        o.render(&mut buf, w, h);
        image::save_buffer(
            format!("{base}-{name}.png"),
            &crate::gfx::argb_to_rgba(&buf),
            w,
            h,
            image::ColorType::Rgba8,
        )
        .unwrap();
    };
    o.pointer_moved(1050.0, 680.0);
    shoot(&mut o, "closed");
    let tools = slot_center(o.bars().main.as_ref().unwrap(), Btn::Tools);
    click(&mut o, (tools.x, tools.y));
    let pen = slot_center(&o.bars().palette[0], Btn::Tool(Tool::Pen));
    o.pointer_moved(pen.x, pen.y);
    shoot(&mut o, "tools");
    o.key(Key::Escape);
    let more = slot_center(o.bars().main.as_ref().unwrap(), Btn::More);
    click(&mut o, (more.x, more.y));
    shoot(&mut o, "more");
}
