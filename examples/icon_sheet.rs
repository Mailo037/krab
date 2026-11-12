//! Renders every icon and the app mark to `target/icon-sheet.png` so they can
//! be checked by eye while drawing them.
//!
//! `cargo run --example icon_sheet`
//!
//! Each icon appears three times: large on its 24px grid (with the 2px safe
//! area outlined), at toolbar size, and at toolbar size on a 1x display.

use krab::geom::{Pt, RectF};
use krab::gfx::icons::{self, Icon};
use krab::gfx::{argb_to_rgba, rrect_path, theme, Canvas, Color};

const CELL: f32 = 150.0;
const BIG: f32 = 96.0;
const COLS: usize = 8;

fn main() {
    let rows = icons::ALL.len().div_ceil(COLS) + 1;
    let (w, h) = (
        (CELL * COLS as f32) as u32,
        (CELL * rows as f32 + 40.0) as u32,
    );
    let mut px = vec![0u32; (w * h) as usize];
    let mut c = Canvas::new(&mut px, w, h);
    let full = rrect_path(RectF::new(0.0, 0.0, w as f32, h as f32), 0.0).unwrap();
    c.fill_path(&full, theme::PANEL_SOLID);

    for (i, icon) in icons::ALL.iter().enumerate() {
        let (col, row) = ((i % COLS) as f32, (i / COLS) as f32);
        let x0 = col * CELL;
        let y0 = row * CELL;
        draw_cell(&mut c, *icon, x0, y0);
    }

    // The app mark: badge (app icon) and bare crab (tray) at several sizes.
    let y = (rows - 1) as f32 * CELL + 20.0;
    let mut x = 20.0;
    for (size, badge) in [
        (128.0, true),
        (64.0, true),
        (32.0, true),
        (64.0, false),
        (32.0, false),
        (16.0, false),
    ] {
        krab::brand::draw_mark(
            &mut c,
            RectF::new(x, y + (128.0 - size) / 2.0, size, size),
            badge,
        );
        x += size + 30.0;
    }

    std::fs::create_dir_all("target").unwrap();
    let rgba = argb_to_rgba(&px);
    image::save_buffer(
        "target/icon-sheet.png",
        &rgba,
        w,
        h,
        image::ColorType::Rgba8,
    )
    .unwrap();
    println!("wrote target/icon-sheet.png ({w}x{h})");
}

fn draw_cell(c: &mut Canvas, icon: Icon, x0: f32, y0: f32) {
    // Grid box and safe area for the large rendering.
    let k = BIG / 24.0;
    let gx = x0 + (CELL - BIG) / 2.0;
    let gy = y0 + 10.0;
    c.stroke_rrect(
        RectF::new(gx, gy, BIG, BIG),
        0.0,
        1.0,
        Color::rgba(255, 255, 255, 18),
    );
    c.stroke_rrect(
        RectF::new(gx + 2.0 * k, gy + 2.0 * k, 20.0 * k, 20.0 * k),
        0.0,
        1.0,
        Color::rgba(0xF4, 0x66, 0x23, 60),
    );
    icons::draw(
        c,
        icon,
        Pt::new(gx + BIG / 2.0, gy + BIG / 2.0),
        BIG,
        theme::ICON,
    );

    // Toolbar size at 2x and 1x.
    let by = gy + BIG + 22.0;
    icons::draw(
        c,
        icon,
        Pt::new(x0 + CELL / 2.0 - 20.0, by),
        theme::ICON_SIZE * 2.0,
        theme::ICON,
    );
    icons::draw(
        c,
        icon,
        Pt::new(x0 + CELL / 2.0 + 24.0, by),
        theme::ICON_SIZE,
        theme::ICON,
    );
}
